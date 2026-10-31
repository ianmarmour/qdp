//! A virtual touchpad, through uinput, for a client's raw touchpad, whose
//! contacts the host's own gesture recognition reads (Section 8.4.6).
//!
//! It reports like a laptop's touchpad: type B multitouch slots, the number
//! of fingers as `BTN_TOOL_*`, and its buttons, with the surface's size as
//! the axes' resolution so that gestures scale as on the real surface.
//! Contacts that are not fingers, such as palms, are left out.

use std::io;

use qdp::message::{
    TouchpadButtons, TouchpadContact, TouchpadContactFlags, TouchpadInfo, TouchpadReport,
};
use qdp_runtime::{TouchPhase, TouchPoint};

use super::touchscreen::{
    ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID, BTN_TOUCH,
    MAX_TRACKING_ID, Translator,
};
use super::uinput::{
    ABS_X, ABS_Y, AxisRange, EV_ABS, EV_KEY, Event, Identity, UI_SET_EVBIT, UI_SET_KEYBIT,
    UI_SET_PROPBIT, VirtualDevice,
};

/// `INPUT_PROP_POINTER`: the device moves a pointer rather than touching the
/// screen where it shows.
const INPUT_PROP_POINTER: u16 = 0x00;

/// `INPUT_PROP_BUTTONPAD`: the surface is the button.
const INPUT_PROP_BUTTONPAD: u16 = 0x02;

/// `BTN_LEFT`.
const BTN_LEFT: u16 = 0x110;

/// `BTN_RIGHT`.
const BTN_RIGHT: u16 = 0x111;

/// The `BTN_TOOL_*` codes reporting one to five or more fingers.
const FINGER_TOOLS: [u16; 5] = [
    0x145, // BTN_TOOL_FINGER
    0x14d, // BTN_TOOL_DOUBLETAP
    0x14e, // BTN_TOOL_TRIPLETAP
    0x14f, // BTN_TOOL_QUADTAP
    0x148, // BTN_TOOL_QUINTTAP
];

/// The largest coordinate along either edge of the surface.
const MAX_COORDINATE: i32 = u16::MAX as i32;

/// Hundredths of a millimetre per millimetre, the unit of the surface size.
const HUNDREDTHS_PER_MM: u64 = 100;

/// How the touchpad presents itself.
const IDENTITY: Identity<'static> = Identity::virtual_device("QDP touchpad");

/// Returns the resolution, in units per millimetre, of an axis covering
/// `size` hundredths of a millimetre.
fn resolution(size: u64) -> i32 {
    let units = (MAX_COORDINATE as u64 * HUNDREDTHS_PER_MM) / size.max(1);
    i32::try_from(units.max(1)).unwrap_or(i32::MAX)
}

/// Turns touchpad reports into events.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TouchpadTranslator {
    /// Assigns the fingers to slots.
    slots: Translator,
    /// The fingers of the last report.
    fingers: Vec<TouchpadContact>,
}

impl TouchpadTranslator {
    /// Returns the events setting the touchpad to `report`, on a clickpad
    /// whose one button both buttons press if `clickpad`.
    fn events(&mut self, report: &TouchpadReport, clickpad: bool) -> Vec<Event> {
        let fingers: Vec<TouchpadContact> = report
            .contacts
            .iter()
            .filter(|contact| contact.flags.contains(TouchpadContactFlags::CONFIDENCE))
            .copied()
            .collect();
        let point = |contact: &TouchpadContact, phase| TouchPoint {
            id: contact.id,
            phase,
            x: i32::from(contact.x),
            y: i32::from(contact.y),
            pressure: 0,
        };
        let mut points: Vec<TouchPoint> = self
            .fingers
            .iter()
            .filter(|old| fingers.iter().all(|finger| finger.id != old.id))
            .map(|old| point(old, TouchPhase::Up))
            .collect();
        points.extend(fingers.iter().map(|finger| {
            let touching = self.fingers.iter().any(|old| old.id == finger.id);
            point(
                finger,
                if touching {
                    TouchPhase::Move
                } else {
                    TouchPhase::Down
                },
            )
        }));
        let mut events = self.slots.events(&points);
        let tool = fingers.len().min(FINGER_TOOLS.len());
        events.extend(
            FINGER_TOOLS
                .iter()
                .enumerate()
                .map(|(index, &code)| Event::new(EV_KEY, code, i32::from(index + 1 == tool))),
        );
        let primary = report.buttons.contains(TouchpadButtons::PRIMARY);
        let secondary = report.buttons.contains(TouchpadButtons::SECONDARY);
        if clickpad {
            events.push(Event::new(
                EV_KEY,
                BTN_LEFT,
                i32::from(primary || secondary),
            ));
        } else {
            events.push(Event::new(EV_KEY, BTN_LEFT, i32::from(primary)));
            events.push(Event::new(EV_KEY, BTN_RIGHT, i32::from(secondary)));
        }
        self.fingers = fingers;
        events
    }
}

/// A virtual touchpad.
#[derive(Debug)]
pub(super) struct VirtualTouchpad {
    /// The uinput device.
    device: VirtualDevice,
    /// Whether the surface is the button.
    clickpad: bool,
    /// Turns reports into events.
    translator: TouchpadTranslator,
}

impl VirtualTouchpad {
    /// Creates a touchpad with a surface of `surface`.
    pub(super) fn new(surface: TouchpadInfo, clickpad: bool) -> io::Result<Self> {
        let file = VirtualDevice::open()?;
        VirtualDevice::set_bit(&file, UI_SET_PROPBIT, INPUT_PROP_POINTER)?;
        if clickpad {
            VirtualDevice::set_bit(&file, UI_SET_PROPBIT, INPUT_PROP_BUTTONPAD)?;
        }
        VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_KEY)?;
        let buttons: &[u16] = if clickpad {
            &[BTN_LEFT]
        } else {
            &[BTN_LEFT, BTN_RIGHT]
        };
        for &code in buttons.iter().chain(&[BTN_TOUCH]).chain(&FINGER_TOOLS) {
            VirtualDevice::set_bit(&file, UI_SET_KEYBIT, code)?;
        }
        VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_ABS)?;
        for (axes, size) in [
            ([ABS_X, ABS_MT_POSITION_X], surface.width.into_inner()),
            ([ABS_Y, ABS_MT_POSITION_Y], surface.height.into_inner()),
        ] {
            let range = AxisRange {
                resolution: resolution(size),
                ..AxisRange::new(0, MAX_COORDINATE)
            };
            for axis in axes {
                VirtualDevice::set_axis(&file, axis, range)?;
            }
        }
        let slots = AxisRange::new(0, qdp::message::MAX_CONTACTS as i32 - 1);
        VirtualDevice::set_axis(&file, ABS_MT_SLOT, slots)?;
        let tracking_ids = AxisRange::new(0, MAX_TRACKING_ID);
        VirtualDevice::set_axis(&file, ABS_MT_TRACKING_ID, tracking_ids)?;
        Ok(Self {
            device: VirtualDevice::create(file, IDENTITY)?,
            clickpad,
            translator: TouchpadTranslator::default(),
        })
    }

    /// Sets the touchpad to `report`.
    pub(super) fn set(&mut self, report: &TouchpadReport) -> io::Result<()> {
        let events = self.translator.events(report, self.clickpad);
        self.device.emit(&events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a contact with `flags` at `x`.
    fn contact(id: u8, x: u16, flags: TouchpadContactFlags) -> TouchpadContact {
        TouchpadContact {
            id,
            flags,
            x,
            y: 100,
        }
    }

    /// Returns the value an event list gives `code`, if any.
    fn value(events: &[Event], kind: u16, code: u16) -> Option<i32> {
        events
            .iter()
            .rev()
            .find(|event| event.kind == kind && event.code == code)
            .map(|event| event.value)
    }

    /// Fingers become slots and finger counts, palms are left out, and a
    /// clickpad's one button takes both buttons.
    #[test]
    fn translates_reports() {
        let finger = TouchpadContactFlags::CONFIDENCE;
        let palm = TouchpadContactFlags::default();
        let mut translator = TouchpadTranslator::default();
        let two = TouchpadReport {
            buttons: TouchpadButtons::SECONDARY,
            contacts: vec![
                contact(0, 10, finger),
                contact(1, 20, finger),
                contact(2, 30, palm),
            ],
        };
        let events = translator.events(&two, true);
        assert_eq!(value(&events, EV_KEY, FINGER_TOOLS[1]), Some(1));
        assert_eq!(value(&events, EV_KEY, FINGER_TOOLS[0]), Some(0));
        assert_eq!(value(&events, EV_KEY, BTN_LEFT), Some(1));
        assert_eq!(value(&events, EV_KEY, BTN_TOUCH), Some(1));
        assert!(events.contains(&Event::new(EV_ABS, ABS_MT_TRACKING_ID, 1)));
        assert!(!events.contains(&Event::new(EV_ABS, ABS_MT_POSITION_X, 30)));
        let lifted = translator.events(&TouchpadReport::default(), false);
        assert_eq!(value(&lifted, EV_KEY, BTN_TOUCH), Some(0));
        assert!(
            FINGER_TOOLS
                .iter()
                .all(|&code| value(&lifted, EV_KEY, code) == Some(0))
        );
        assert_eq!(value(&lifted, EV_KEY, BTN_RIGHT), Some(0));
    }

    /// A 100 mm surface gives 655 units per millimetre.
    #[test]
    fn sizes_the_surface() {
        assert_eq!(resolution(10_000), 655);
        assert_eq!(resolution(0), MAX_COORDINATE * 100);
    }
}
