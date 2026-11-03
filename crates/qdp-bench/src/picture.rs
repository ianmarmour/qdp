//! The client's screen's frames, read a pixel at a time: in memory, or in a
//! buffer of the GPU's, mapped, where only the pixels read are fetched, so
//! that reading a frame takes a fraction of the screen's refresh.

use qdp_codec::{DRM_FORMAT_MOD_LINEAR, DmaBufFrame, PixelDepth, Rgb32Order, Yuv420Frame};
use qdp_desktop::MappedDmaBuf;

use crate::layout::Picture;

impl Picture for Yuv420Frame {
    fn size(&self) -> (usize, usize) {
        (self.width(), self.height())
    }

    fn luma(&self, x: usize, y: usize) -> u8 {
        self.y()[y * self.width() + x]
    }

    fn chroma(&self, x: usize, y: usize) -> (u8, u8) {
        let index = y / 2 * self.width().div_ceil(2) + x / 2;
        (self.u()[index], self.v()[index])
    }
}

/// The bytes of a pixel in a frame the GPU holds.
const PIXEL_BYTES: usize = 4;

/// A frame of 8-bit colours in a linear buffer of the GPU's, mapped for
/// reading.
pub struct MappedFrame<'a> {
    /// The buffer, mapped.
    mapped: MappedDmaBuf<'a>,
    /// The width and height, in pixels.
    size: (usize, usize),
    /// The offset of the first row, in bytes.
    offset: usize,
    /// The bytes from one row to the next.
    pitch: usize,
    /// Where red, green and blue lie in each pixel's bytes.
    channels: [usize; 3],
}

impl<'a> MappedFrame<'a> {
    /// Maps `frame`, or returns `None` if it is not all of a linear buffer
    /// of 8-bit colours that the processor can map.
    pub fn map(frame: &'a DmaBufFrame) -> Option<Self> {
        let layout = frame.layout();
        if layout.modifier != DRM_FORMAT_MOD_LINEAR
            || frame.depth() != PixelDepth::Eight
            || !frame.is_whole_buffer()
        {
            return None;
        }
        let (offset, pitch) = (layout.offset as usize, layout.pitch as usize);
        let end = offset + pitch * (layout.height - 1) + layout.width * PIXEL_BYTES;
        if end > usize::try_from(frame.size()).ok()? {
            return None;
        }
        let mapped = MappedDmaBuf::map(frame.descriptor(), end)?;
        let channels = match layout.order {
            Rgb32Order::Bgr => [2, 1, 0],
            Rgb32Order::Rgb => [0, 1, 2],
        };
        Some(Self {
            mapped,
            size: (layout.width, layout.height),
            offset,
            pitch,
            channels,
        })
    }

    /// Returns the red, green and blue of the pixel at `x`, `y`, read at
    /// once, as each read of the GPU's memory is slow.
    fn rgb(&self, x: usize, y: usize) -> [i32; 3] {
        let start = self.offset + y * self.pitch + x * PIXEL_BYTES;
        let mut pixel = [0; PIXEL_BYTES];
        pixel.copy_from_slice(&self.mapped.bytes()[start..start + PIXEL_BYTES]);
        self.channels.map(|channel| i32::from(pixel[channel]))
    }
}

impl Picture for MappedFrame<'_> {
    fn size(&self) -> (usize, usize) {
        self.size
    }

    fn luma(&self, x: usize, y: usize) -> u8 {
        to_yuv(self.rgb(x, y)).0
    }

    fn chroma(&self, x: usize, y: usize) -> (u8, u8) {
        let (_, blue, red) = to_yuv(self.rgb(x, y));
        (blue, red)
    }
}

/// Returns the luma and the blue and red differences of a red, green and
/// blue, by BT.709's weights in 256ths, at full range.
pub(crate) fn to_yuv([red, green, blue]: [i32; 3]) -> (u8, u8, u8) {
    let byte = |value: i32| value.clamp(0, i32::from(u8::MAX)) as u8;
    (
        byte((54 * red + 183 * green + 19 * blue) >> 8),
        byte(128 + ((-29 * red - 99 * green + 128 * blue) >> 8)),
        byte(128 + ((128 * red - 116 * green - 12 * blue) >> 8)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The colours the probe draws read as it expects: green as green, and
    /// black and white on either side of the luma threshold.
    #[test]
    fn converts_the_probe_colours() {
        let (_, blue, red) = to_yuv([0, 255, 0]);
        assert!(blue < 90 && red < 90, "{blue} {red}");
        assert_eq!(to_yuv([0, 0, 0]), (0, 128, 128));
        let (luma, blue, red) = to_yuv([255, 255, 255]);
        assert!(luma > 250 && blue.abs_diff(128) <= 1 && red.abs_diff(128) <= 1);
    }
}
