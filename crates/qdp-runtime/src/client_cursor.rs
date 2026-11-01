//! The client's side of the cursor (Section 9): the images the host sent,
//! and which one to draw.

use std::collections::HashMap;

use qdp::message::{CursorId, CursorImage, CursorSet, ImageSerial, MAX_CURSOR_IDS};

/// What the client should draw after a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CursorChange<'a> {
    /// Hide the pointer.
    Hidden,
    /// Draw this image at the pointer.
    Shown(&'a CursorImage),
}

/// The host's cursor images and the one it means.
#[derive(Debug)]
pub(crate) struct ClientCursor {
    /// The newest image of each Cursor ID.
    images: HashMap<CursorId, CursorImage>,
    /// The host's latest `CURSOR_SET`.
    current: CursorSet,
}

impl Default for ClientCursor {
    fn default() -> Self {
        Self {
            images: HashMap::new(),
            current: CursorSet {
                cursor_id: CursorId::HIDDEN,
                image_serial: ImageSerial::default(),
            },
        }
    }
}

impl ClientCursor {
    /// Keeps a cursor image, unless a newer one for its Cursor ID has
    /// arrived, and returns what to draw if the image is the one the host
    /// means.
    pub(crate) fn image(&mut self, image: CursorImage) -> Option<CursorChange<'_>> {
        if self
            .images
            .get(&image.cursor_id)
            .is_some_and(|cached| cached.image_serial > image.image_serial)
        {
            return None;
        }
        // The host never defines more than 128 Cursor IDs; one that does
        // loses its oldest image rather than grow the cache.
        if !self.images.contains_key(&image.cursor_id) && self.images.len() >= MAX_CURSOR_IDS {
            if let Some(oldest) = self
                .images
                .values()
                .min_by_key(|cached| cached.image_serial)
                .map(|cached| cached.cursor_id)
            {
                self.images.remove(&oldest);
            }
        }
        let id = image.cursor_id;
        let means_this =
            self.current.cursor_id == id && self.current.image_serial == image.image_serial;
        self.images.insert(id, image);
        means_this.then(|| CursorChange::Shown(&self.images[&id]))
    }

    /// Records the host's `CURSOR_SET` and returns what to draw, or `None`
    /// to keep drawing the previous cursor until the image arrives.
    pub(crate) fn set(&mut self, set: CursorSet) -> Option<CursorChange<'_>> {
        self.current = set;
        self.shown()
    }

    /// Returns what the host's latest `CURSOR_SET` means to draw, or `None`
    /// while its image has not arrived.
    pub(crate) fn shown(&self) -> Option<CursorChange<'_>> {
        if self.current.cursor_id == CursorId::HIDDEN {
            return Some(CursorChange::Hidden);
        }
        self.images
            .get(&self.current.cursor_id)
            .filter(|image| image.image_serial == self.current.image_serial)
            .map(CursorChange::Shown)
    }
}

#[cfg(test)]
mod tests {
    use qdp::wire::VarInt;

    use super::*;

    /// Returns a 1 × 1 image for a Cursor ID and serial.
    fn image(id: u32, serial: u32) -> CursorImage {
        CursorImage {
            cursor_id: CursorId(VarInt::from_u32(id)),
            image_serial: ImageSerial(VarInt::from_u32(serial)),
            width: 1,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            scale: VarInt::from_u32(100),
            pixels: vec![serial as u8; 4],
        }
    }

    /// Returns a `CURSOR_SET`.
    fn set(id: u32, serial: u32) -> CursorSet {
        CursorSet {
            cursor_id: CursorId(VarInt::from_u32(id)),
            image_serial: ImageSerial(VarInt::from_u32(serial)),
        }
    }

    /// The cursor changes once both the `CURSOR_SET` and its image have
    /// arrived, in either order.
    #[test]
    fn shows_an_image_once_both_arrive() {
        let mut cursor = ClientCursor::default();
        assert_eq!(cursor.set(set(1, 1)), None);
        assert_eq!(
            cursor.image(image(1, 1)),
            Some(CursorChange::Shown(&image(1, 1)))
        );
        assert_eq!(cursor.image(image(2, 2)), None);
        assert_eq!(
            cursor.set(set(2, 2)),
            Some(CursorChange::Shown(&image(2, 2)))
        );
        assert_eq!(cursor.set(set(0, 0)), Some(CursorChange::Hidden));
        assert_eq!(
            cursor.set(set(1, 1)),
            Some(CursorChange::Shown(&image(1, 1)))
        );
    }

    /// An image older than the cached one for its Cursor ID is discarded,
    /// and a replacement with the current serial redraws the cursor.
    #[test]
    fn keeps_the_newest_image() {
        let mut cursor = ClientCursor::default();
        assert_eq!(cursor.image(image(1, 3)), None);
        assert_eq!(cursor.image(image(1, 2)), None);
        assert_eq!(cursor.set(set(1, 2)), None, "the older image was discarded");
        assert_eq!(
            cursor.set(set(1, 3)),
            Some(CursorChange::Shown(&image(1, 3)))
        );
        assert_eq!(
            cursor.set(set(1, 4)),
            None,
            "the replacement is still on its way"
        );
        assert_eq!(
            cursor.image(image(1, 4)),
            Some(CursorChange::Shown(&image(1, 4)))
        );
    }

    /// The cache never holds more than 128 images.
    #[test]
    fn bounds_the_cache() {
        let mut cursor = ClientCursor::default();
        for id in 1..=MAX_CURSOR_IDS as u32 + 1 {
            cursor.image(image(id, id));
        }
        assert_eq!(cursor.images.len(), MAX_CURSOR_IDS);
        assert!(!cursor.images.contains_key(&CursorId(VarInt::from_u32(1))));
    }
}
