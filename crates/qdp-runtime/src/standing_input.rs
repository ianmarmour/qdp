//! The client's standing input: the input that describes its state rather
//! than an action, such as the devices connected and whether the session
//! has focus. Each session on an input queue starts from it, so that one
//! that resumes after a lost connection knows the client's devices, focus,
//! lock keys, microphone and display as they are now (Section 4.4.7).

use std::collections::{BTreeMap, VecDeque};

use crate::client::DisplaySize;
use crate::client_input::UserInput;

/// A connected device, by kind and slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum DeviceSlot {
    /// A gamepad.
    Gamepad(u32),
    /// A touchpad whose raw contacts are sent.
    Touchpad(u32),
    /// A passed-through device.
    Passthrough(u32),
}

/// The client's standing input, as the input passing through its queue
/// leaves it.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct StandingInput {
    /// The connection of each device still connected.
    devices: BTreeMap<DeviceSlot, UserInput>,
    /// The latest display.
    display: Option<DisplaySize>,
    /// The latest microphone choice.
    microphone: Option<UserInput>,
    /// The latest focus change.
    focus: Option<UserInput>,
    /// The lock keys as the session last gained keyboard focus.
    lock_keys: Option<UserInput>,
}

impl StandingInput {
    /// Notes `input`, which changes the standing input if it describes the
    /// client's state.
    pub(crate) fn note(&mut self, input: &UserInput) {
        let connected = match input {
            UserInput::GamepadConnected { slot, .. } => Some(DeviceSlot::Gamepad(*slot)),
            UserInput::TouchpadConnected { slot, .. } => Some(DeviceSlot::Touchpad(*slot)),
            UserInput::PassthroughConnected { slot, .. } => Some(DeviceSlot::Passthrough(*slot)),
            _ => None,
        };
        if let Some(device) = connected {
            self.devices.insert(device, input.clone());
            return;
        }
        match input {
            UserInput::GamepadDisconnected { slot } => {
                self.devices.remove(&DeviceSlot::Gamepad(*slot));
            }
            UserInput::TouchpadDisconnected { slot } => {
                self.devices.remove(&DeviceSlot::Touchpad(*slot));
            }
            UserInput::PassthroughDisconnected { slot } => {
                self.devices.remove(&DeviceSlot::Passthrough(*slot));
            }
            UserInput::Display(display) => self.display = Some(*display),
            UserInput::Microphone(_) => self.microphone = Some(input.clone()),
            UserInput::Focus(_) => self.focus = Some(input.clone()),
            UserInput::LockKeys(_) => self.lock_keys = Some(input.clone()),
            _ => {}
        }
    }

    /// Returns the latest display, if the client reported one.
    pub(crate) fn display(&self) -> Option<DisplaySize> {
        self.display
    }

    /// Returns the input that brings a new session to the standing input:
    /// the devices' connections, then the microphone, focus and lock keys.
    /// The display is not among it, since a session requests it first.
    pub(crate) fn replay(&self) -> VecDeque<UserInput> {
        self.devices
            .values()
            .chain(&self.microphone)
            .chain(&self.focus)
            .chain(&self.lock_keys)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::DeviceFeatures;

    use crate::audio::Microphone;
    use crate::clipboard::Focus;

    /// Returns the connection of the gamepad in `slot`.
    fn gamepad(slot: u32) -> UserInput {
        UserInput::GamepadConnected {
            slot,
            features: DeviceFeatures::default(),
            name: format!("Pad {slot}"),
        }
    }

    /// Only the devices still connected and the latest state are replayed,
    /// and actions are not.
    #[test]
    fn replays_the_state() {
        let mut standing = StandingInput::default();
        assert!(standing.replay().is_empty());
        let display = DisplaySize {
            width: 800,
            height: 600,
            refresh_millihertz: 60_000,
            scale_percent: DisplaySize::UNSCALED,
        };
        for input in [
            gamepad(1),
            gamepad(2),
            UserInput::Key {
                usage: 0x04,
                pressed: true,
            },
            UserInput::GamepadDisconnected { slot: 1 },
            UserInput::Focus(Focus::Unfocused),
            UserInput::Microphone(Microphone::On),
            UserInput::Focus(Focus::Focused),
            UserInput::Display(display),
        ] {
            standing.note(&input);
        }
        assert_eq!(standing.display(), Some(display));
        assert_eq!(
            Vec::from(standing.replay()),
            [
                gamepad(2),
                UserInput::Microphone(Microphone::On),
                UserInput::Focus(Focus::Focused),
            ]
        );
    }
}
