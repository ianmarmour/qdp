//! The host's cursor images, scaled to the window's picture and drawn into
//! it when the window cannot use them as its pointer.

use std::sync::Arc;

use qdp::message::CursorImage;

/// The bytes per RGBA pixel.
const RGBA_PIXEL: usize = 4;

/// The largest side of a scaled cursor, in pixels, which window systems
/// accept.
const MAX_SIDE: usize = 1024;

/// The largest value of an 8-bit channel.
const CHANNEL_MAX: f64 = 255.0;

/// A cursor image at the size it is drawn in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaledCursor {
    /// The width in pixels.
    pub width: usize,
    /// The height in pixels.
    pub height: usize,
    /// The hotspot's column and row.
    pub hotspot: (usize, usize),
    /// RGBA pixels with straight alpha, row by row, shared with the drawing
    /// that shows them.
    pub rgba: Arc<[u8]>,
}

impl ScaledCursor {
    /// Scales a cursor image by `factor`, the window pixels per host pixel,
    /// so that it keeps its size relative to the picture (Section 9.2).
    /// Colours are interpolated weighted by alpha, so that transparent
    /// pixels leave no fringe.
    pub fn new(image: &CursorImage, factor: f64) -> Self {
        let (source_width, source_height) = (image.width as usize, image.height as usize);
        let side = |source: usize| ((source as f64 * factor).round() as usize).clamp(1, MAX_SIDE);
        let (width, height) = (side(source_width), side(source_height));
        let sample = |x: usize, y: usize| -> [f64; 4] {
            let start = (y * source_width + x) * RGBA_PIXEL;
            let pixel = &image.pixels[start..start + RGBA_PIXEL];
            let alpha = f64::from(pixel[3]) / CHANNEL_MAX;
            [
                f64::from(pixel[0]) * alpha,
                f64::from(pixel[1]) * alpha,
                f64::from(pixel[2]) * alpha,
                alpha,
            ]
        };
        let mut rgba = Vec::with_capacity(width * height * RGBA_PIXEL);
        for row in 0..height {
            // The centre of the output pixel, in source coordinates.
            let y = ((row as f64 + 0.5) * source_height as f64 / height as f64 - 0.5)
                .clamp(0.0, (source_height - 1) as f64);
            let (top, bottom) = (
                y.floor() as usize,
                (y.ceil() as usize).min(source_height - 1),
            );
            let vertical = y - top as f64;
            for column in 0..width {
                let x = ((column as f64 + 0.5) * source_width as f64 / width as f64 - 0.5)
                    .clamp(0.0, (source_width - 1) as f64);
                let (left, right) = (
                    x.floor() as usize,
                    (x.ceil() as usize).min(source_width - 1),
                );
                let horizontal = x - left as f64;
                let mut mixed = [0.0; 4];
                for (sample_x, sample_y, weight) in [
                    (left, top, (1.0 - horizontal) * (1.0 - vertical)),
                    (right, top, horizontal * (1.0 - vertical)),
                    (left, bottom, (1.0 - horizontal) * vertical),
                    (right, bottom, horizontal * vertical),
                ] {
                    for (total, value) in mixed.iter_mut().zip(sample(sample_x, sample_y)) {
                        *total += value * weight;
                    }
                }
                let alpha = mixed[3];
                for channel in &mixed[..3] {
                    let straight = if alpha > 0.0 { channel / alpha } else { 0.0 };
                    rgba.push(straight.round().clamp(0.0, CHANNEL_MAX) as u8);
                }
                rgba.push((alpha * CHANNEL_MAX).round() as u8);
            }
        }
        let hotspot = |position: u32, source: usize, size: usize| {
            ((f64::from(position) * size as f64 / source as f64) as usize).min(size - 1)
        };
        Self {
            width,
            height,
            hotspot: (
                hotspot(image.hotspot_x, source_width, width),
                hotspot(image.hotspot_y, source_height, height),
            ),
            rgba: rgba.into(),
        }
    }

    /// Draws the cursor into a buffer of `0x00RRGGBB` pixels, `width` wide,
    /// with its hotspot at `(x, y)`, clipped to the buffer.
    pub fn draw_into(&self, buffer: &mut [u32], width: usize, (x, y): (f64, f64)) {
        let height = buffer.len() / width.max(1);
        let left = x.round() as i64 - self.hotspot.0 as i64;
        let top = y.round() as i64 - self.hotspot.1 as i64;
        for row in 0..self.height {
            let Some(target_row) = offset(top, row, height) else {
                continue;
            };
            for column in 0..self.width {
                let Some(target_column) = offset(left, column, width) else {
                    continue;
                };
                let start = (row * self.width + column) * RGBA_PIXEL;
                let source = &self.rgba[start..start + RGBA_PIXEL];
                let alpha = u32::from(source[3]);
                let target = &mut buffer[target_row * width + target_column];
                let mut blended = 0;
                for (index, shift) in [16, 8, 0].into_iter().enumerate() {
                    let under = (*target >> shift) & 0xff;
                    let over = u32::from(source[index]);
                    let channel = (over * alpha + under * (255 - alpha) + 127) / 255;
                    blended |= channel << shift;
                }
                *target = blended;
            }
        }
    }
}

/// Returns `start + index` if it is inside `0..limit`.
fn offset(start: i64, index: usize, limit: usize) -> Option<usize> {
    usize::try_from(start + index as i64)
        .ok()
        .filter(|&position| position < limit)
}

#[cfg(test)]
mod tests {
    use qdp::message::{CursorId, ImageSerial};
    use qdp::wire::VarInt;

    use super::*;

    /// Returns a `width × height` image of one RGBA colour.
    fn solid(width: u32, height: u32, rgba: [u8; 4]) -> CursorImage {
        CursorImage {
            cursor_id: CursorId(VarInt::from_u32(1)),
            image_serial: ImageSerial(VarInt::from_u32(1)),
            width,
            height,
            hotspot_x: width - 1,
            hotspot_y: 0,
            scale: VarInt::from_u32(100),
            pixels: rgba.repeat((width * height) as usize),
        }
    }

    /// Scaling changes the size and hotspot and keeps a solid colour.
    #[test]
    fn scales_images() {
        let image = solid(4, 2, [10, 20, 30, 255]);
        let scaled = ScaledCursor::new(&image, 2.0);
        assert_eq!((scaled.width, scaled.height), (8, 4));
        assert_eq!(scaled.hotspot, (6, 0));
        assert!(
            scaled
                .rgba
                .chunks_exact(4)
                .all(|pixel| pixel == [10, 20, 30, 255])
        );
        let small = ScaledCursor::new(&image, 0.1);
        assert_eq!((small.width, small.height), (1, 1));
        assert_eq!(small.hotspot, (0, 0));
    }

    /// Transparent pixels add no colour to their neighbours.
    #[test]
    fn leaves_no_fringes() {
        let mut image = solid(2, 1, [255, 255, 255, 255]);
        image.pixels[4..].copy_from_slice(&[0, 0, 0, 0]);
        let scaled = ScaledCursor::new(&image, 1.5);
        for pixel in scaled.rgba.chunks_exact(4).filter(|pixel| pixel[3] > 0) {
            assert_eq!(&pixel[..3], [255, 255, 255]);
        }
    }

    /// Drawing blends by alpha with the hotspot at the position, clipped.
    #[test]
    fn draws_into_buffers() {
        let opaque = ScaledCursor::new(&solid(2, 2, [255, 0, 0, 255]), 1.0);
        let mut buffer = vec![0x0000_00ff; 9];
        // The hotspot is the top-right pixel.
        opaque.draw_into(&mut buffer, 3, (1.0, 1.0));
        assert_eq!(
            buffer,
            [
                0xff, 0xff, 0xff, //
                0xff_0000, 0xff_0000, 0xff, //
                0xff_0000, 0xff_0000, 0xff,
            ]
        );
        let half = ScaledCursor::new(&solid(1, 1, [255, 255, 255, 128]), 1.0);
        let mut buffer = vec![0; 1];
        half.draw_into(&mut buffer, 1, (0.0, 0.0));
        assert_eq!(buffer, [0x80_8080]);
        half.draw_into(&mut buffer, 1, (-5.0, 7.0));
        assert_eq!(buffer, [0x80_8080]);
    }
}
