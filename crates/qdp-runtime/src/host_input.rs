//! Turning the client's standard device reports into actions for the host's
//! input injector (Sections 8.4 and 8.7).
//!
//! Standard reports state each device's complete current state, so the host
//! compares every report with the last one it applied for the device and
//! performs only the differences: keys pressed and released, buttons, motion
//! and scrolling.

use std::collections::HashMap;

use qdp::input::{InputEvent, total_delta};
use qdp::message::{
    DeviceId, DeviceKind, KeyboardLeds, KeyboardReport, MouseReport, PointerButtons, PointerReport,
    TextReport,
};
use tracing::debug;

/// The buttons a pointer or mouse report can hold, in report bit order.
const BUTTONS: [PointerButton; 5] = [
    PointerButton::Left,
    PointerButton::Right,
    PointerButton::Middle,
    PointerButton::Back,
    PointerButton::Forward,
];

/// The number of Keyboard/Keypad page usages a keyboard report covers.
const KEYBOARD_USAGES: usize = 256;

/// A pointer or mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerButton {
    /// The left button.
    Left,
    /// The right button.
    Right,
    /// The middle button.
    Middle,
    /// The back button.
    Back,
    /// The forward button.
    Forward,
}

impl PointerButton {
    /// Returns the button's bit in a report's Buttons field.
    pub const fn mask(self) -> PointerButtons {
        match self {
            Self::Left => PointerButtons::LEFT,
            Self::Right => PointerButtons::RIGHT,
            Self::Middle => PointerButtons::MIDDLE,
            Self::Back => PointerButtons::BACK,
            Self::Forward => PointerButtons::FORWARD,
        }
    }
}

/// One thing the host's input injector does.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InputAction {
    /// Presses the key with a Keyboard/Keypad page (0x07) usage.
    KeyDown(u8),
    /// Releases the key with a Keyboard/Keypad page usage.
    KeyUp(u8),
    /// Presses the key with a Consumer page (0x0C) usage.
    ConsumerKeyDown(u16),
    /// Releases the key with a Consumer page usage.
    ConsumerKeyUp(u16),
    /// Moves the pointer to a desktop position, in 1/256 pixel.
    PointerMoveTo {
        /// The horizontal position.
        x: i32,
        /// The vertical position.
        y: i32,
    },
    /// Moves the pointer by a relative amount, in 1/256 device count.
    MouseMoveBy {
        /// The horizontal motion.
        dx: i32,
        /// The vertical motion.
        dy: i32,
    },
    /// Presses a button.
    ButtonDown(PointerButton),
    /// Releases a button.
    ButtonUp(PointerButton),
    /// Scrolls, in 1/120 of a wheel detent.
    Scroll {
        /// Vertical scrolling; positive is away from the user.
        vertical: i32,
        /// Horizontal scrolling; positive is to the right.
        horizontal: i32,
    },
    /// Types committed text.
    Text(String),
}

/// Performs input actions on the host's desktop.
pub trait InputInjector: Send {
    /// Performs one action.
    fn inject(&mut self, action: &InputAction) -> Result<(), String>;

    /// Returns which injector this is, for logs.
    fn description(&self) -> &str;

    /// Returns the host's keyboard LED state, for the keyboard output report
    /// (Section 8.4.1).
    fn keyboard_leds(&mut self) -> KeyboardLeds {
        KeyboardLeds::default()
    }

    /// Returns whether the injector can type committed text, so that the host
    /// accepts text devices.
    fn supports_text(&self) -> bool {
        false
    }
}

/// An injector that performs nothing, for hosts that only stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NoInput;

impl InputInjector for NoInput {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        debug!(?action, "input ignored");
        Ok(())
    }

    fn description(&self) -> &str {
        "none"
    }

    fn supports_text(&self) -> bool {
        true
    }
}

/// Whether a report's pointer positions are applied (Section 8.7, rule 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Positions {
    /// The positions are current: apply them.
    Apply,
    /// The positions were computed before the host's latest warp: ignore
    /// them, applying buttons and scrolling only.
    Ignore,
}

/// The last applied state of one device.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DeviceState {
    /// A keyboard.
    Keyboard(KeyboardReport),
    /// An absolute pointer, with whether a position has been applied yet.
    Pointer(PointerReport, bool),
    /// A relative mouse.
    Mouse(MouseReport),
    /// A text device, which has no state.
    Text,
}

/// The standard devices of one session and their last applied states.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StandardInput {
    /// The accepted devices.
    devices: HashMap<DeviceId, DeviceState>,
}

impl StandardInput {
    /// Adds an accepted device, at rest, returning whether its kind is
    /// supported.
    pub(crate) fn attach(&mut self, device_id: DeviceId, kind: DeviceKind) -> bool {
        let state = match kind {
            DeviceKind::KEYBOARD => DeviceState::Keyboard(KeyboardReport::default()),
            DeviceKind::POINTER => DeviceState::Pointer(PointerReport::default(), false),
            DeviceKind::MOUSE => DeviceState::Mouse(MouseReport::default()),
            DeviceKind::TEXT => DeviceState::Text,
            _ => return false,
        };
        self.devices.insert(device_id, state);
        true
    }

    /// Returns the accepted keyboards.
    pub(crate) fn keyboards(&self) -> Vec<DeviceId> {
        self.devices
            .iter()
            .filter(|(_, state)| matches!(state, DeviceState::Keyboard(_)))
            .map(|(&id, _)| id)
            .collect()
    }

    /// Returns whether a device has been accepted.
    pub(crate) fn contains(&self, device_id: DeviceId) -> bool {
        self.devices.contains_key(&device_id)
    }

    /// Removes a device, returning the actions that release what it held.
    pub(crate) fn detach(&mut self, device_id: DeviceId) -> Vec<InputAction> {
        self.devices
            .remove(&device_id)
            .map_or_else(Vec::new, |state| release(&state))
    }

    /// Returns the actions that release everything every device holds, as when
    /// the session ends, and leaves the devices at rest.
    pub(crate) fn release_all(&mut self) -> Vec<InputAction> {
        let ids: Vec<DeviceId> = self.devices.keys().copied().collect();
        let mut actions = Vec::new();
        for id in ids {
            if let Some(state) = self.devices.get_mut(&id) {
                actions.extend(release(state));
                *state = at_rest(state);
            }
        }
        actions
    }

    /// Returns the actions a report calls for. A report that does not parse
    /// is skipped.
    pub(crate) fn apply(&mut self, event: &InputEvent, positions: Positions) -> Vec<InputAction> {
        let Some(state) = self.devices.get_mut(&event.device_id) else {
            return Vec::new();
        };
        let mut actions = Vec::new();
        match state {
            DeviceState::Keyboard(previous) => {
                let Ok(report) = KeyboardReport::decode(&event.data) else {
                    debug!("malformed keyboard report skipped");
                    return actions;
                };
                keyboard_actions(previous, &report, &mut actions);
                *previous = report;
            }
            DeviceState::Pointer(previous, positioned) => {
                let Ok(mut report) = PointerReport::decode(&event.data) else {
                    debug!("malformed pointer report skipped");
                    return actions;
                };
                // Move first, so that a press lands where the report says.
                let moved = !*positioned || (report.x, report.y) != (previous.x, previous.y);
                if moved && positions == Positions::Apply {
                    actions.push(InputAction::PointerMoveTo {
                        x: report.x,
                        y: report.y,
                    });
                    *positioned = true;
                }
                if positions == Positions::Ignore {
                    // Keep the last applied position, so that the next current
                    // report moves the pointer even to the same place.
                    (report.x, report.y) = (previous.x, previous.y);
                }
                button_actions(previous.buttons, report.buttons, &mut actions);
                scroll_action(
                    (previous.wheel_total, previous.pan_total),
                    (report.wheel_total, report.pan_total),
                    &mut actions,
                );
                *previous = report;
            }
            DeviceState::Mouse(previous) => {
                let Ok(report) = MouseReport::decode(&event.data) else {
                    debug!("malformed mouse report skipped");
                    return actions;
                };
                let dx = total_delta(previous.motion_x_total, report.motion_x_total);
                let dy = total_delta(previous.motion_y_total, report.motion_y_total);
                if dx != 0 || dy != 0 {
                    actions.push(InputAction::MouseMoveBy { dx, dy });
                }
                button_actions(previous.buttons, report.buttons, &mut actions);
                scroll_action(
                    (previous.wheel_total, previous.pan_total),
                    (report.wheel_total, report.pan_total),
                    &mut actions,
                );
                *previous = report;
            }
            DeviceState::Text => match TextReport::decode(&event.data) {
                Ok(report) => actions.push(InputAction::Text(report.text)),
                Err(_) => debug!("malformed text report skipped"),
            },
        }
        actions
    }
}

/// Appends the key releases, then presses, between two keyboard states.
fn keyboard_actions(
    previous: &KeyboardReport,
    report: &KeyboardReport,
    actions: &mut Vec<InputAction>,
) {
    let usages = || (0..KEYBOARD_USAGES).map(|usage| usage as u8);
    actions.extend(
        usages()
            .filter(|&usage| previous.is_held(usage) && !report.is_held(usage))
            .map(InputAction::KeyUp),
    );
    actions.extend(
        previous
            .consumer_keys
            .iter()
            .filter(|&&usage| usage != 0 && !report.consumer_keys.contains(&usage))
            .map(|&usage| InputAction::ConsumerKeyUp(usage)),
    );
    actions.extend(
        usages()
            .filter(|&usage| !previous.is_held(usage) && report.is_held(usage))
            .map(InputAction::KeyDown),
    );
    actions.extend(
        report
            .consumer_keys
            .iter()
            .filter(|&&usage| usage != 0 && !previous.consumer_keys.contains(&usage))
            .map(|&usage| InputAction::ConsumerKeyDown(usage)),
    );
}

/// Appends the button releases, then presses, between two button states.
fn button_actions(
    previous: PointerButtons,
    current: PointerButtons,
    actions: &mut Vec<InputAction>,
) {
    for button in BUTTONS {
        if previous.contains(button.mask()) && !current.contains(button.mask()) {
            actions.push(InputAction::ButtonUp(button));
        }
    }
    for button in BUTTONS {
        if !previous.contains(button.mask()) && current.contains(button.mask()) {
            actions.push(InputAction::ButtonDown(button));
        }
    }
}

/// Appends the scrolling between two pairs of wheel and pan totals, if any.
fn scroll_action(previous: (i32, i32), current: (i32, i32), actions: &mut Vec<InputAction>) {
    let vertical = total_delta(previous.0, current.0);
    let horizontal = total_delta(previous.1, current.1);
    if vertical != 0 || horizontal != 0 {
        actions.push(InputAction::Scroll {
            vertical,
            horizontal,
        });
    }
}

/// Returns the actions that release what a device holds.
fn release(state: &DeviceState) -> Vec<InputAction> {
    let mut actions = Vec::new();
    match state {
        DeviceState::Keyboard(report) => {
            keyboard_actions(report, &KeyboardReport::default(), &mut actions);
        }
        DeviceState::Pointer(report, _) => {
            button_actions(report.buttons, PointerButtons::default(), &mut actions);
        }
        DeviceState::Mouse(report) => {
            button_actions(report.buttons, PointerButtons::default(), &mut actions);
        }
        DeviceState::Text => {}
    }
    actions
}

/// Returns a device's state with nothing held, keeping its position and
/// running totals.
fn at_rest(state: &DeviceState) -> DeviceState {
    match state {
        DeviceState::Keyboard(_) => DeviceState::Keyboard(KeyboardReport::default()),
        DeviceState::Pointer(report, positioned) => DeviceState::Pointer(
            PointerReport {
                buttons: PointerButtons::default(),
                ..*report
            },
            *positioned,
        ),
        DeviceState::Mouse(report) => DeviceState::Mouse(MouseReport {
            buttons: PointerButtons::default(),
            ..*report
        }),
        DeviceState::Text => DeviceState::Text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::{DisplayConfigId, InputSequence, Timestamp, WarpId};
    use qdp::wire::VarInt;

    /// The keyboard's Device ID in the tests.
    const KEYBOARD: DeviceId = DeviceId(VarInt::from_u32(1));

    /// The pointer's Device ID in the tests.
    const POINTER: DeviceId = DeviceId(VarInt::from_u32(2));

    /// The mouse's Device ID in the tests.
    const MOUSE: DeviceId = DeviceId(VarInt::from_u32(3));

    /// Returns an event carrying `data` from `device_id`.
    fn event(device_id: DeviceId, data: Vec<u8>) -> InputEvent {
        InputEvent {
            sequence: InputSequence(VarInt::from_u32(1)),
            device_id,
            produced_at: Timestamp::default(),
            display_config_id: DisplayConfigId::default(),
            warp_id: WarpId::NONE,
            data,
        }
    }

    /// Returns devices with a keyboard, a pointer and a mouse attached.
    fn devices() -> StandardInput {
        let mut input = StandardInput::default();
        assert!(input.attach(KEYBOARD, DeviceKind::KEYBOARD));
        assert!(input.attach(POINTER, DeviceKind::POINTER));
        assert!(input.attach(MOUSE, DeviceKind::MOUSE));
        assert!(!input.attach(DeviceId(VarInt::from_u32(9)), DeviceKind::GAMEPAD));
        input
    }

    /// Keyboard reports become releases, then presses, of the keys that changed.
    #[test]
    fn keyboard_differences() {
        let mut input = devices();
        let mut report = KeyboardReport::default();
        report.set_held(0xe1, true); // Left Shift
        report.set_held(0x04, true); // A
        report.consumer_keys[0] = 0x00e9; // Volume Increment
        let actions = input.apply(&event(KEYBOARD, report.to_bytes()), Positions::Apply);
        assert_eq!(
            actions,
            [
                InputAction::KeyDown(0x04),
                InputAction::KeyDown(0xe1),
                InputAction::ConsumerKeyDown(0x00e9)
            ]
        );
        report.set_held(0x04, false);
        report.set_held(0x05, true);
        report.consumer_keys[0] = 0;
        let actions = input.apply(&event(KEYBOARD, report.to_bytes()), Positions::Apply);
        assert_eq!(
            actions,
            [
                InputAction::KeyUp(0x04),
                InputAction::ConsumerKeyUp(0x00e9),
                InputAction::KeyDown(0x05)
            ]
        );
        assert_eq!(
            input.release_all(),
            [InputAction::KeyUp(0x05), InputAction::KeyUp(0xe1)]
        );
        assert!(input.release_all().is_empty());
    }

    /// Pointer reports move before pressing, and scroll by the change in totals,
    /// across wrap-around.
    #[test]
    fn pointer_differences() {
        let mut input = devices();
        let report = PointerReport {
            buttons: PointerButtons::LEFT,
            x: 100 * 256,
            y: 50 * 256,
            wheel_total: i32::MAX,
            pan_total: 0,
        };
        assert_eq!(
            input.apply(&event(POINTER, report.to_bytes()), Positions::Apply),
            [
                InputAction::PointerMoveTo {
                    x: 100 * 256,
                    y: 50 * 256
                },
                InputAction::ButtonDown(PointerButton::Left),
                InputAction::Scroll {
                    vertical: i32::MAX,
                    horizontal: 0
                }
            ]
        );
        let next = PointerReport {
            buttons: PointerButtons::default(),
            wheel_total: i32::MIN,
            ..report
        };
        assert_eq!(
            input.apply(&event(POINTER, next.to_bytes()), Positions::Apply),
            [
                InputAction::ButtonUp(PointerButton::Left),
                InputAction::Scroll {
                    vertical: 1,
                    horizontal: 0
                }
            ]
        );
    }

    /// Positions computed before a warp are ignored while buttons apply, and
    /// the next current report moves the pointer.
    #[test]
    fn ignores_positions_before_a_warp() {
        let mut input = devices();
        let report = PointerReport {
            buttons: PointerButtons::LEFT,
            x: 10 * 256,
            y: 20 * 256,
            wheel_total: 0,
            pan_total: 0,
        };
        assert_eq!(
            input.apply(&event(POINTER, report.to_bytes()), Positions::Ignore),
            [InputAction::ButtonDown(PointerButton::Left)]
        );
        assert_eq!(
            input.apply(&event(POINTER, report.to_bytes()), Positions::Apply),
            [InputAction::PointerMoveTo {
                x: 10 * 256,
                y: 20 * 256
            }]
        );
    }

    /// Mouse reports move by the change in motion totals, and detaching
    /// releases held buttons.
    #[test]
    fn mouse_differences() {
        let mut input = devices();
        let report = MouseReport {
            buttons: PointerButtons::RIGHT,
            motion_x_total: 512,
            motion_y_total: -256,
            wheel_total: 0,
            pan_total: 0,
        };
        assert_eq!(
            input.apply(&event(MOUSE, report.to_bytes()), Positions::Apply),
            [
                InputAction::MouseMoveBy { dx: 512, dy: -256 },
                InputAction::ButtonDown(PointerButton::Right)
            ]
        );
        assert_eq!(
            input.detach(MOUSE),
            [InputAction::ButtonUp(PointerButton::Right)]
        );
        assert!(
            input
                .apply(&event(MOUSE, report.to_bytes()), Positions::Apply)
                .is_empty()
        );
        assert!(
            input
                .apply(&event(KEYBOARD, vec![1, 2]), Positions::Apply)
                .is_empty()
        );
    }
}
