//! A virtual touchscreen covering the desktop, through uinput, for the
//! client's touches.
//!
//! It speaks the kernel's type B multitouch protocol: each contact occupies a
//! slot, which a tracking ID claims while the contact touches and -1 frees.
//! It also reports the first contact as a single touch, with `BTN_TOUCH` and
//! `ABS_X` and `ABS_Y`, for software that reads only those.

use std::io;

use qdp::message::MAX_CONTACTS;
use qdp_runtime::{TouchPhase, TouchPoint};

use super::uinput::{
    ABS_X, ABS_Y, AxisRange, EV_ABS, EV_KEY, Event, INPUT_PROP_DIRECT, Identity, UI_SET_EVBIT,
    UI_SET_KEYBIT, UI_SET_PROPBIT, VirtualDevice,
};

/// `BTN_TOUCH`: whether anything touches.
const BTN_TOUCH: u16 = 0x14a;

/// `ABS_MT_SLOT`: selects the slot the following events change.
const ABS_MT_SLOT: u16 = 0x2f;

/// `ABS_MT_POSITION_X`.
const ABS_MT_POSITION_X: u16 = 0x35;

/// `ABS_MT_POSITION_Y`.
const ABS_MT_POSITION_Y: u16 = 0x36;

/// `ABS_MT_TRACKING_ID`: claims a slot for a contact, or frees it with -1.
const ABS_MT_TRACKING_ID: u16 = 0x39;

/// `ABS_MT_PRESSURE`.
const ABS_MT_PRESSURE: u16 = 0x3a;

/// The tracking ID that frees a slot.
const NO_TRACKING_ID: i32 = -1;

/// The largest tracking ID; they wrap around after it.
const MAX_TRACKING_ID: i32 = u16::MAX as i32;

/// How the touchscreen presents itself.
const IDENTITY: Identity<'static> = Identity::virtual_device("QDP touchscreen");

/// Turns touch frames into multitouch events, assigning contacts to slots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Translator {
    /// The Contact ID in each slot, and its last position.
    slots: [Option<(u8, (i32, i32))>; MAX_CONTACTS],
    /// The tracking ID the next contact gets.
    next_tracking_id: i32,
}

impl Translator {
    /// Returns the events performing a frame.
    fn events(&mut self, points: &[TouchPoint]) -> Vec<Event> {
        let mut events = Vec::new();
        for point in points {
            let position = (point.x.max(0), point.y.max(0));
            let slot = self
                .slots
                .iter()
                .position(|slot| slot.is_some_and(|(id, _)| id == point.id));
            match (point.phase, slot) {
                (TouchPhase::Down, None) => {
                    let Some(free) = self.slots.iter().position(Option::is_none) else {
                        continue;
                    };
                    self.slots[free] = Some((point.id, position));
                    events.push(Event::new(EV_ABS, ABS_MT_SLOT, free as i32));
                    events.push(Event::new(
                        EV_ABS,
                        ABS_MT_TRACKING_ID,
                        self.next_tracking_id,
                    ));
                    self.next_tracking_id = (self.next_tracking_id + 1) % (MAX_TRACKING_ID + 1);
                    events.extend(contact_events(position, point.pressure));
                }
                (TouchPhase::Down | TouchPhase::Move, Some(slot)) => {
                    self.slots[slot] = Some((point.id, position));
                    events.push(Event::new(EV_ABS, ABS_MT_SLOT, slot as i32));
                    events.extend(contact_events(position, point.pressure));
                }
                // The kernel has no cancellation: a cancelled contact lifts.
                (TouchPhase::Up | TouchPhase::Cancel, Some(slot)) => {
                    self.slots[slot] = None;
                    events.push(Event::new(EV_ABS, ABS_MT_SLOT, slot as i32));
                    events.push(Event::new(EV_ABS, ABS_MT_TRACKING_ID, NO_TRACKING_ID));
                }
                (TouchPhase::Move | TouchPhase::Up | TouchPhase::Cancel, None) => {}
            }
        }
        let first = self.slots.iter().flatten().next();
        events.push(Event::new(EV_KEY, BTN_TOUCH, i32::from(first.is_some())));
        if let Some((_, (x, y))) = first {
            events.push(Event::new(EV_ABS, ABS_X, *x));
            events.push(Event::new(EV_ABS, ABS_Y, *y));
        }
        events
    }
}

/// Returns the events placing the selected slot's contact.
fn contact_events(position: (i32, i32), pressure: u16) -> [Event; 3] {
    [
        Event::new(EV_ABS, ABS_MT_POSITION_X, position.0),
        Event::new(EV_ABS, ABS_MT_POSITION_Y, position.1),
        Event::new(EV_ABS, ABS_MT_PRESSURE, i32::from(pressure)),
    ]
}

/// A virtual touchscreen.
#[derive(Debug)]
pub(super) struct VirtualTouchscreen {
    /// The uinput device.
    device: VirtualDevice,
    /// Turns frames into events.
    translator: Translator,
}

impl VirtualTouchscreen {
    /// Creates a touchscreen covering a `width × height` pixel desktop, in
    /// 1/256 pixel.
    pub(super) fn new(width: u32, height: u32) -> io::Result<Self> {
        let file = VirtualDevice::open()?;
        VirtualDevice::set_bit(&file, UI_SET_PROPBIT, INPUT_PROP_DIRECT)?;
        VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_KEY)?;
        VirtualDevice::set_bit(&file, UI_SET_KEYBIT, BTN_TOUCH)?;
        VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_ABS)?;
        for (axes, extent) in [
            ([ABS_X, ABS_MT_POSITION_X], width),
            ([ABS_Y, ABS_MT_POSITION_Y], height),
        ] {
            for axis in axes {
                VirtualDevice::set_axis(&file, axis, AxisRange::desktop(extent))?;
            }
        }
        let slots = AxisRange::new(0, MAX_CONTACTS as i32 - 1);
        VirtualDevice::set_axis(&file, ABS_MT_SLOT, slots)?;
        let tracking_ids = AxisRange::new(0, MAX_TRACKING_ID);
        VirtualDevice::set_axis(&file, ABS_MT_TRACKING_ID, tracking_ids)?;
        let pressure = AxisRange::new(0, i32::from(u16::MAX));
        VirtualDevice::set_axis(&file, ABS_MT_PRESSURE, pressure)?;
        Ok(Self {
            device: VirtualDevice::create(file, IDENTITY)?,
            translator: Translator::default(),
        })
    }

    /// Performs a touch frame.
    pub(super) fn touch(&mut self, points: &[TouchPoint]) -> io::Result<()> {
        let events = self.translator.events(points);
        self.device.emit(&events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a point of contact `id`.
    fn point(id: u8, phase: TouchPhase, x: i32) -> TouchPoint {
        TouchPoint {
            id,
            phase,
            x,
            y: 100,
            pressure: 7,
        }
    }

    /// Contacts claim free slots with new tracking IDs, move within them, and
    /// free them when they lift or are cancelled, with the first contact as
    /// the single touch.
    #[test]
    fn translates_frames() {
        assert_eq!(UI_SET_PROPBIT, 0x4004_556e);
        let mut translator = Translator::default();
        let down = translator.events(&[point(4, TouchPhase::Down, 10)]);
        assert_eq!(
            down,
            [
                Event::new(EV_ABS, ABS_MT_SLOT, 0),
                Event::new(EV_ABS, ABS_MT_TRACKING_ID, 0),
                Event::new(EV_ABS, ABS_MT_POSITION_X, 10),
                Event::new(EV_ABS, ABS_MT_POSITION_Y, 100),
                Event::new(EV_ABS, ABS_MT_PRESSURE, 7),
                Event::new(EV_KEY, BTN_TOUCH, 1),
                Event::new(EV_ABS, ABS_X, 10),
                Event::new(EV_ABS, ABS_Y, 100),
            ]
        );
        let second = translator.events(&[
            point(4, TouchPhase::Move, 12),
            point(9, TouchPhase::Down, 50),
        ]);
        assert!(second.contains(&Event::new(EV_ABS, ABS_MT_SLOT, 1)));
        assert!(second.contains(&Event::new(EV_ABS, ABS_MT_TRACKING_ID, 1)));
        let lifted =
            translator.events(&[point(4, TouchPhase::Up, 12), point(9, TouchPhase::Move, 50)]);
        assert!(lifted.contains(&Event::new(EV_ABS, ABS_MT_TRACKING_ID, NO_TRACKING_ID)));
        assert_eq!(lifted.last(), Some(&Event::new(EV_ABS, ABS_Y, 100)));
        assert!(lifted.contains(&Event::new(EV_ABS, ABS_X, 50)));
        let cancelled = translator.events(&[point(9, TouchPhase::Cancel, 50)]);
        assert_eq!(
            cancelled,
            [
                Event::new(EV_ABS, ABS_MT_SLOT, 1),
                Event::new(EV_ABS, ABS_MT_TRACKING_ID, NO_TRACKING_ID),
                Event::new(EV_KEY, BTN_TOUCH, 0),
            ]
        );
        let reused = translator.events(&[point(4, TouchPhase::Down, 0)]);
        assert_eq!(
            reused[..2],
            [
                Event::new(EV_ABS, ABS_MT_SLOT, 0),
                Event::new(EV_ABS, ABS_MT_TRACKING_ID, 2),
            ]
        );
    }
}
