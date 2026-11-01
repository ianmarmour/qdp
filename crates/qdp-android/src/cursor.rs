//! The host's cursor image, as the app draws it over the video.

use qdp::message::CursorImage;

/// The values before the pixels in [`CursorBitmap::to_ints`]: the width,
/// the height, and the hotspot's column and row.
const HEADER_INTS: usize = 4;

/// A cursor image as Android's `Bitmap.createBitmap(int[], …)` takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorBitmap {
    /// The width in pixels.
    width: u32,
    /// The height in pixels.
    height: u32,
    /// The hotspot's column and row.
    hotspot: (u32, u32),
    /// The pixels, row by row from the top, each `0xAARRGGBB` with straight
    /// alpha.
    argb: Vec<i32>,
}

impl CursorBitmap {
    /// Converts the host's image, whose pixels are RGBA bytes.
    pub fn new(image: &CursorImage) -> Self {
        let argb = image
            .pixels
            .chunks_exact(4)
            .map(|pixel| i32::from_be_bytes([pixel[3], pixel[0], pixel[1], pixel[2]]))
            .collect();
        Self {
            width: image.width,
            height: image.height,
            hotspot: (image.hotspot_x, image.hotspot_y),
            argb,
        }
    }

    /// Returns the image as the app reads it: its width, height, hotspot
    /// column and hotspot row, then its pixels.
    pub fn to_ints(&self) -> Vec<i32> {
        let header = [self.width, self.height, self.hotspot.0, self.hotspot.1]
            .map(|value| i32::try_from(value).unwrap_or(i32::MAX));
        let mut ints = Vec::with_capacity(HEADER_INTS + self.argb.len());
        ints.extend_from_slice(&header);
        ints.extend_from_slice(&self.argb);
        ints
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use qdp::message::{CursorId, ImageSerial};
    use qdp::wire::VarInt;

    /// RGBA pixels become ARGB colours after the image's size and hotspot.
    #[test]
    fn converts_images() {
        let image = CursorImage {
            cursor_id: CursorId(VarInt::from_u32(1)),
            image_serial: ImageSerial(VarInt::from_u32(1)),
            width: 2,
            height: 1,
            hotspot_x: 1,
            hotspot_y: 0,
            scale: VarInt::from_u32(100),
            pixels: vec![0xFF, 0x00, 0x00, 0x80, 0x01, 0x02, 0x03, 0xFF],
        };
        assert_eq!(
            CursorBitmap::new(&image).to_ints(),
            vec![2, 1, 1, 0, 0x80FF_0000_u32 as i32, 0xFF01_0203_u32 as i32]
        );
    }
}
