//! Uncompressed video frames.

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

/// The order of the colour bytes of a 32-bit pixel; the fourth byte, alpha
/// or padding, is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rgb32Order {
    /// Blue, green, red, then padding: `BGRx` or `BGRA`.
    Bgr,
    /// Red, green, blue, then padding: `RGBx` or `RGBA`.
    Rgb,
}

/// Applies 16-bit fixed-point weights to red, green and blue, with rounding.
fn weigh(weights: [i32; 3], [red, green, blue]: [i32; 3]) -> i32 {
    (weights[0] * red + weights[1] * green + weights[2] * blue + (1 << (FRACTION_BITS - 1)))
        >> FRACTION_BITS
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
        let pixel = |x: usize, y: usize| -> [i32; 3] {
            let start = y * stride + x * RGB32_PIXEL;
            let bytes = &data[start..start + 3];
            let [first, second, third] = [bytes[0], bytes[1], bytes[2]].map(i32::from);
            match order {
                Rgb32Order::Bgr => [third, second, first],
                Rgb32Order::Rgb => [first, second, third],
            }
        };
        let (chroma_width, chroma_height) = chroma_size(width, height);
        let mut frame = Self::black(width, height);
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
