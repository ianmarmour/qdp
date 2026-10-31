//! Input injection on macOS through Quartz event services (`CGEventPost`).
//!
//! macOS delivers posted events only from apps the user has allowed under
//! System Settings, Privacy & Security, Accessibility.

use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType};
use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{
    CGDisplayBounds, CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID,
    CGEventTapLocation, CGEventType, CGMainDisplayID, CGMouseButton, CGScrollEventUnit,
};
use qdp::message::KeyboardLeds;
use qdp_runtime::{InputAction, InputInjector, PointerButton};

use super::keycodes::virtual_key;

/// QDP positions and motion are in 1/256 pixel or count (Section 1.5.6).
const SUBUNITS: f64 = 256.0;

/// Scroll units per line: QDP scrolls in 1/120 detent.
const UNITS_PER_LINE: i32 = 120;

/// The longest string one keyboard event carries, in UTF-16 units.
const MAX_UNICODE_UNITS: usize = 20;

/// The Caps Lock LED bit of the keyboard output report.
const CAPS_LOCK_LED: u8 = 0x02;

/// The Back and Forward buttons' numbers, which macOS counts after the left,
/// right and middle buttons.
const BACK_BUTTON: u32 = 3;

/// The Forward button's number.
const FORWARD_BUTTON: u32 = 4;

/// The subtype of system-defined events that carry the media keys.
const MEDIA_KEY_SUBTYPE: i16 = 8;

/// The state bits of a media key event: pressed, and released.
const MEDIA_KEY_DOWN: isize = 0x0a;

/// The state bits of a released media key.
const MEDIA_KEY_UP: isize = 0x0b;

/// The modifier flags system-defined media key events carry, pressed and
/// released.
const MEDIA_KEY_FLAGS: [usize; 2] = [0xa00, 0xb00];

/// Returns the macOS media key (`NX_KEYTYPE_*` in IOKit's `ev_keymap.h`) of a
/// Consumer page usage, if it has one.
fn media_key(usage: u16) -> Option<isize> {
    match usage {
        0x00e9 => Some(0),  // Volume Increment: NX_KEYTYPE_SOUND_UP
        0x00ea => Some(1),  // Volume Decrement: NX_KEYTYPE_SOUND_DOWN
        0x00e2 => Some(7),  // Mute: NX_KEYTYPE_MUTE
        0x00cd => Some(16), // Play/Pause: NX_KEYTYPE_PLAY
        0x00b5 => Some(17), // Scan Next Track: NX_KEYTYPE_NEXT
        0x00b6 => Some(18), // Scan Previous Track: NX_KEYTYPE_PREVIOUS
        _ => None,
    }
}

/// Returns the event types and button number of a button: the press, the
/// release and the drag.
fn button_events(button: PointerButton) -> (CGEventType, CGEventType, CGEventType, CGMouseButton) {
    match button {
        PointerButton::Left => (
            CGEventType::LeftMouseDown,
            CGEventType::LeftMouseUp,
            CGEventType::LeftMouseDragged,
            CGMouseButton::Left,
        ),
        PointerButton::Right => (
            CGEventType::RightMouseDown,
            CGEventType::RightMouseUp,
            CGEventType::RightMouseDragged,
            CGMouseButton::Right,
        ),
        PointerButton::Middle => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGEventType::OtherMouseDragged,
            CGMouseButton::Center,
        ),
        PointerButton::Back => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGEventType::OtherMouseDragged,
            CGMouseButton(BACK_BUTTON),
        ),
        PointerButton::Forward => (
            CGEventType::OtherMouseDown,
            CGEventType::OtherMouseUp,
            CGEventType::OtherMouseDragged,
            CGMouseButton(FORWARD_BUTTON),
        ),
    }
}

/// Injects input as Quartz events on the main display.
#[derive(Debug)]
pub struct QuartzInjector {
    /// The main display's top-left corner in global coordinates, in points.
    origin: CGPoint,
    /// Points per captured pixel: below 1 on Retina displays.
    scale: (f64, f64),
    /// The main display's size in points.
    size: (f64, f64),
    /// The pointer's position, in global points.
    position: CGPoint,
    /// The buttons held, in press order, so moves become drags.
    held: Vec<PointerButton>,
    /// Scrolling not yet sent as whole lines, in 1/120 detent.
    scroll_remainder: (i32, i32),
}

impl QuartzInjector {
    /// Returns an injector for the main display, captured at
    /// `width × height` pixels.
    pub fn new(width: u32, height: u32) -> Self {
        let bounds = CGDisplayBounds(CGMainDisplayID());
        let position =
            CGEvent::new(None).map_or(bounds.origin, |event| CGEvent::location(Some(&event)));
        Self {
            origin: bounds.origin,
            scale: (
                bounds.size.width / f64::from(width.max(1)),
                bounds.size.height / f64::from(height.max(1)),
            ),
            size: (bounds.size.width, bounds.size.height),
            position,
            held: Vec::new(),
            scroll_remainder: (0, 0),
        }
    }

    /// Moves the pointer to a global position, dragging held buttons.
    fn move_to(&mut self, position: CGPoint, delta: Option<(i64, i64)>) -> Result<(), String> {
        self.position = CGPoint {
            x: position
                .x
                .clamp(self.origin.x, self.origin.x + self.size.0 - 1.0),
            y: position
                .y
                .clamp(self.origin.y, self.origin.y + self.size.1 - 1.0),
        };
        let (kind, button) = match self.held.last() {
            Some(&held) => {
                let (_, _, drag, button) = button_events(held);
                (drag, button)
            }
            None => (CGEventType::MouseMoved, CGMouseButton::Left),
        };
        let event = CGEvent::new_mouse_event(None, kind, self.position, button)
            .ok_or("cannot create a mouse event")?;
        if let Some((dx, dy)) = delta {
            CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventDeltaX, dx);
            CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventDeltaY, dy);
        }
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        Ok(())
    }

    /// Presses or releases a key by virtual key code.
    fn key(&self, code: u16, down: bool) -> Result<(), String> {
        let event =
            CGEvent::new_keyboard_event(None, code, down).ok_or("cannot create a key event")?;
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        Ok(())
    }

    /// Presses or releases a media key, which macOS posts as a system-defined
    /// event rather than a key event.
    fn media_key(&self, key: isize, down: bool) -> Result<(), String> {
        let (state, flags) = if down {
            (MEDIA_KEY_DOWN, MEDIA_KEY_FLAGS[0])
        } else {
            (MEDIA_KEY_UP, MEDIA_KEY_FLAGS[1])
        };
        let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::SystemDefined,
            CGPoint { x: 0.0, y: 0.0 },
            NSEventModifierFlags(flags),
            0.0,
            0,
            None,
            MEDIA_KEY_SUBTYPE,
            (key << 16) | (state << 8),
            -1,
        )
        .and_then(|event| event.CGEvent())
        .ok_or("cannot create a media key event")?;
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
        Ok(())
    }

    /// Types text, a few characters per event.
    fn text(&self, text: &str) -> Result<(), String> {
        let units: Vec<u16> = text.encode_utf16().collect();
        for chunk in units.chunks(MAX_UNICODE_UNITS) {
            for down in [true, false] {
                let event = CGEvent::new_keyboard_event(None, 0, down)
                    .ok_or("cannot create a key event")?;
                // SAFETY: `chunk` holds `chunk.len()` UTF-16 units, which the
                // call copies.
                unsafe {
                    CGEvent::keyboard_set_unicode_string(
                        Some(&event),
                        chunk.len() as _,
                        chunk.as_ptr(),
                    );
                }
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
            }
        }
        Ok(())
    }
}

impl InputInjector for QuartzInjector {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        match action {
            &InputAction::KeyDown(usage) | &InputAction::KeyUp(usage) => {
                let code = virtual_key(usage).ok_or("the key has no macOS key code")?;
                self.key(code, matches!(action, InputAction::KeyDown(_)))
            }
            &InputAction::ConsumerKeyDown(usage) | &InputAction::ConsumerKeyUp(usage) => {
                let key = media_key(usage).ok_or("the media key has no macOS equivalent")?;
                self.media_key(key, matches!(action, InputAction::ConsumerKeyDown(_)))
            }
            &InputAction::PointerMoveTo { x, y } => {
                let position = CGPoint {
                    x: self.origin.x + f64::from(x) / SUBUNITS * self.scale.0,
                    y: self.origin.y + f64::from(y) / SUBUNITS * self.scale.1,
                };
                self.move_to(position, None)
            }
            &InputAction::MouseMoveBy { dx, dy } => {
                let dx = f64::from(dx) / SUBUNITS * self.scale.0;
                let dy = f64::from(dy) / SUBUNITS * self.scale.1;
                let position = CGPoint {
                    x: self.position.x + dx,
                    y: self.position.y + dy,
                };
                self.move_to(position, Some((dx.round() as i64, dy.round() as i64)))
            }
            &InputAction::ButtonDown(button) | &InputAction::ButtonUp(button) => {
                let down = matches!(action, InputAction::ButtonDown(_));
                let (press, release, _, number) = button_events(button);
                self.held.retain(|&held| held != button);
                if down {
                    self.held.push(button);
                }
                let kind = if down { press } else { release };
                let event = CGEvent::new_mouse_event(None, kind, self.position, number)
                    .ok_or("cannot create a mouse event")?;
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
                Ok(())
            }
            &InputAction::Scroll {
                vertical,
                horizontal,
            } => {
                let vertical = self.scroll_remainder.0 + vertical;
                let horizontal = self.scroll_remainder.1 + horizontal;
                self.scroll_remainder = (vertical % UNITS_PER_LINE, horizontal % UNITS_PER_LINE);
                let (lines_up, lines_right) =
                    (vertical / UNITS_PER_LINE, horizontal / UNITS_PER_LINE);
                if lines_up == 0 && lines_right == 0 {
                    return Ok(());
                }
                // The second wheel's positive direction scrolls left.
                let event = CGEvent::new_scroll_wheel_event2(
                    None,
                    CGScrollEventUnit::Line,
                    2,
                    lines_up,
                    -lines_right,
                    0,
                )
                .ok_or("cannot create a scroll event")?;
                CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
                Ok(())
            }
            InputAction::Text(text) => self.text(text),
            // The host does not accept gamepads, so none are attached.
            InputAction::Gamepad { .. } | InputAction::GamepadRemoved(_) => {
                Err("Quartz events cannot present gamepads".to_owned())
            }
            // The host does not accept touchscreens: macOS has no touch events.
            InputAction::Touch(_) => Err("Quartz events cannot touch".to_owned()),
        }
    }

    fn description(&self) -> &str {
        "Quartz events"
    }

    fn keyboard_leds(&mut self) -> KeyboardLeds {
        let flags = CGEventSource::flags_state(CGEventSourceStateID::HIDSystemState);
        if flags.contains(CGEventFlags::MaskAlphaShift) {
            KeyboardLeds(CAPS_LOCK_LED)
        } else {
            KeyboardLeds::default()
        }
    }

    fn supports_text(&self) -> bool {
        true
    }
}
