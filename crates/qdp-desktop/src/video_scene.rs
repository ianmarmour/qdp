//! What a client's video presenter draws: the picture's place, and an image
//! over it, such as the host's cursor; and what became of each picture
//! handed to it, which the session's frame timing reports.

use std::sync::Arc;
use std::time::{Duration, Instant};

/// A rectangle in the drawable's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelRect {
    /// Its left edge.
    pub left: f64,
    /// Its top edge.
    pub top: f64,
    /// Its width.
    pub width: f64,
    /// Its height.
    pub height: f64,
}

/// An image drawn over the picture, such as the host's cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    /// Its width in pixels.
    pub width: usize,
    /// Its height in pixels.
    pub height: usize,
    /// Its RGBA pixels with straight alpha, row by row.
    pub rgba: Arc<[u8]>,
    /// Its top-left corner in the drawable's pixels.
    pub left: f64,
    /// Its top edge.
    pub top: f64,
}

/// What to draw besides the picture, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// The drawable's width and height in pixels.
    pub size: (u32, u32),
    /// Where the picture goes, if one is shown.
    pub picture: Option<PixelRect>,
    /// What is drawn over the picture, if anything, such as the host's
    /// cursor.
    pub overlay: Option<Overlay>,
    /// What is drawn over that, if anything, such as the stream's
    /// statistics.
    pub badge: Option<Overlay>,
}

/// The layers drawn over the picture: the overlay, then the badge.
pub(crate) const OVERLAY_LAYERS: usize = 2;

impl Scene {
    /// Returns what is drawn over the picture, bottom first.
    pub(crate) fn layers(&self) -> [Option<&Overlay>; OVERLAY_LAYERS] {
        [self.overlay.as_ref(), self.badge.as_ref()]
    }
}

/// What became of a picture handed to the render thread, tagged as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate<T> {
    /// It was drawn and handed to the display.
    Shown {
        /// Its tag.
        tag: T,
        /// When it was handed to the display.
        at: Instant,
        /// How long drawing it took, once a drawable was free.
        drew_in: Duration,
        /// How long the render thread waited for a free drawable.
        waited: Duration,
    },
    /// A newer picture replaced it before it was drawn, or it could not be.
    Unshown(T),
}
