//! The host's side of the cursor (Section 9): which image the client draws,
//! sending each image once, and warping the client's cursor when the pointer
//! moves without the client's input.

use std::sync::Arc;
use std::time::{Duration, Instant};

use qdp::message::{
    CursorId, CursorImage, CursorSet, CursorWarp, ImageSerial, MAX_CURSOR_IDS, MAX_CURSOR_SIZE,
    WarpFlags, WarpId,
};
use qdp::wire::{SignedVarInt, VarInt};

use crate::host_input::InputAction;
use crate::media::{CursorShape, CursorState, PointerPosition};

/// QDP positions are in 1/256 pixel (Section 1.5.6).
const SUBUNITS: i64 = 256;

/// How long after the client's last pointer input the pointer must have been
/// still before a move is taken as the host's own: captured positions lag the
/// input that caused them by a frame or two.
const WARP_QUIET: Duration = Duration::from_millis(250);

/// How long after the last input of a device the client cannot follow, such
/// as a raw touchpad or a passthrough device, the pointer's moves are taken
/// as that device's: reported at once, with `FOLLOW` (Section 9.4).
const FOLLOW_WINDOW: Duration = Duration::from_millis(500);

/// How far, in pixels, the captured pointer may be from where the client's
/// input put it before the difference is taken as a move: injection rounds
/// positions to the host's own units.
const WARP_TOLERANCE: i64 = 2;

/// The largest cursor image side, in pixels.
const MAX_SIDE: u32 = MAX_CURSOR_SIZE as u32;

/// The bytes per RGBA pixel.
const RGBA_PIXEL: usize = 4;

/// What the host sends the client about its cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CursorUpdate {
    /// A cursor image, sent on a `CURSOR_IMAGE` stream.
    Image(CursorImage),
    /// A `CURSOR_SET` message.
    Set(CursorSet),
    /// A `CURSOR_WARP` message.
    Warp(CursorWarp),
}

/// One defined Cursor ID.
#[derive(Debug, Clone)]
struct Slot {
    /// The Cursor ID.
    id: CursorId,
    /// The serial of the image it holds.
    serial: ImageSerial,
    /// The image it holds.
    shape: Arc<CursorShape>,
    /// When the slot was last shown, on the cursor's update clock, for
    /// replacing the least recently used one.
    last_used: u64,
}

/// The host's cursor state.
#[derive(Debug)]
pub(crate) struct HostCursor {
    /// The defined Cursor IDs, at most [`MAX_CURSOR_IDS`].
    slots: Vec<Slot>,
    /// The serial of the last image sent.
    last_serial: u64,
    /// The `CURSOR_SET` last sent.
    current: CursorSet,
    /// Counts the updates that showed a cursor, ordering slots by last use.
    clock: u64,
    /// Where the client's input last put the pointer, in pixels, or `None`
    /// if it last moved the pointer relatively.
    expected: Option<(i64, i64)>,
    /// When the client's input last moved the pointer.
    last_input: Option<Instant>,
    /// The pointer's position in the last capture, in pixels.
    last_position: Option<(i64, i64)>,
    /// The Warp ID of the last `CURSOR_WARP`.
    last_warp: u64,
    /// When a device the client cannot follow last reported.
    last_unfollowed_input: Option<Instant>,
}

impl Default for HostCursor {
    fn default() -> Self {
        Self {
            slots: Vec::new(),
            last_serial: 0,
            current: CursorSet {
                cursor_id: CursorId::HIDDEN,
                image_serial: ImageSerial::default(),
            },
            clock: 0,
            expected: None,
            last_input: None,
            last_position: None,
            last_warp: 0,
            last_unfollowed_input: None,
        }
    }
}

impl HostCursor {
    /// Returns the Warp ID of the last `CURSOR_WARP`: positions the client
    /// computed before it are ignored (Section 8.7).
    pub(crate) fn latest_warp(&self) -> WarpId {
        WarpId(VarInt::new(self.last_warp).unwrap_or(VarInt::MAX))
    }

    /// Forgets the images sent, so that the next update sends the shown one
    /// again, with a new serial and so a new `CURSOR_SET`: a new layout
    /// changes the Scale images are sent with.
    pub(crate) fn forget_images(&mut self) {
        self.slots.clear();
    }

    /// Notes where the client's input moves the pointer.
    pub(crate) fn input(&mut self, action: &InputAction, now: Instant) {
        match *action {
            InputAction::PointerMoveTo { x, y } => {
                self.expected = Some((i64::from(x) / SUBUNITS, i64::from(y) / SUBUNITS));
                self.last_input = Some(now);
            }
            InputAction::MouseMoveBy { .. } => {
                self.expected = None;
                self.last_input = Some(now);
            }
            InputAction::Touchpad { .. } | InputAction::PassthroughReport { .. } => {
                self.last_unfollowed_input = Some(now);
            }
            _ => {}
        }
    }

    /// Returns what to send the client after a capture saw the pointer as
    /// `state`, in order: an image before the `CURSOR_SET` that shows it.
    pub(crate) fn update(&mut self, state: &CursorState, now: Instant) -> Vec<CursorUpdate> {
        let mut updates = Vec::new();
        let wanted = match (&state.shape, state.position) {
            (Some(_), PointerPosition::Hidden) | (None, _) => None,
            (Some(shape), _) => Some(self.slot_for(shape, &mut updates)),
        };
        let set = match wanted {
            Some(index) => {
                self.clock += 1;
                let slot = &mut self.slots[index];
                slot.last_used = self.clock;
                CursorSet {
                    cursor_id: slot.id,
                    image_serial: slot.serial,
                }
            }
            None => CursorSet {
                cursor_id: CursorId::HIDDEN,
                image_serial: ImageSerial::default(),
            },
        };
        if set != self.current {
            self.current = set;
            updates.push(CursorUpdate::Set(set));
        }
        let warp = match state.position {
            PointerPosition::At(x, y) => self.warp((x, y), now),
            PointerPosition::Hidden | PointerPosition::Unknown => None,
        };
        if let Some(warp) = warp {
            updates.push(CursorUpdate::Warp(warp));
        }
        updates
    }

    /// Returns the index of the slot holding `shape`, defining one, and
    /// queueing its image, if none does.
    fn slot_for(&mut self, shape: &Arc<CursorShape>, updates: &mut Vec<CursorUpdate>) -> usize {
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| Arc::ptr_eq(&slot.shape, shape) || *slot.shape == **shape)
        {
            return index;
        }
        self.last_serial += 1;
        let serial = ImageSerial(VarInt::new(self.last_serial).unwrap_or(VarInt::MAX));
        let index = if self.slots.len() < MAX_CURSOR_IDS {
            let id = CursorId(VarInt::try_from(self.slots.len() + 1).unwrap_or(VarInt::MAX));
            self.slots.push(Slot {
                id,
                serial,
                shape: Arc::clone(shape),
                last_used: 0,
            });
            self.slots.len() - 1
        } else {
            let index = self
                .slots
                .iter()
                .enumerate()
                .min_by_key(|(_, slot)| slot.last_used)
                .map_or(0, |(index, _)| index);
            let slot = &mut self.slots[index];
            slot.serial = serial;
            slot.shape = Arc::clone(shape);
            index
        };
        let slot = &self.slots[index];
        updates.push(CursorUpdate::Image(image(slot.id, serial, shape)));
        index
    }

    /// Returns a warp to where the pointer is now, as the host sends when it
    /// switches to absolute mode (Section 8.9), if a capture has seen it.
    /// Returns the warp the host sends on switching to absolute mode, which
    /// it must (Section 8.9): to where a capture last saw the pointer, or
    /// else where the client's input last put it, or else the desktop's
    /// origin.
    pub(crate) fn warp_to_current(&mut self) -> CursorWarp {
        let (x, y) = self.last_position.or(self.expected).unwrap_or_default();
        self.expected = Some((x, y));
        self.last_warp += 1;
        let subunits = |value: i64| SignedVarInt::new(value * SUBUNITS).unwrap_or_default();
        CursorWarp {
            warp_id: self.latest_warp(),
            flags: WarpFlags::default(),
            x: subunits(x),
            y: subunits(y),
        }
    }

    /// Returns a warp if the pointer moved to `position` without the
    /// client's input.
    fn warp(&mut self, (x, y): (i32, i32), now: Instant) -> Option<CursorWarp> {
        let position = (i64::from(x), i64::from(y));
        if self.last_position == Some(position) {
            return None;
        }
        self.last_position = Some(position);
        // A device the client cannot follow is moving the pointer: each
        // capture, once per display refresh, reports where it went.
        let following = self
            .last_unfollowed_input
            .is_some_and(|input| now.saturating_duration_since(input) < FOLLOW_WINDOW);
        let quiet = self
            .last_input
            .is_none_or(|input| now.saturating_duration_since(input) >= WARP_QUIET);
        let unexplained = self.expected.is_none_or(|(expected_x, expected_y)| {
            (expected_x - position.0).abs() > WARP_TOLERANCE
                || (expected_y - position.1).abs() > WARP_TOLERANCE
        });
        if !following && !(quiet && unexplained) {
            return None;
        }
        self.expected = Some(position);
        self.last_warp += 1;
        let subunits = |value: i64| SignedVarInt::new(value * SUBUNITS).unwrap_or_default();
        Some(CursorWarp {
            warp_id: self.latest_warp(),
            flags: if following {
                WarpFlags::FOLLOW
            } else {
                WarpFlags::default()
            },
            x: subunits(position.0),
            y: subunits(position.1),
        })
    }
}

/// Returns the cursor image of `shape`, cropped to the largest size QDP
/// allows.
fn image(cursor_id: CursorId, image_serial: ImageSerial, shape: &CursorShape) -> CursorImage {
    let width = shape.width.clamp(1, MAX_SIDE);
    let height = shape.height.clamp(1, MAX_SIDE);
    let mut pixels = Vec::with_capacity(width as usize * height as usize * RGBA_PIXEL);
    let source_row = shape.width as usize * RGBA_PIXEL;
    for row in 0..height as usize {
        let start = row * source_row;
        let end = start + width as usize * RGBA_PIXEL;
        match shape.pixels.get(start..end) {
            Some(line) => pixels.extend_from_slice(line),
            None => pixels.resize(pixels.len() + width as usize * RGBA_PIXEL, 0),
        }
    }
    CursorImage {
        cursor_id,
        image_serial,
        width,
        height,
        hotspot_x: shape.hotspot.0.min(width - 1),
        hotspot_y: shape.hotspot.1.min(height - 1),
        scale: VarInt::from_u32(crate::host::SCALE_PERCENT),
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::{DeviceId, TouchpadInfo, TouchpadReport};

    /// Returns a solid `size × size` shape of one colour.
    fn shape(size: u32, colour: u8) -> Arc<CursorShape> {
        Arc::new(CursorShape {
            width: size,
            height: size,
            hotspot: (1, 1),
            pixels: vec![colour; (size * size) as usize * RGBA_PIXEL],
        })
    }

    /// Returns a visible pointer at `position` with `shape`.
    fn visible(position: (i32, i32), shape: &Arc<CursorShape>) -> CursorState {
        CursorState {
            position: PointerPosition::At(position.0, position.1),
            shape: Some(Arc::clone(shape)),
        }
    }

    /// Returns the `CURSOR_SET`s among updates.
    fn sets(updates: &[CursorUpdate]) -> Vec<CursorSet> {
        updates
            .iter()
            .filter_map(|update| match update {
                CursorUpdate::Set(set) => Some(*set),
                _ => None,
            })
            .collect()
    }

    /// Each image is sent once, before the `CURSOR_SET` that shows it, and
    /// shapes seen before are shown again by their Cursor ID.
    #[test]
    fn sends_each_image_once() {
        let mut cursor = HostCursor::default();
        let now = Instant::now();
        let arrow = shape(8, 1);
        let updates = cursor.update(&visible((5, 5), &arrow), now);
        let CursorUpdate::Image(image) = &updates[0] else {
            panic!("the image comes first: {updates:?}");
        };
        assert_eq!((image.width, image.height, image.hotspot_x), (8, 8, 1));
        assert_eq!(image.image_serial.0.into_inner(), 1);
        let first = sets(&updates)[0];
        assert_eq!(first.cursor_id.0.into_inner(), 1);
        assert_eq!(first.image_serial.0.into_inner(), 1);

        // An equal shape in a new allocation is the same cursor.
        assert!(
            cursor
                .update(&visible((5, 5), &shape(8, 1)), now)
                .is_empty()
        );
        let beam = shape(4, 2);
        let updates = cursor.update(&visible((5, 5), &beam), now);
        assert!(matches!(updates[0], CursorUpdate::Image(_)));
        assert_eq!(sets(&updates)[0].cursor_id.0.into_inner(), 2);
        let updates = cursor.update(&visible((5, 5), &arrow), now);
        assert_eq!(sets(&updates), [first]);

        let hidden = CursorState {
            position: PointerPosition::Hidden,
            shape: Some(arrow),
        };
        let updates = cursor.update(&hidden, now);
        assert_eq!(sets(&updates)[0].cursor_id, CursorId::HIDDEN);
    }

    /// Past 128 shapes, the least recently shown Cursor ID is replaced.
    #[test]
    fn replaces_the_least_recently_used_cursor() {
        let mut cursor = HostCursor::default();
        let now = Instant::now();
        let shapes: Vec<_> = (0..=MAX_CURSOR_IDS as u8)
            .map(|colour| shape(2, colour))
            .collect();
        for shape in &shapes[..MAX_CURSOR_IDS] {
            cursor.update(&visible((0, 0), shape), now);
        }
        // Show the first again, so that the second is the least recently used.
        cursor.update(&visible((0, 0), &shapes[0]), now);
        let updates = cursor.update(&visible((0, 0), &shapes[MAX_CURSOR_IDS]), now);
        let CursorUpdate::Image(image) = &updates[0] else {
            panic!("a new image: {updates:?}");
        };
        assert_eq!(image.cursor_id.0.into_inner(), 2);
        assert_eq!(image.image_serial.0.into_inner(), MAX_CURSOR_IDS as u64 + 1);
        assert_eq!(cursor.slots.len(), MAX_CURSOR_IDS);
    }

    /// Returns the warps among updates.
    fn warps(updates: &[CursorUpdate]) -> Vec<CursorWarp> {
        updates
            .iter()
            .filter_map(|update| match update {
                CursorUpdate::Warp(warp) => Some(*warp),
                _ => None,
            })
            .collect()
    }

    /// The pointer warps when it moves without the client's input, and not
    /// while following it.
    #[test]
    fn warps_on_moves_the_client_did_not_make() {
        let mut cursor = HostCursor::default();
        let start = Instant::now();
        let arrow = shape(8, 1);
        let first = warps(&cursor.update(&visible((10, 20), &arrow), start));
        assert_eq!(first.len(), 1, "the client learns where the pointer is");
        assert_eq!(first[0].x.into_inner(), 10 * 256);
        assert_eq!(first[0].warp_id.0.into_inner(), 1);

        // The client moves the pointer; captures follow it a little late.
        cursor.input(
            &InputAction::PointerMoveTo {
                x: 100 * 256,
                y: 50 * 256,
            },
            start,
        );
        let soon = start + Duration::from_millis(16);
        assert!(warps(&cursor.update(&visible((60, 40), &arrow), soon)).is_empty());
        let later = start + Duration::from_millis(400);
        assert!(warps(&cursor.update(&visible((101, 50), &arrow), later)).is_empty());

        // An application moves the pointer.
        let moved = warps(&cursor.update(&visible((300, 200), &arrow), later));
        assert_eq!(moved.len(), 1);
        assert_eq!(
            (moved[0].x.into_inner(), moved[0].y.into_inner()),
            (300 * 256, 200 * 256)
        );
        assert_eq!(cursor.latest_warp().0.into_inner(), 2);
        assert!(!moved[0].flags.contains(WarpFlags::FOLLOW));
    }

    /// While a raw touchpad drives the pointer, every move is reported at
    /// once with `FOLLOW`, and plain warps return once it stops.
    #[test]
    fn follows_unknown_devices() {
        let mut cursor = HostCursor::default();
        let start = Instant::now();
        let arrow = shape(8, 1);
        cursor.update(&visible((10, 20), &arrow), start);
        cursor.input(
            &InputAction::Touchpad {
                device_id: DeviceId(VarInt::from_u32(16)),
                surface: TouchpadInfo {
                    width: VarInt::from_u32(10_000),
                    height: VarInt::from_u32(6_000),
                },
                clickpad: true,
                report: TouchpadReport::default(),
            },
            start,
        );
        for (step, x) in [(16, 12), (32, 15)] {
            let at = start + Duration::from_millis(step);
            let followed = warps(&cursor.update(&visible((x, 20), &arrow), at));
            assert_eq!(followed.len(), 1);
            assert!(followed[0].flags.contains(WarpFlags::FOLLOW));
            assert_eq!(followed[0].x.into_inner(), i64::from(x) * 256);
        }
        let later = start + FOLLOW_WINDOW + WARP_QUIET;
        let moved = warps(&cursor.update(&visible((300, 20), &arrow), later));
        assert!(!moved[0].flags.contains(WarpFlags::FOLLOW));
    }

    /// The warp sent on switching to absolute mode names the last position.
    #[test]
    fn warps_to_the_current_position() {
        let mut cursor = HostCursor::default();
        assert_eq!(cursor.warp_to_current().x.into_inner(), 0);
        cursor.input(
            &InputAction::PointerMoveTo { x: 3 * 256, y: 0 },
            Instant::now(),
        );
        assert_eq!(cursor.warp_to_current().x.into_inner(), 3 * 256);
        cursor.update(&visible((7, 9), &shape(4, 1)), Instant::now());
        let warp = cursor.warp_to_current();
        assert_eq!(
            (warp.x.into_inner(), warp.y.into_inner()),
            (7 * 256, 9 * 256)
        );
        assert_eq!(warp.warp_id, cursor.latest_warp());
    }

    /// A pointer whose position the source cannot tell is shown, and never
    /// warped.
    #[test]
    fn shows_untracked_pointers() {
        let mut cursor = HostCursor::default();
        let arrow = Arc::new(CursorShape::arrow());
        let state = CursorState {
            position: PointerPosition::Unknown,
            shape: Some(arrow),
        };
        let updates = cursor.update(&state, Instant::now());
        assert!(matches!(updates[0], CursorUpdate::Image(_)));
        assert!(matches!(updates[1], CursorUpdate::Set(set) if set.cursor_id != CursorId::HIDDEN));
        assert!(warps(&updates).is_empty());
        assert_eq!(CursorShape::arrow().pixels.len(), 12 * 19 * 4);
    }

    /// Oversized shapes are cropped to 512 pixels.
    #[test]
    fn crops_large_shapes() {
        let large = CursorShape {
            width: 600,
            height: 2,
            hotspot: (599, 1),
            pixels: vec![7; 600 * 2 * RGBA_PIXEL],
        };
        let image = image(
            CursorId(VarInt::from_u32(1)),
            ImageSerial(VarInt::from_u32(1)),
            &large,
        );
        assert_eq!((image.width, image.height), (512, 2));
        assert_eq!(image.hotspot_x, 511);
        assert_eq!(image.pixels.len(), 512 * 2 * RGBA_PIXEL);
        let encoded = image.to_bytes();
        assert_eq!(CursorImage::decode(&encoded).unwrap(), image);
    }
}
