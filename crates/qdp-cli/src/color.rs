//! Conversion of decoded frames to RGB for display.
//!
//! Converting and scaling a frame each run on bands of rows in parallel,
//! one band for each processor, since a large window at a high frame rate
//! asks more of one processor than it has.

use std::num::NonZeroUsize;
use std::sync::OnceLock;

use qdp_codec::Yuv420Frame;

/// The number of fractional bits of the fixed-point conversion coefficients.
const FRACTION_BITS: u32 = 16;

/// The luma of black in limited range.
const LUMA_OFFSET: i32 = 16;

/// The chroma of a neutral colour.
const CHROMA_OFFSET: i32 = 128;

/// 255 / 219: scales limited-range luma to full range.
const LUMA_SCALE: i32 = 76_309;

/// The red contribution of Cr in BT.709, scaled from limited range:
/// 2 × (1 − Kr) × 255 / 224.
const RED_FROM_CR: i32 = 117_489;

/// The green contribution of Cb in BT.709, scaled from limited range.
const GREEN_FROM_CB: i32 = 13_975;

/// The green contribution of Cr in BT.709, scaled from limited range.
const GREEN_FROM_CR: i32 = 34_925;

/// The blue contribution of Cb in BT.709, scaled from limited range:
/// 2 × (1 − Kb) × 255 / 224.
const BLUE_FROM_CB: i32 = 138_438;

/// Half of one unit of the fixed-point result, for rounding.
const ROUNDING: i32 = 1 << (FRACTION_BITS - 1);

/// The number of fractional bits of the scaling weights.
const WEIGHT_BITS: u32 = 8;

/// The weight of a whole pixel when scaling.
const WHOLE: u32 = 1 << WEIGHT_BITS;

/// The red and blue components of a `0x00RRGGBB` pixel, blended together.
const RED_BLUE: u32 = 0x00ff_00ff;

/// The green component of a `0x00RRGGBB` pixel.
const GREEN: u32 = 0x0000_ff00;

/// The fewest rows a band holds: smaller bands cost more to hand to a thread
/// than they save.
const MIN_BAND_ROWS: usize = 64;

/// An RGB image with one `0x00RRGGBB` pixel per element, as `softbuffer`
/// expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbFrame {
    /// The width in pixels.
    pub width: usize,
    /// The height in pixels.
    pub height: usize,
    /// The pixels, row by row.
    pub pixels: Vec<u32>,
}

/// Returns one fixed-point colour component as a byte.
fn component(value: i32) -> u32 {
    ((value + ROUNDING) >> FRACTION_BITS).clamp(0, 255) as u32
}

/// Converts a limited-range BT.709 YUV 4:2:0 frame, the format QDP's standard
/// dynamic range streams use, to RGB.
pub fn yuv420_to_rgb(frame: &Yuv420Frame) -> RgbFrame {
    convert_in_bands(frame, band_count(frame.height()))
}

/// Converts `frame` to RGB in `bands` bands of rows at once.
fn convert_in_bands(frame: &Yuv420Frame, bands: usize) -> RgbFrame {
    let (width, height) = (frame.width(), frame.height());
    let mut pixels = vec![0; width * height];
    if width > 0 {
        in_bands(&mut pixels, width, bands, |first_row, band| {
            convert_rows(frame, first_row, band);
        });
    }
    RgbFrame {
        width,
        height,
        pixels,
    }
}

/// Converts the rows of `frame` from `first_row` into `output`, which holds
/// whole rows of RGB pixels.
fn convert_rows(frame: &Yuv420Frame, first_row: usize, output: &mut [u32]) {
    let width = frame.width();
    let chroma_width = frame.chroma_width();
    for (index, output_row) in output.chunks_exact_mut(width).enumerate() {
        let row = first_row + index;
        let luma_row = &frame.y()[row * width..(row + 1) * width];
        let chroma_offset = (row / 2) * chroma_width;
        let u_row = &frame.u()[chroma_offset..chroma_offset + chroma_width];
        let v_row = &frame.v()[chroma_offset..chroma_offset + chroma_width];
        for (column, pixel) in output_row.iter_mut().enumerate() {
            let luma = (i32::from(luma_row[column]) - LUMA_OFFSET) * LUMA_SCALE;
            let cb = i32::from(u_row[column / 2]) - CHROMA_OFFSET;
            let cr = i32::from(v_row[column / 2]) - CHROMA_OFFSET;
            let red = component(luma + RED_FROM_CR * cr);
            let green = component(luma - GREEN_FROM_CB * cb - GREEN_FROM_CR * cr);
            let blue = component(luma + BLUE_FROM_CB * cb);
            *pixel = (red << 16) | (green << 8) | blue;
        }
    }
}

/// Returns how many threads drawing uses: one for each processor, found once.
fn drawing_threads() -> usize {
    static THREADS: OnceLock<usize> = OnceLock::new();
    *THREADS.get_or_init(|| std::thread::available_parallelism().map_or(1, NonZeroUsize::get))
}

/// Returns how many bands `rows` rows are drawn in: one for each thread, of
/// at least [`MIN_BAND_ROWS`] rows.
fn band_count(rows: usize) -> usize {
    (rows / MIN_BAND_ROWS).clamp(1, drawing_threads())
}

/// Runs `work` on up to `bands` bands of whole `row_length`-long rows of
/// `pixels` at once, each given the index of its first row. The calling
/// thread works on the first band.
fn in_bands(
    pixels: &mut [u32],
    row_length: usize,
    bands: usize,
    work: impl Fn(usize, &mut [u32]) + Sync,
) {
    let rows = pixels.len() / row_length.max(1);
    let band_rows = rows.div_ceil(bands.max(1)).max(1);
    if band_rows >= rows {
        work(0, pixels);
        return;
    }
    std::thread::scope(|scope| {
        let mut chunks = pixels.chunks_mut(band_rows * row_length).enumerate();
        let first = chunks.next();
        for (index, band) in chunks {
            let work = &work;
            scope.spawn(move || work(index * band_rows, band));
        }
        if let Some((_, band)) = first {
            work(0, band);
        }
    });
}

/// Where a scaled picture sits in a window: centred, with its aspect ratio
/// kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The picture's left edge in the window, in pixels.
    pub left: usize,
    /// The picture's top edge.
    pub top: usize,
    /// The picture's width in the window.
    pub width: usize,
    /// The picture's height in the window.
    pub height: usize,
}

impl Placement {
    /// Returns where a `frame_width × frame_height` picture is drawn in a
    /// `target_width × target_height` window, or `None` if either is empty.
    pub fn fit(
        frame_width: usize,
        frame_height: usize,
        target_width: usize,
        target_height: usize,
    ) -> Option<Self> {
        if frame_width == 0 || frame_height == 0 || target_width == 0 || target_height == 0 {
            return None;
        }
        // Fit by the dimension that limits: compare the aspect ratios by
        // cross-multiplying.
        let (width, height) = if target_width * frame_height <= target_height * frame_width {
            (
                target_width,
                (frame_height * target_width / frame_width).max(1),
            )
        } else {
            (
                (frame_width * target_height / frame_height).max(1),
                target_height,
            )
        };
        Some(Self {
            left: (target_width - width) / 2,
            top: (target_height - height) / 2,
            width,
            height,
        })
    }

    /// Converts a window position to a position in the picture's pixels,
    /// clamped to the picture.
    pub fn to_frame(self, x: f64, y: f64, frame_width: usize, frame_height: usize) -> (f64, f64) {
        let scale = |position: f64, start: usize, size: usize, frame: usize| {
            let frame = frame as f64;
            ((position - start as f64) * frame / size as f64).clamp(0.0, (frame - 1.0).max(0.0))
        };
        (
            scale(x, self.left, self.width, frame_width),
            scale(y, self.top, self.height, frame_height),
        )
    }

    /// Converts a position in the picture's pixels to a window position.
    pub fn to_window(self, x: f64, y: f64, frame_width: usize, frame_height: usize) -> (f64, f64) {
        (
            self.left as f64 + x * self.width as f64 / frame_width.max(1) as f64,
            self.top as f64 + y * self.height as f64 / frame_height.max(1) as f64,
        )
    }

    /// Returns whether a window position is over the picture.
    pub fn contains(self, x: f64, y: f64) -> bool {
        let (left, top) = (self.left as f64, self.top as f64);
        (left..left + self.width as f64).contains(&x)
            && (top..top + self.height as f64).contains(&y)
    }

    /// Returns the window pixels per picture pixel.
    pub fn scale(self, frame_width: usize) -> f64 {
        self.width as f64 / frame_width.max(1) as f64
    }
}

/// Draws `frame` into a `target_width × target_height` buffer, scaled to fit
/// with its aspect ratio kept and black bars around it. A picture of the
/// window's size is copied as it is; any other is scaled bilinearly, so
/// that edges and text stay smooth rather than turn jagged.
pub fn scale_into(frame: &RgbFrame, target: &mut [u32], target_width: usize, target_height: usize) {
    scale_in_bands(
        frame,
        target,
        target_width,
        target_height,
        band_count(target_height),
    );
}

/// Draws `frame` into `target`, as [`scale_into`] does, in `bands` bands of
/// rows at once.
fn scale_in_bands(
    frame: &RgbFrame,
    target: &mut [u32],
    target_width: usize,
    target_height: usize,
    bands: usize,
) {
    let Some(placement) = Placement::fit(frame.width, frame.height, target_width, target_height)
    else {
        target.fill(0);
        return;
    };
    let exact = (placement.width, placement.height) == (frame.width, frame.height);
    let (columns, rows) = if exact {
        (Vec::new(), Vec::new())
    } else {
        (
            samples(frame.width, placement.width),
            samples(frame.height, placement.height),
        )
    };
    let picture = placement.left..placement.left + placement.width;
    in_bands(target, target_width, bands, |first_row, band| {
        // The two source rows each output row blends, scaled horizontally,
        // with the source row each holds: consecutive output rows mostly
        // share them.
        let mut upper = (usize::MAX, vec![0; if exact { 0 } else { placement.width }]);
        let mut lower = (usize::MAX, vec![0; if exact { 0 } else { placement.width }]);
        for (index, line) in band.chunks_exact_mut(target_width).enumerate() {
            let Some(picture_row) = (first_row + index)
                .checked_sub(placement.top)
                .filter(|row| *row < placement.height)
            else {
                line.fill(0);
                continue;
            };
            // Only the bars beside the picture are cleared.
            line[..picture.start].fill(0);
            line[picture.end..].fill(0);
            let destination = &mut line[picture.clone()];
            if exact {
                let start = picture_row * frame.width;
                destination.copy_from_slice(&frame.pixels[start..start + frame.width]);
                continue;
            }
            let sample = rows[picture_row];
            if upper.0 != sample.first {
                if lower.0 == sample.first {
                    std::mem::swap(&mut upper, &mut lower);
                } else {
                    scale_row(frame, sample.first, &columns, &mut upper.1);
                    upper.0 = sample.first;
                }
            }
            if lower.0 != sample.second {
                scale_row(frame, sample.second, &columns, &mut lower.1);
                lower.0 = sample.second;
            }
            if sample.weight == 0 {
                destination.copy_from_slice(&upper.1);
                continue;
            }
            for ((pixel, &top), &bottom) in destination.iter_mut().zip(&upper.1).zip(&lower.1) {
                *pixel = blend(top, bottom, sample.weight);
            }
        }
    });
}

/// Where a scaled pixel samples the source along one axis: the two source
/// pixels about its centre, and the weight of the second, out of
/// [`WHOLE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sample {
    /// The source pixel at or before the centre.
    first: usize,
    /// The source pixel after it, or the same one at the edge.
    second: usize,
    /// How much of the second pixel the scaled one takes.
    weight: u32,
}

/// Returns where each of `scaled` pixels samples `source` pixels along one
/// axis, with the pixels' centres aligned. Both counts are above zero.
fn samples(source: usize, scaled: usize) -> Vec<Sample> {
    (0..scaled)
        .map(|index| {
            // The centre of scaled pixel `index`, in source pixels:
            // (index + ½) × source ÷ scaled − ½, in 1/WHOLE of a pixel.
            let centre =
                ((2 * index + 1) * source).saturating_sub(scaled) * WHOLE as usize / (2 * scaled);
            let first = (centre >> WEIGHT_BITS).min(source - 1);
            Sample {
                first,
                second: (first + 1).min(source - 1),
                weight: (centre % WHOLE as usize) as u32,
            }
        })
        .collect()
}

/// Scales source row `row` of `frame` horizontally into `scaled`, one pixel
/// for each of `columns`.
fn scale_row(frame: &RgbFrame, row: usize, columns: &[Sample], scaled: &mut [u32]) {
    let source = &frame.pixels[row * frame.width..(row + 1) * frame.width];
    for (pixel, sample) in scaled.iter_mut().zip(columns) {
        *pixel = blend(source[sample.first], source[sample.second], sample.weight);
    }
}

/// Returns `first` and `second` blended, with `weight` out of [`WHOLE`] of
/// `second`. Red and blue are blended together, in the bytes that keep them
/// apart.
fn blend(first: u32, second: u32, weight: u32) -> u32 {
    let keep = WHOLE - weight;
    let red_blue =
        (((first & RED_BLUE) * keep + (second & RED_BLUE) * weight) >> WEIGHT_BITS) & RED_BLUE;
    let green = (((first & GREEN) * keep + (second & GREEN) * weight) >> WEIGHT_BITS) & GREEN;
    red_blue | green
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a 2 × 2 frame of one colour.
    fn solid(y: u8, u: u8, v: u8) -> Yuv420Frame {
        Yuv420Frame::from_planes(2, 2, vec![y; 4], vec![u], vec![v]).unwrap()
    }

    /// Splits a pixel into its red, green and blue components.
    fn rgb(pixel: u32) -> (u32, u32, u32) {
        ((pixel >> 16) & 0xff, (pixel >> 8) & 0xff, pixel & 0xff)
    }

    /// Limited-range black, white and the BT.709 primaries convert correctly.
    #[test]
    fn converts_reference_colours() {
        assert_eq!(
            rgb(yuv420_to_rgb(&solid(16, 128, 128)).pixels[0]),
            (0, 0, 0)
        );
        assert_eq!(
            rgb(yuv420_to_rgb(&solid(235, 128, 128)).pixels[0]),
            (255, 255, 255)
        );
        let near = |actual: u32, expected: u32| actual.abs_diff(expected) <= 2;
        // BT.709 limited-range red, green and blue.
        for ((y, u, v), (r, g, b)) in [
            ((63, 102, 240), (255, 0, 0)),
            ((173, 42, 26), (0, 255, 0)),
            ((32, 240, 118), (0, 0, 255)),
        ] {
            let (red, green, blue) = rgb(yuv420_to_rgb(&solid(y, u, v)).pixels[0]);
            assert!(
                near(red, r) && near(green, g) && near(blue, b),
                "{red} {green} {blue}"
            );
        }
    }

    /// Scaling keeps the aspect ratio and centres the image, blending
    /// neighbouring pixels.
    #[test]
    fn scales_with_letterboxing() {
        let frame = RgbFrame {
            width: 2,
            height: 1,
            pixels: vec![0x00ff_0000, 0x0000_00ff],
        };
        let mut target = vec![1; 4 * 4];
        scale_into(&frame, &mut target, 4, 4);
        // A 2:1 image in a square: two rows of picture between bars. The
        // middle pixels' centres lie a quarter and three quarters of the
        // way from red to blue.
        let row = [0x00ff_0000, 0x00bf_003f, 0x003f_00bf, 0x0000_00ff];
        assert_eq!(&target[0..4], [0; 4]);
        assert_eq!(&target[4..8], row);
        assert_eq!(&target[8..12], row);
        assert_eq!(&target[12..16], [0; 4]);
    }

    /// A picture of the window's size is copied as it is, and scaling
    /// keeps a uniform picture uniform.
    #[test]
    fn copies_and_scales_exactly() {
        let frame = RgbFrame {
            width: 3,
            height: 2,
            pixels: vec![1, 2, 3, 4, 5, 6],
        };
        let mut target = vec![9; 6];
        scale_into(&frame, &mut target, 3, 2);
        assert_eq!(target, frame.pixels);

        let grey = RgbFrame {
            width: 7,
            height: 5,
            pixels: vec![0x0080_8080; 35],
        };
        let mut target = vec![0; 13 * 9];
        scale_into(&grey, &mut target, 13, 9);
        let placement = Placement::fit(7, 5, 13, 9).unwrap();
        for row in placement.top..placement.top + placement.height {
            let start = row * 13 + placement.left;
            assert!(
                target[start..start + placement.width]
                    .iter()
                    .all(|&pixel| pixel == 0x0080_8080)
            );
        }
        assert_eq!(blend(0x00ff_ffff, 0, WHOLE), 0);
        assert_eq!(blend(0x00ff_ffff, 0, 0), 0x00ff_ffff);
    }

    /// Window positions map back to picture pixels, clamped to the picture.
    #[test]
    fn maps_window_positions() {
        // A 1280 × 720 picture in a 1000 × 1000 window: 1000 × 562, 219 down.
        let placement = Placement::fit(1280, 720, 1000, 1000).unwrap();
        assert_eq!(
            (
                placement.left,
                placement.top,
                placement.width,
                placement.height
            ),
            (0, 219, 1000, 562)
        );
        let (x, y) = placement.to_frame(500.0, 219.0 + 281.0, 1280, 720);
        assert!(
            (x - 640.0).abs() < 0.01 && (y - 360.0).abs() < 1.0,
            "{x} {y}"
        );
        assert_eq!(placement.to_frame(-5.0, 2000.0, 1280, 720), (0.0, 719.0));
        assert_eq!(Placement::fit(0, 10, 10, 10), None);
    }

    /// Empty sizes leave a black buffer and do not panic.
    #[test]
    fn maps_picture_positions() {
        let placement = Placement::fit(100, 50, 400, 400).unwrap();
        assert_eq!(placement.scale(100), 4.0);
        assert_eq!(placement.to_window(50.0, 25.0, 100, 50), (200.0, 200.0));
        assert!(placement.contains(200.0, 200.0));
        assert!(!placement.contains(200.0, 50.0));
    }

    /// Drawing in bands gives exactly what drawing in one does, for odd
    /// sizes whose rows do not split evenly.
    #[test]
    fn draws_the_same_in_bands() {
        let (width, height) = (334, 202);
        // A varied picture from a simple generator.
        let mut seed = 7_u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) as u8
        };
        let chroma = (width / 2) * (height / 2);
        let frame = Yuv420Frame::from_planes(
            width,
            height,
            (0..width * height).map(|_| next()).collect(),
            (0..chroma).map(|_| next()).collect(),
            (0..chroma).map(|_| next()).collect(),
        )
        .unwrap();
        let rgb = convert_in_bands(&frame, 1);
        assert_eq!(convert_in_bands(&frame, 3), rgb);
        assert_eq!(convert_in_bands(&frame, 4), rgb);
        for (target_width, target_height) in [(517, 389), (334, 202), (200, 640)] {
            let draw = |bands| {
                let mut target = vec![5; target_width * target_height];
                scale_in_bands(&rgb, &mut target, target_width, target_height, bands);
                target
            };
            let whole = draw(1);
            assert_eq!(draw(3), whole, "{target_width}x{target_height}");
            assert_eq!(draw(4), whole, "{target_width}x{target_height}");
        }
    }

    /// Empty pictures and windows have no placement.
    #[test]
    fn handles_empty_sizes() {
        let frame = RgbFrame {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        };
        let mut target = vec![7; 4];
        scale_into(&frame, &mut target, 2, 2);
        assert_eq!(target, [0; 4]);
    }
}
