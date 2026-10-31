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
