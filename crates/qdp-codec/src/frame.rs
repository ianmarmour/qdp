//! Uncompressed video frames.

use std::ops::Range;

/// An 8-bit YUV 4:2:0 frame with tightly packed planes.
///
/// The luma plane has `width × height` samples; each chroma plane has
/// `⌈width / 2⌉ × ⌈height / 2⌉`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yuv420Frame {
    /// The width in pixels.
    width: usize,
    /// The height in pixels.
    height: usize,
    /// The luma plane, row by row.
    y: Vec<u8>,
    /// The blue-difference chroma plane, row by row.
    u: Vec<u8>,
    /// The red-difference chroma plane, row by row.
    v: Vec<u8>,
}

/// The luma value of black in limited range.
const LIMITED_BLACK: u8 = 16;

/// The chroma value of a neutral colour.
const NEUTRAL_CHROMA: u8 = 128;

/// The number of fractional bits of the RGB to YUV coefficients.
const FRACTION_BITS: u32 = 16;

/// BT.709 luma weights of red, green and blue, scaled to limited range
/// (219/255) in 16-bit fixed point.
const LUMA_WEIGHTS: [i32; 3] = [11_966, 40_254, 4_064];

/// BT.709 blue-difference weights, scaled to limited range (224/255).
const CB_WEIGHTS: [i32; 3] = [-6_598, -22_186, 28_784];

/// BT.709 red-difference weights, scaled to limited range (224/255).
const CR_WEIGHTS: [i32; 3] = [28_784, -26_148, -2_636];

/// The bytes of one 32-bit RGB pixel.
const RGB32_PIXEL: usize = 4;

/// The fewest pixels a frame has before converting or scaling it is split
/// between threads: below it, starting them costs more than they save.
const PARALLEL_PIXELS: usize = 1 << 20;

/// The most threads converting or scaling one frame.
const MAX_WORKERS: usize = 8;

/// Returns how many threads should share work on `pixels` pixels.
pub(crate) fn workers(pixels: usize) -> usize {
    if pixels < PARALLEL_PIXELS {
        return 1;
    }
    std::thread::available_parallelism().map_or(1, |cores| cores.get().min(MAX_WORKERS))
}

/// The order of the colour bytes of a 32-bit pixel; the fourth byte, alpha
/// or padding, is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rgb32Order {
    /// Blue, green, red, then padding: `BGRx` or `BGRA`.
    Bgr,
    /// Red, green, blue, then padding: `RGBx` or `RGBA`.
    Rgb,
}

/// Returns, for each of `target` positions along one side of a scaled plane,
/// the range of the `source` positions it covers, never empty.
fn spans(source: usize, target: usize) -> Vec<Range<usize>> {
    (0..target)
        .map(|index| {
            let start = index * source / target;
            let end = ((index + 1) * source / target).max(start + 1);
            start..end
        })
        .collect()
}

/// Scales a plane of `source` width and height to `target`, averaging the
/// samples each target sample covers. Large planes are scaled in bands of
/// rows on several threads.
fn scale_plane(
    plane: &[u8],
    (source_width, source_height): (usize, usize),
    (target_width, target_height): (usize, usize),
) -> Vec<u8> {
    let columns = spans(source_width, target_width);
    let rows = spans(source_height, target_height);
    let mut scaled = vec![0; target_width * target_height];
    let rows_per_band = target_height.div_ceil(workers(source_width * source_height));
    std::thread::scope(|scope| {
        for (band, rows) in scaled
            .chunks_mut(rows_per_band * target_width)
            .zip(rows.chunks(rows_per_band))
        {
            let columns = &columns;
            scope.spawn(move || {
                scale_rows(plane, source_width, columns, rows, band);
            });
        }
    });
    scaled
}

/// Scales the target rows `rows` of a plane `source_width` samples wide,
/// whose columns cover `columns`, into `band`. Each target row first adds
/// up, column by column, the source rows it covers, then adds up the
/// columns each target sample covers.
fn scale_rows(
    plane: &[u8],
    source_width: usize,
    columns: &[Range<usize>],
    rows: &[Range<usize>],
    band: &mut [u8],
) {
    // When every target column covers the same number of source columns,
    // as halving a plane gives, each covers the next run of that many.
    let run = source_width / columns.len();
    let uniform = run * columns.len() == source_width;
    let mut column_sums = vec![0_u32; source_width];
    for (row, target) in rows.iter().zip(band.chunks_exact_mut(columns.len())) {
        column_sums.fill(0);
        for line in plane
            .chunks_exact(source_width)
            .skip(row.start)
            .take(row.len())
        {
            for (sum, &sample) in column_sums.iter_mut().zip(line) {
                *sum += u32::from(sample);
            }
        }
        let rows_covered = row.len() as u32;
        if uniform {
            let count = run as u32 * rows_covered;
            for (sample, sums) in target.iter_mut().zip(column_sums.chunks_exact(run)) {
                *sample = ((sums.iter().sum::<u32>() + count / 2) / count) as u8;
            }
            continue;
        }
        for (sample, column) in target.iter_mut().zip(columns) {
            let count = column.len() as u32 * rows_covered;
            let sum: u32 = column_sums[column.clone()].iter().sum();
            *sample = ((sum + count / 2) / count) as u8;
        }
    }
}

/// Applies 16-bit fixed-point weights to red, green and blue, with rounding.
fn weigh(weights: [i32; 3], [red, green, blue]: [i32; 3]) -> i32 {
    (weights[0] * red + weights[1] * green + weights[2] * blue + (1 << (FRACTION_BITS - 1)))
        >> FRACTION_BITS
}

/// A band of rows of a 32-bit RGB image, converted to YUV 4:2:0 on one
/// thread.
#[derive(Debug, Clone, Copy)]
struct Rgb32Band<'a> {
    /// The whole image.
    data: &'a [u8],
    /// The bytes per row of the image.
    stride: usize,
    /// The width in pixels.
    width: usize,
    /// The offsets of red, green and blue in each pixel.
    channels: [usize; 3],
    /// The first row of the band, an even one.
    first_row: usize,
}

impl Rgb32Band<'_> {
    /// Converts the band into its luma rows `y` and chroma rows `u` and
    /// `v`. Each chroma sample is the average colour of the two-by-two
    /// pixels it covers, or of those that exist at the right and bottom
    /// edges.
    fn convert(&self, y: &mut [u8], u: &mut [u8], v: &mut [u8]) {
        let width = self.width;
        let chroma_width = width.div_ceil(2);
        let neutral = i32::from(NEUTRAL_CHROMA);
        let [red, green, blue] = self.channels;
        let colour = |pixel: &[u8]| {
            [
                i32::from(pixel[red]),
                i32::from(pixel[green]),
                i32::from(pixel[blue]),
            ]
        };
        for (pair, ((luma, u), v)) in y
            .chunks_mut(2 * width)
            .zip(u.chunks_exact_mut(chroma_width))
            .zip(v.chunks_exact_mut(chroma_width))
            .enumerate()
        {
            // The last pair of an odd-height image has one row.
            let rows = luma.len() / width;
            let top = self.first_row + 2 * pair;
            let lines: [&[u8]; 2] = [0, rows - 1].map(|row| {
                let start = (top + row) * self.stride;
                &self.data[start..start + width * RGB32_PIXEL]
            });
            for (luma_row, line) in luma.chunks_exact_mut(width).zip(lines) {
                for (sample, pixel) in luma_row.iter_mut().zip(line.chunks_exact(RGB32_PIXEL)) {
                    *sample = (i32::from(LIMITED_BLACK) + weigh(LUMA_WEIGHTS, colour(pixel))) as u8;
                }
            }
            let pairs = lines.map(|line| line.chunks(2 * RGB32_PIXEL));
            let [top_pairs, bottom_pairs] = pairs;
            for (((u, v), top), bottom) in u
                .iter_mut()
                .zip(v.iter_mut())
                .zip(top_pairs)
                .zip(bottom_pairs)
            {
                let mut sum = [0; 3];
                let mut count = 0;
                for pixels in [top, bottom].iter().take(rows) {
                    for pixel in pixels.chunks_exact(RGB32_PIXEL) {
                        for (total, value) in sum.iter_mut().zip(colour(pixel)) {
                            *total += value;
                        }
                        count += 1;
                    }
                }
                let average = sum.map(|total| total / count);
                *u = (neutral + weigh(CB_WEIGHTS, average)).clamp(0, 255) as u8;
                *v = (neutral + weigh(CR_WEIGHTS, average)).clamp(0, 255) as u8;
            }
        }
    }
}

impl Yuv420Frame {
    /// Returns a black frame of `width × height` pixels.
    pub fn black(width: usize, height: usize) -> Self {
        let (chroma_width, chroma_height) = chroma_size(width, height);
        Self {
            width,
            height,
            y: vec![LIMITED_BLACK; width * height],
            u: vec![NEUTRAL_CHROMA; chroma_width * chroma_height],
            v: vec![NEUTRAL_CHROMA; chroma_width * chroma_height],
        }
    }

    /// Creates a frame from its planes, returning `None` if their sizes do not
    /// match the dimensions.
    pub fn from_planes(
        width: usize,
        height: usize,
        y: Vec<u8>,
        u: Vec<u8>,
        v: Vec<u8>,
    ) -> Option<Self> {
        let (chroma_width, chroma_height) = chroma_size(width, height);
        let chroma_len = chroma_width * chroma_height;
        (y.len() == width * height && u.len() == chroma_len && v.len() == chroma_len).then_some(
            Self {
                width,
                height,
                y,
                u,
                v,
            },
        )
    }

    /// Copies an NV12 picture, a luma plane of `luma_stride` bytes per row
    /// and a plane of interleaved Cb and Cr of `chroma_stride` bytes per row,
    /// as hardware decoders produce it, into a frame. Returns `None` if the
    /// planes are too short for `width × height` pixels. The copy runs on
    /// the calling thread alone: pictures are copied only where something
    /// needs them in memory, and the processor's other cores are better
    /// left to the network and the window.
    pub fn from_nv12(
        width: usize,
        height: usize,
        (luma, luma_stride): (&[u8], usize),
        (chroma, chroma_stride): (&[u8], usize),
    ) -> Option<Self> {
        let (chroma_width, chroma_height) = chroma_size(width, height);
        if width == 0 || height == 0 || luma_stride < width || chroma_stride < 2 * chroma_width {
            return None;
        }
        if luma.len() < luma_stride * (height - 1) + width
            || chroma.len() < chroma_stride * (chroma_height - 1) + 2 * chroma_width
        {
            return None;
        }
        let mut frame = Self {
            width,
            height,
            y: vec![0; width * height],
            u: vec![0; chroma_width * chroma_height],
            v: vec![0; chroma_width * chroma_height],
        };
        for (target, source) in frame.y.chunks_mut(width).zip(luma.chunks(luma_stride)) {
            target.copy_from_slice(&source[..width]);
        }
        let rows = frame
            .u
            .chunks_mut(chroma_width)
            .zip(frame.v.chunks_mut(chroma_width))
            .zip(chroma.chunks(chroma_stride));
        for ((u, v), source) in rows {
            let pairs = source[..2 * chroma_width].chunks_exact(2);
            for ((u, v), pair) in u.iter_mut().zip(v.iter_mut()).zip(pairs) {
                *u = pair[0];
                *v = pair[1];
            }
        }
        Some(frame)
    }

    /// Converts a 32-bit RGB image, `stride` bytes per row, to a BT.709
    /// limited-range frame, as standard dynamic range QDP streams use. Chroma
    /// is the average of each 2 × 2 block. Returns `None` if `data` is too
    /// short.
    pub fn from_rgb32(
        width: usize,
        height: usize,
        stride: usize,
        order: Rgb32Order,
        data: &[u8],
    ) -> Option<Self> {
        if width == 0 || height == 0 || stride < width * RGB32_PIXEL {
            return None;
        }
        if data.len() < stride * (height - 1) + width * RGB32_PIXEL {
            return None;
        }
        // Where red, green and blue lie in each pixel.
        let channels = match order {
            Rgb32Order::Bgr => [2, 1, 0],
            Rgb32Order::Rgb => [0, 1, 2],
        };
        let (chroma_width, chroma_height) = chroma_size(width, height);
        let mut frame = Self::black(width, height);
        // Each chroma row covers two luma rows; large frames convert in
        // bands of them on several threads.
        let chroma_rows_per_band = chroma_height.div_ceil(workers(width * height));
        std::thread::scope(|scope| {
            let bands = frame
                .y
                .chunks_mut(2 * chroma_rows_per_band * width)
                .zip(frame.u.chunks_mut(chroma_rows_per_band * chroma_width))
                .zip(frame.v.chunks_mut(chroma_rows_per_band * chroma_width));
            for (index, ((y, u), v)) in bands.enumerate() {
                let band = Rgb32Band {
                    data,
                    stride,
                    width,
                    channels,
                    first_row: 2 * index * chroma_rows_per_band,
                };
                scope.spawn(move || band.convert(y, u, v));
            }
        });
        Some(frame)
    }

    /// Returns the width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Returns the height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// Returns the width of the chroma planes.
    pub fn chroma_width(&self) -> usize {
        chroma_size(self.width, self.height).0
    }

    /// Returns the height of the chroma planes.
    pub fn chroma_height(&self) -> usize {
        chroma_size(self.width, self.height).1
    }

    /// Returns the luma plane.
    pub fn y(&self) -> &[u8] {
        &self.y
    }

    /// Returns the blue-difference chroma plane.
    pub fn u(&self) -> &[u8] {
        &self.u
    }

    /// Returns the red-difference chroma plane.
    pub fn v(&self) -> &[u8] {
        &self.v
    }

    /// Returns the luma plane for writing.
    pub fn y_mut(&mut self) -> &mut [u8] {
        &mut self.y
    }

    /// Returns the blue-difference chroma plane for writing.
    pub fn u_mut(&mut self) -> &mut [u8] {
        &mut self.u
    }

    /// Returns the red-difference chroma plane for writing.
    pub fn v_mut(&mut self) -> &mut [u8] {
        &mut self.v
    }

    /// Returns the top-left `width × height` pixels of the frame, or the frame
    /// itself if it is no larger. Hardware encoders may pad frames to their
    /// alignment, which this removes (Section 6.4.1).
    pub fn cropped(self, width: usize, height: usize) -> Self {
        if width >= self.width && height >= self.height {
            return self;
        }
        let (width, height) = (width.min(self.width), height.min(self.height));
        let (chroma_width, chroma_height) = chroma_size(width, height);
        let crop = |plane: &[u8], stride: usize, width: usize, height: usize| -> Vec<u8> {
            plane
                .chunks_exact(stride)
                .take(height)
                .flat_map(|row| &row[..width])
                .copied()
                .collect()
        };
        let source_chroma_width = self.chroma_width();
        Self {
            width,
            height,
            y: crop(&self.y, self.width, width, height),
            u: crop(&self.u, source_chroma_width, chroma_width, chroma_height),
            v: crop(&self.v, source_chroma_width, chroma_width, chroma_height),
        }
    }

    /// Returns a black `width × height` frame with this one in it at
    /// `(x, y)`, moved to even columns and rows so that its chroma lines up,
    /// and cut where it goes past the edges; or the frame itself if it is
    /// all of such a frame.
    pub fn placed_on_black(self, width: usize, height: usize, x: usize, y: usize) -> Self {
        let (x, y) = (x & !1, y & !1);
        if (x, y, self.width, self.height) == (0, 0, width, height) {
            return self;
        }
        let mut placed = Self::black(width, height);
        let paste =
            |to: &mut [u8], to_width: usize, from: &[u8], from_width: usize, at: (usize, usize)| {
                let (column, row) = at;
                if column >= to_width || from_width == 0 {
                    return;
                }
                let length = from_width.min(to_width - column);
                for (to_row, from_row) in to
                    .chunks_exact_mut(to_width)
                    .skip(row)
                    .zip(from.chunks_exact(from_width))
                {
                    to_row[column..column + length].copy_from_slice(&from_row[..length]);
                }
            };
        let chroma_width = placed.chroma_width();
        let source_chroma_width = self.chroma_width();
        paste(&mut placed.y, width, &self.y, self.width, (x, y));
        paste(
            &mut placed.u,
            chroma_width,
            &self.u,
            source_chroma_width,
            (x / 2, y / 2),
        );
        paste(
            &mut placed.v,
            chroma_width,
            &self.v,
            source_chroma_width,
            (x / 2, y / 2),
        );
        placed
    }

    /// Returns the frame scaled to `width × height` pixels, each sample the
    /// average of the samples it covers, as suits making a frame smaller, or
    /// the frame itself if it has that size.
    pub fn scaled(self, width: usize, height: usize) -> Self {
        if (width, height) == (self.width, self.height) {
            return self;
        }
        self.to_scaled(width, height)
    }

    /// Returns a copy of the frame scaled to `width × height` pixels, each
    /// sample the average of the samples it covers.
    pub fn to_scaled(&self, width: usize, height: usize) -> Self {
        let (chroma_width, chroma_height) = chroma_size(width, height);
        let source_chroma = (self.chroma_width(), self.chroma_height());
        Self {
            width,
            height,
            y: scale_plane(&self.y, (self.width, self.height), (width, height)),
            u: scale_plane(&self.u, source_chroma, (chroma_width, chroma_height)),
            v: scale_plane(&self.v, source_chroma, (chroma_width, chroma_height)),
        }
    }

    /// Returns the peak signal-to-noise ratio of the luma plane against
    /// `reference`, in decibels, or `None` if the sizes differ. Identical planes
    /// give infinity.
    pub fn luma_psnr(&self, reference: &Self) -> Option<f64> {
        if self.width != reference.width || self.height != reference.height {
            return None;
        }
        let squared_error: f64 = self
            .y
            .iter()
            .zip(&reference.y)
            .map(|(&a, &b)| (f64::from(a) - f64::from(b)).powi(2))
            .sum();
        let mean = squared_error / self.y.len() as f64;
        Some(10.0 * (f64::from(u8::MAX).powi(2) / mean).log10())
    }
}

/// Returns the size of the chroma planes of a 4:2:0 frame.
fn chroma_size(width: usize, height: usize) -> (usize, usize) {
    (width.div_ceil(2), height.div_ceil(2))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame placed on black keeps its samples where it is put, moved to
    /// an even place and cut at the edges, and leaves the rest black.
    #[test]
    fn places_a_frame_on_black() {
        let white =
            Yuv420Frame::from_planes(4, 2, vec![235; 8], vec![100; 2], vec![200; 2]).unwrap();
        let placed = white.clone().placed_on_black(8, 4, 5, 1);
        assert_eq!((placed.width(), placed.height()), (8, 4));
        // At (4, 0): luma rows 0 and 1, columns 4 to 7.
        assert_eq!(
            &placed.y()[0..8],
            &[
                LIMITED_BLACK,
                LIMITED_BLACK,
                LIMITED_BLACK,
                LIMITED_BLACK,
                235,
                235,
                235,
                235
            ]
        );
        assert_eq!(&placed.y()[16..24], &[LIMITED_BLACK; 8]);
        assert_eq!(
            placed.u(),
            &[
                NEUTRAL_CHROMA,
                NEUTRAL_CHROMA,
                100,
                100,
                NEUTRAL_CHROMA,
                NEUTRAL_CHROMA,
                NEUTRAL_CHROMA,
                NEUTRAL_CHROMA
            ]
        );
        let cut = white.clone().placed_on_black(6, 2, 4, 0);
        assert_eq!(
            &cut.y()[0..6],
            &[
                LIMITED_BLACK,
                LIMITED_BLACK,
                LIMITED_BLACK,
                LIMITED_BLACK,
                235,
                235
            ]
        );
        let same = white.clone().placed_on_black(4, 2, 0, 0);
        assert_eq!(same.y(), white.y());
    }

    /// An NV12 picture is copied plane by plane, padding left out and
    /// chroma split, for an odd size.
    #[test]
    fn copies_nv12() {
        let (width, height) = (1283, 1081);
        let (chroma_width, chroma_height) = chroma_size(width, height);
        let (luma_stride, chroma_stride) = (1344, 1344);
        let luma: Vec<u8> = (0..luma_stride * height)
            .map(|index| (index % luma_stride % 251) as u8)
            .collect();
        let chroma: Vec<u8> = (0..chroma_stride * chroma_height)
            .map(|index| (index / chroma_stride + index % chroma_stride) as u8)
            .collect();
        let frame = Yuv420Frame::from_nv12(
            width,
            height,
            (&luma, luma_stride),
            (&chroma, chroma_stride),
        )
        .unwrap();
        for row in 0..height {
            assert_eq!(
                &frame.y()[row * width..(row + 1) * width],
                &luma[row * luma_stride..row * luma_stride + width]
            );
        }
        for row in 0..chroma_height {
            for column in 0..chroma_width {
                let pair = row * chroma_stride + 2 * column;
                let sample = row * chroma_width + column;
                assert_eq!(frame.u()[sample], chroma[pair], "Cb at {row}, {column}");
                assert_eq!(frame.v()[sample], chroma[pair + 1], "Cr at {row}, {column}");
            }
        }
        // Planes too short for the size are refused.
        assert!(
            Yuv420Frame::from_nv12(width, height, (&luma[..1000], luma_stride), (&chroma, 1344))
                .is_none()
        );
        assert!(
            Yuv420Frame::from_nv12(width, height, (&luma, width - 1), (&chroma, 1344)).is_none()
        );
    }

    /// Returns `length` bytes of varied content.
    fn noise(length: usize) -> Vec<u8> {
        (0..length)
            .map(|index| (index.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect()
    }

    /// Converts as the first, single-threaded implementation did, pixel by
    /// pixel: the reference the conversion must match exactly.
    fn reference_from_rgb32(
        width: usize,
        height: usize,
        stride: usize,
        order: Rgb32Order,
        data: &[u8],
    ) -> Yuv420Frame {
        let pixel = |x: usize, y: usize| -> [i32; 3] {
            let start = y * stride + x * RGB32_PIXEL;
            let [first, second, third] =
                [data[start], data[start + 1], data[start + 2]].map(i32::from);
            match order {
                Rgb32Order::Bgr => [third, second, first],
                Rgb32Order::Rgb => [first, second, third],
            }
        };
        let (chroma_width, chroma_height) = chroma_size(width, height);
        let mut frame = Yuv420Frame::black(width, height);
        for y in 0..height {
            for x in 0..width {
                frame.y[y * width + x] =
                    (i32::from(LIMITED_BLACK) + weigh(LUMA_WEIGHTS, pixel(x, y))) as u8;
            }
        }
        for chroma_y in 0..chroma_height {
            for chroma_x in 0..chroma_width {
                let mut sum = [0; 3];
                let mut count = 0;
                for y in (2 * chroma_y)..(2 * chroma_y + 2).min(height) {
                    for x in (2 * chroma_x)..(2 * chroma_x + 2).min(width) {
                        let rgb = pixel(x, y);
                        for channel in 0..3 {
                            sum[channel] += rgb[channel];
                        }
                        count += 1;
                    }
                }
                let average = sum.map(|total| total / count);
                let index = chroma_y * chroma_width + chroma_x;
                let neutral = i32::from(NEUTRAL_CHROMA);
                frame.u[index] = (neutral + weigh(CB_WEIGHTS, average)).clamp(0, 255) as u8;
                frame.v[index] = (neutral + weigh(CR_WEIGHTS, average)).clamp(0, 255) as u8;
            }
        }
        frame
    }

    /// Conversion matches the pixel-by-pixel reference exactly, for odd and
    /// even sizes, padded rows, either byte order, and frames large enough
    /// to be converted on several threads.
    #[test]
    fn conversion_matches_reference() {
        for (width, height, padding) in [(1, 1, 0), (5, 3, 4), (64, 33, 0), (1283, 817, 12)] {
            let stride = width * RGB32_PIXEL + padding;
            let data = noise(stride * height);
            for order in [Rgb32Order::Bgr, Rgb32Order::Rgb] {
                assert_eq!(
                    Yuv420Frame::from_rgb32(width, height, stride, order, &data).unwrap(),
                    reference_from_rgb32(width, height, stride, order, &data),
                    "{width}×{height}"
                );
            }
        }
    }

    /// Scales a plane as the first, single-threaded implementation did,
    /// column range by column range: the reference scaling must match.
    fn reference_scale_plane(
        plane: &[u8],
        (source_width, source_height): (usize, usize),
        (target_width, target_height): (usize, usize),
    ) -> Vec<u8> {
        let columns = spans(source_width, target_width);
        let mut scaled = Vec::new();
        for row in spans(source_height, target_height) {
            for column in &columns {
                let sum: u32 = plane
                    .chunks_exact(source_width)
                    .skip(row.start)
                    .take(row.len())
                    .flat_map(|line| &line[column.clone()])
                    .map(|&sample| u32::from(sample))
                    .sum();
                let count = (column.len() * row.len()) as u32;
                scaled.push(((sum + count / 2) / count) as u8);
            }
        }
        scaled
    }

    /// Scaling matches the reference exactly, halving included, for planes
    /// large enough to be scaled on several threads.
    #[test]
    fn scaling_matches_reference() {
        for (width, height, target_width, target_height) in [
            (1283, 817, 641, 408),
            (1283, 817, 640, 360),
            (1283, 817, 1000, 817),
            (1280, 816, 640, 408),
        ] {
            let data = noise(width * height * 4);
            let frame =
                Yuv420Frame::from_rgb32(width, height, width * 4, Rgb32Order::Bgr, &data).unwrap();
            let scaled = frame.to_scaled(target_width, target_height);
            let size = (target_width, target_height);
            assert_eq!(
                scaled.y(),
                reference_scale_plane(frame.y(), (width, height), size),
                "{target_width}×{target_height}"
            );
            let chroma = (frame.chroma_width(), frame.chroma_height());
            let target_chroma = (scaled.chroma_width(), scaled.chroma_height());
            assert_eq!(
                scaled.u(),
                reference_scale_plane(frame.u(), chroma, target_chroma)
            );
        }
    }

    /// Odd dimensions round the chroma planes up.
    #[test]
    fn chroma_rounds_up() {
        let frame = Yuv420Frame::black(5, 3);
        assert_eq!((frame.chroma_width(), frame.chroma_height()), (3, 2));
        assert_eq!(frame.u().len(), 6);
        assert!(Yuv420Frame::from_planes(5, 3, vec![0; 15], vec![0; 6], vec![0; 5]).is_none());
    }

    /// Reference colours convert to their BT.709 limited-range values, in
    /// either byte order.
    #[test]
    fn converts_rgb() {
        let convert = |order, pixel: [u8; 4]| {
            let data: Vec<u8> = pixel.repeat(4);
            let frame = Yuv420Frame::from_rgb32(2, 2, 8, order, &data).unwrap();
            (frame.y()[0], frame.u()[0], frame.v()[0])
        };
        assert_eq!(convert(Rgb32Order::Rgb, [0, 0, 0, 255]), (16, 128, 128));
        assert_eq!(
            convert(Rgb32Order::Rgb, [255, 255, 255, 255]),
            (235, 128, 128)
        );
        assert_eq!(convert(Rgb32Order::Rgb, [255, 0, 0, 0]), (63, 102, 240));
        assert_eq!(convert(Rgb32Order::Bgr, [0, 0, 255, 0]), (63, 102, 240));
        assert_eq!(convert(Rgb32Order::Rgb, [0, 255, 0, 0]), (173, 42, 26));
        assert_eq!(convert(Rgb32Order::Bgr, [255, 0, 0, 0]), (32, 240, 118));
        // Rows may be padded, and the data must cover every row.
        let padded = vec![0; 3 * 12];
        let frame = Yuv420Frame::from_rgb32(3, 3, 12, Rgb32Order::Rgb, &padded).unwrap();
        assert_eq!((frame.chroma_width(), frame.chroma_height()), (2, 2));
        assert!(Yuv420Frame::from_rgb32(3, 3, 12, Rgb32Order::Rgb, &padded[..20]).is_none());
    }

    /// Scaling down averages the samples each output sample covers, in
    /// every plane, and odd sizes keep their rounded-up chroma.
    #[test]
    fn scales_by_averaging() {
        let luma = vec![0, 100, 200, 50, 10, 110, 210, 70];
        let frame = Yuv420Frame::from_planes(4, 2, luma, vec![20, 40], vec![60, 80]).unwrap();
        let half = frame.clone().scaled(2, 1);
        assert_eq!(half.y(), [55, 133]);
        assert_eq!((half.u(), half.v()), (&[30][..], &[70][..]));
        assert_eq!(frame.clone().scaled(4, 2), frame);
        let odd = Yuv420Frame::black(33, 17).scaled(15, 9);
        assert_eq!(
            (odd.width(), odd.chroma_width(), odd.chroma_height()),
            (15, 8, 5)
        );
        assert!(odd.y().iter().all(|&sample| sample == LIMITED_BLACK));
    }

    /// Cropping keeps the top-left pixels of every plane.
    #[test]
    fn crops_padding() {
        let y = (0..4 * 4).map(|value| value as u8).collect();
        let u = vec![1, 2, 3, 4];
        let v = vec![5, 6, 7, 8];
        let frame = Yuv420Frame::from_planes(4, 4, y, u, v).unwrap();
        assert_eq!(frame.clone().cropped(4, 4), frame);
        let cropped = frame.cropped(2, 3);
        assert_eq!((cropped.width(), cropped.height()), (2, 3));
        assert_eq!(cropped.y(), [0, 1, 4, 5, 8, 9]);
        assert_eq!(cropped.u(), [1, 3]);
        assert_eq!(cropped.v(), [5, 7]);
    }

    /// Identical frames have infinite PSNR.
    #[test]
    fn psnr() {
        let frame = Yuv420Frame::black(4, 4);
        assert_eq!(frame.luma_psnr(&frame), Some(f64::INFINITY));
        let mut other = frame.clone();
        other.y_mut()[0] = 17;
        assert!(other.luma_psnr(&frame).unwrap() > 50.0);
    }
}
