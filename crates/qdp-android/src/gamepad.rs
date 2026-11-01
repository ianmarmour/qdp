//! Gamepads as the app reads them, from Android's key and motion events,
//! converted to QDP's reports.
//!
//! The app sends a gamepad's whole state on each change: its held buttons as
//! a mask of Standard Gamepad button indices, which a handheld's built-in
//! controls and a Bluetooth controller both map to, its sticks from -1 to 1,
//! negative left and up as Android reports them, and its triggers from 0 to
//! 1.

use qdp::message::{GamepadButtons, GamepadReport};

/// The buttons of the Standard Gamepad there are, from 0 to 16.
const STANDARD_BUTTONS: u32 = 17;

/// The largest value of a stick axis.
const STICK_MAX: f32 = i16::MAX as f32;

/// The largest value of a trigger.
const TRIGGER_MAX: f32 = u16::MAX as f32;

/// A gamepad's state as the app sends it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GamepadState {
    /// The held buttons: bit n is Standard Gamepad button n.
    pub buttons: u32,
    /// The left stick, each axis from -1 to 1, negative left and up.
    pub left_stick: (f32, f32),
    /// The right stick, likewise.
    pub right_stick: (f32, f32),
    /// The left trigger, from 0 released to 1.
    pub left_trigger: f32,
    /// The right trigger, likewise.
    pub right_trigger: f32,
}

impl GamepadState {
    /// Returns the state as QDP's report.
    pub fn report(&self) -> GamepadReport {
        let stick = |value: f32| (value.clamp(-1.0, 1.0) * STICK_MAX).round() as i16;
        let trigger = |value: f32| (value.clamp(0.0, 1.0) * TRIGGER_MAX).round() as u16;
        let known = (1u32 << STANDARD_BUTTONS) - 1;
        GamepadReport {
            buttons: GamepadButtons(self.buttons & known),
            left_trigger: trigger(self.left_trigger),
            right_trigger: trigger(self.right_trigger),
            left_stick_x: stick(self.left_stick.0),
            left_stick_y: stick(self.left_stick.1),
            right_stick_x: stick(self.right_stick.0),
            right_stick_y: stick(self.right_stick.1),
            gyro: [0; 3],
            accel: [0; 3],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Axes scale to their ranges, out-of-range values are kept in them,
    /// and buttons beyond the Standard Gamepad's are dropped.
    #[test]
    fn converts_states_to_reports() {
        let state = GamepadState {
            buttons: 0b1 | 1 << 16 | 1 << 20,
            left_stick: (-1.0, 0.5),
            right_stick: (2.0, -0.25),
            left_trigger: 1.0,
            right_trigger: -0.5,
        };
        let report = state.report();
        assert_eq!(report.buttons, GamepadButtons(0b1 | 1 << 16));
        assert_eq!((report.left_stick_x, report.left_stick_y), (-32767, 16384));
        assert_eq!((report.right_stick_x, report.right_stick_y), (32767, -8192));
        assert_eq!((report.left_trigger, report.right_trigger), (65535, 0));
    }
}
