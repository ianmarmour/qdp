//! The macOS pointer, for the client to draw (Section 9.1): its image from
//! AppKit and its position from Quartz.
//!
//! AppKit reports the system-wide pointer through `currentSystemCursor`,
//! which Apple has deprecated and says will return nothing in a future
//! macOS. Once it returns nothing, the client shows a stock arrow: the host
//! never draws the pointer into its frames.

use std::ptr;
use std::sync::Arc;

use objc2::rc::autoreleasepool;
use objc2_app_kit::NSCursor;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDisplayBounds, CGEvent, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGMainDisplayID,
};
use qdp_runtime::{CursorShape, CursorState, PointerPosition};

/// The bytes per RGBA pixel.
const RGBA_PIXEL: usize = 4;

/// The bits per colour component.
const BITS_PER_COMPONENT: usize = 8;

/// The largest value of an 8-bit channel.
const CHANNEL_MAX: u32 = 255;

/// The largest pointer side, in pixels, read from AppKit.
const MAX_SIDE: usize = 512;

/// Returns the system-wide pointer, if macOS still reports it.
#[allow(deprecated)]
fn system_cursor() -> Option<objc2::rc::Retained<NSCursor>> {
    // Deprecated, but the only way to read another app's pointer image
    // while leaving it out of the captured frames; see the module comment.
    NSCursor::currentSystemCursor()
}

/// Reads the main display's pointer.
#[derive(Debug)]
pub(super) struct SystemCursor {
    /// The main display's area in global points.
    bounds: CGRect,
    /// Captured pixels per point, across and down.
    scale: (f64, f64),
    /// The image read last, kept while the pointer keeps its look.
    shape: Option<Arc<CursorShape>>,
}

impl SystemCursor {
    /// Returns a reader for a main display captured at `width × height`
    /// pixels.
    pub(super) fn new(width: usize, height: usize) -> Self {
        let bounds = CGDisplayBounds(CGMainDisplayID());
        Self {
            bounds,
            scale: (
                width as f64 / bounds.size.width.max(1.0),
                height as f64 / bounds.size.height.max(1.0),
            ),
            shape: None,
        }
    }

    /// Returns the pointer now: its hotspot's position in captured pixels
    /// and its image at the captured pixel density.
    pub(super) fn state(&mut self) -> CursorState {
        let shape = autoreleasepool(|_| self.read_shape()).or_else(|| Some(CursorShape::arrow()));
        let shape = match (shape, &self.shape) {
            (Some(new), Some(old)) if new == **old => Some(Arc::clone(old)),
            (new, _) => new.map(Arc::new),
        };
        self.shape = shape.clone();
        let location = CGEvent::new(None).map(|event| CGEvent::location(Some(&event)));
        let position = location.and_then(|location| {
            let x = location.x - self.bounds.origin.x;
            let y = location.y - self.bounds.origin.y;
            let inside = (0.0..self.bounds.size.width).contains(&x)
                && (0.0..self.bounds.size.height).contains(&y);
            inside.then_some(((x * self.scale.0) as i32, (y * self.scale.1) as i32))
        });
        CursorState {
            position: position.map_or(PointerPosition::Hidden, |(x, y)| PointerPosition::At(x, y)),
            shape,
        }
    }

    /// Reads the pointer's image, drawn at the captured pixel density.
    fn read_shape(&self) -> Option<CursorShape> {
        let cursor = system_cursor()?;
        let image = cursor.image();
        let size = image.size();
        let side = |points: f64, scale: f64| ((points * scale).round() as usize).clamp(1, MAX_SIDE);
        let (width, height) = (
            side(size.width, self.scale.0),
            side(size.height, self.scale.1),
        );
        // SAFETY: a null rectangle asks for the image's own size, and no
        // context or hints are passed.
        let picture =
            unsafe { image.CGImageForProposedRect_context_hints(ptr::null_mut(), None, None) }?;
        let mut pixels = vec![0_u8; width * height * RGBA_PIXEL];
        let colour_space = CGColorSpace::new_device_rgb()?;
        let layout = CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0;
        // SAFETY: the buffer holds `height` rows of `width` RGBA pixels and
        // outlives the context, which is released at the end of this scope.
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                BITS_PER_COMPONENT,
                width * RGBA_PIXEL,
                Some(&colour_space),
                layout,
            )
        }?;
        let area = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: width as f64,
                height: height as f64,
            },
        };
        CGContext::draw_image(Some(&context), area, Some(&picture));
        drop(context);
        straighten(&mut pixels);
        let hotspot = cursor.hotSpot();
        let pixel = |points: f64, scale: f64, size: usize| {
            ((points * scale).max(0.0) as u32).min(size as u32 - 1)
        };
        Some(CursorShape {
            width: width as u32,
            height: height as u32,
            hotspot: (
                pixel(hotspot.x, self.scale.0, width),
                pixel(hotspot.y, self.scale.1, height),
            ),
            pixels,
        })
    }
}

/// Converts premultiplied RGBA pixels to straight alpha.
fn straighten(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(RGBA_PIXEL) {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            // A fully transparent pixel has no colour.
            *channel = (u32::from(*channel) * CHANNEL_MAX)
                .checked_div(alpha)
                .map_or(0, |straight| straight.min(CHANNEL_MAX) as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Premultiplied colours are divided by their alpha.
    #[test]
    fn straightens_alpha() {
        let mut pixels = [128, 64, 0, 128, 10, 10, 10, 0, 255, 0, 0, 255];
        straighten(&mut pixels);
        assert_eq!(pixels, [255, 127, 0, 128, 0, 0, 0, 0, 255, 0, 0, 255]);
    }
}
