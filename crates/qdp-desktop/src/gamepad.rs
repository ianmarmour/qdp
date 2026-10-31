//! The client's gamepads, through gilrs: evdev on Linux, IOKit on macOS and
//! Windows.Gaming.Input on Windows.
//!
//! A thread of its own watches the gamepads, attaching each one the user
//! connects as a standard `GAMEPAD` device (Section 8.4.7) and sending its
//! state while the window has focus, and plays the rumble the host sends
//! back.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use gilrs::ff::{BaseEffect, BaseEffectType, Effect, EffectBuilder, Repeat, Replay, Ticks};
use gilrs::{Axis, Button, EventType, Gamepad, GamepadId, Gilrs};
use qdp::message::{DeviceFeatures, GamepadButtons, GamepadOutput, GamepadReport};
use qdp_runtime::{GamepadFeedback, InputHandle, UserInput};
use tracing::{debug, info, warn};

/// The name of the thread watching the gamepads.
const THREAD_NAME: &str = "qdp-gamepads";

/// The longest the thread waits for a gamepad event before checking for
/// rumble and whether to stop.
const POLL_INTERVAL: Duration = Duration::from_millis(4);

/// How long one play of a rumble effect lasts. The effect repeats until the
/// host's rumble changes.
const RUMBLE_PLAY_MILLISECONDS: u32 = 1000;

/// The USB vendor ID of Sony, whose controllers have the PlayStation layout.
const SONY_VENDOR_ID: u16 = 0x054c;

/// The USB vendor ID of Nintendo, whose controllers have the Nintendo layout.
const NINTENDO_VENDOR_ID: u16 = 0x057e;

/// The buttons gilrs reports, with their Standard Gamepad button numbers.
/// gilrs names the shoulder buttons triggers and the analog triggers second
/// triggers.
const BUTTONS: [(Button, u32); 17] = [
    (Button::South, 0),
    (Button::East, 1),
    (Button::West, 2),
    (Button::North, 3),
    (Button::LeftTrigger, 4),
    (Button::RightTrigger, 5),
    (Button::LeftTrigger2, 6),
    (Button::RightTrigger2, 7),
    (Button::Select, 8),
    (Button::Start, 9),
    (Button::LeftThumb, 10),
    (Button::RightThumb, 11),
    (Button::DPadUp, 12),
    (Button::DPadDown, 13),
    (Button::DPadLeft, 14),
    (Button::DPadRight, 15),
    (Button::Mode, 16),
];

/// The D-pad axes some gamepads report instead of buttons, with the Standard
/// Gamepad buttons of their negative and positive directions. gilrs's
/// vertical axes are positive upwards.
const DPAD_AXES: [(Axis, u32, u32); 2] = [(Axis::DPadX, 14, 15), (Axis::DPadY, 13, 12)];

/// How far a D-pad axis must be pushed to count as held.
const DPAD_THRESHOLD: f32 = 0.5;

/// What a gamepad's controls read.
trait Controls {
    /// Returns whether a button is held.
    fn is_pressed(&self, button: Button) -> bool;
    /// Returns how far a button is pressed, from 0 to 1.
    fn button_value(&self, button: Button) -> f32;
    /// Returns an axis's position, from -1 to 1.
    fn axis_value(&self, axis: Axis) -> f32;
}

impl Controls for Gamepad<'_> {
    fn is_pressed(&self, button: Button) -> bool {
        Gamepad::is_pressed(self, button)
    }

    fn button_value(&self, button: Button) -> f32 {
        self.button_data(button).map_or(0.0, |data| data.value())
    }

    fn axis_value(&self, axis: Axis) -> f32 {
        self.value(axis)
    }
}

/// Returns a value from 0 to 1 as a trigger's, from 0 to 65535.
fn trigger(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16
}

/// Returns a value from -1 to 1 as a stick's, from -32767 to 32767.
fn stick(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

/// Returns the report of a gamepad's controls. QDP's sticks are negative
/// upwards, gilrs's positive.
fn report(controls: &impl Controls) -> GamepadReport {
    let mut buttons = BUTTONS
        .iter()
        .filter(|(button, _)| controls.is_pressed(*button))
        .fold(0, |buttons, &(_, number)| {
            buttons | GamepadButtons::button(number).0
        });
    for (axis, negative, positive) in DPAD_AXES {
        let value = controls.axis_value(axis);
        if value <= -DPAD_THRESHOLD {
            buttons |= GamepadButtons::button(negative).0;
        } else if value >= DPAD_THRESHOLD {
            buttons |= GamepadButtons::button(positive).0;
        }
    }
    GamepadReport {
        buttons: GamepadButtons(buttons),
        left_trigger: trigger(controls.button_value(Button::LeftTrigger2)),
        right_trigger: trigger(controls.button_value(Button::RightTrigger2)),
        left_stick_x: stick(controls.axis_value(Axis::LeftStickX)),
        left_stick_y: stick(-controls.axis_value(Axis::LeftStickY)),
        right_stick_x: stick(controls.axis_value(Axis::RightStickX)),
        right_stick_y: stick(-controls.axis_value(Axis::RightStickY)),
        ..GamepadReport::default()
    }
}

/// Returns the features of a gamepad: rumble if it has motors, and its
/// button layout, judged by its maker.
fn features(gamepad: &Gamepad<'_>) -> DeviceFeatures {
    let mut features = DeviceFeatures::default();
    if gamepad.is_ff_supported() {
        features = features.union(DeviceFeatures::GAMEPAD_RUMBLE);
    }
    match gamepad.vendor_id() {
        Some(SONY_VENDOR_ID) => features.union(DeviceFeatures::GAMEPAD_PLAYSTATION_LAYOUT),
        Some(NINTENDO_VENDOR_ID) => features.union(DeviceFeatures::GAMEPAD_NINTENDO_LAYOUT),
        _ => features,
    }
}

/// Returns the slot a gamepad's input and feedback carry.
fn slot(id: GamepadId) -> u32 {
    u32::try_from(usize::from(id)).unwrap_or(u32::MAX)
}

/// The gamepads the thread watches.
struct Watcher {
    /// The gamepads.
    gilrs: Gilrs,
    /// Where the input goes.
    input: InputHandle,
    /// Whether the window has focus, so that the gamepads' state is sent.
    focused: Arc<AtomicBool>,
    /// The connected gamepads, by slot, with the rumble playing on each.
    gamepads: HashMap<u32, (GamepadId, Option<Effect>)>,
}

impl Watcher {
    /// Attaches a connected gamepad, if it is not attached yet. Returns
    /// `false` if the session has ended.
    fn connect(&mut self, id: GamepadId) -> bool {
        let slot = slot(id);
        if self.gamepads.contains_key(&slot) {
            return true;
        }
        let gamepad = self.gilrs.gamepad(id);
        info!(name = gamepad.name(), "gamepad connected");
        self.gamepads.insert(slot, (id, None));
        self.input.send(UserInput::GamepadConnected {
            slot,
            features: features(&gamepad),
            name: gamepad.name().to_owned(),
        }) && self.send_state(id)
    }

    /// Detaches a disconnected gamepad. Returns `false` if the session has
    /// ended.
    fn disconnect(&mut self, id: GamepadId) -> bool {
        let slot = slot(id);
        if self.gamepads.remove(&slot).is_none() {
            return true;
        }
        info!("gamepad disconnected");
        self.input.send(UserInput::GamepadDisconnected { slot })
    }

    /// Sends a gamepad's state while the window has focus. Returns `false` if
    /// the session has ended.
    fn send_state(&self, id: GamepadId) -> bool {
        if !self.focused.load(Ordering::Relaxed) {
            return true;
        }
        let report = report(&self.gilrs.gamepad(id));
        self.input.send(UserInput::Gamepad {
            slot: slot(id),
            report,
        })
    }

    /// Plays the rumble the host asked for on a gamepad, replacing what it
    /// played before.
    fn rumble(&mut self, slot: u32, output: GamepadOutput) {
        let Some((id, playing)) = self.gamepads.get_mut(&slot) else {
            return;
        };
        // Dropping an effect stops it.
        *playing = None;
        if !output.is_rumbling() {
            return;
        }
        let scheduling = Replay {
            play_for: Ticks::from_ms(RUMBLE_PLAY_MILLISECONDS),
            ..Replay::default()
        };
        let effect = EffectBuilder::new()
            .add_effect(BaseEffect {
                kind: BaseEffectType::Strong {
                    magnitude: output.low_frequency_rumble,
                },
                scheduling,
                ..BaseEffect::default()
            })
            .add_effect(BaseEffect {
                kind: BaseEffectType::Weak {
                    magnitude: output.high_frequency_rumble,
                },
                scheduling,
                ..BaseEffect::default()
            })
            .gamepads(&[*id])
            .repeat(Repeat::Infinitely)
            .finish(&mut self.gilrs);
        match effect.and_then(|effect| effect.play().map(|()| effect)) {
            Ok(effect) => *playing = Some(effect),
            Err(error) => debug!(%error, "the gamepad cannot rumble"),
        }
    }

    /// Watches the gamepads until `stop` is raised or the session ends.
    fn run(&mut self, stop: &AtomicBool) {
        let connected: Vec<GamepadId> = self.gilrs.gamepads().map(|(id, _)| id).collect();
        for id in connected {
            if !self.connect(id) {
                return;
            }
        }
        while !stop.load(Ordering::Relaxed) {
            while let Some(GamepadFeedback { slot, output }) = self.input.try_gamepad_feedback() {
                self.rumble(slot, output);
            }
            let Some(event) = self.gilrs.next_event_blocking(Some(POLL_INTERVAL)) else {
                continue;
            };
            let open = match event.event {
                EventType::Connected => self.connect(event.id),
                EventType::Disconnected => self.disconnect(event.id),
                EventType::Dropped | EventType::ForceFeedbackEffectCompleted => true,
                _ => self.send_state(event.id),
            };
            if !open {
                return;
            }
        }
    }
}

/// Watches the client's gamepads on a thread of its own until dropped.
#[derive(Debug)]
pub struct GamepadCapture {
    /// Whether the window has focus.
    focused: Arc<AtomicBool>,
    /// Raised to stop the thread.
    stop: Arc<AtomicBool>,
    /// The thread.
    thread: Option<JoinHandle<()>>,
}

impl GamepadCapture {
    /// Starts watching the gamepads, sending their input to `input`.
    pub fn start(input: InputHandle) -> Result<Self, String> {
        let focused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_focused = Arc::clone(&focused);
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                let gilrs = match Gilrs::new() {
                    Ok(gilrs) => gilrs,
                    Err(error) => {
                        warn!(%error, "gamepads are not available");
                        return;
                    }
                };
                let mut watcher = Watcher {
                    gilrs,
                    input,
                    focused: thread_focused,
                    gamepads: HashMap::new(),
                };
                watcher.run(&thread_stop);
            })
            .map_err(|error| format!("cannot watch the gamepads: {error}"))?;
        Ok(Self {
            focused,
            stop,
            thread: Some(thread),
        })
    }

    /// Tells the capture whether the window has focus: gamepads' state is
    /// sent only while it has.
    pub fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::Relaxed);
    }
}

impl Drop for GamepadCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Controls set by a test.
    #[derive(Debug, Default)]
    struct TestControls {
        /// The held buttons, with how far they are pressed.
        buttons: Vec<(Button, f32)>,
        /// The axes' positions.
        axes: Vec<(Axis, f32)>,
    }

    impl Controls for TestControls {
        fn is_pressed(&self, button: Button) -> bool {
            self.buttons.iter().any(|(held, _)| *held == button)
        }

        fn button_value(&self, button: Button) -> f32 {
            self.buttons
                .iter()
                .find(|(held, _)| *held == button)
                .map_or(0.0, |(_, value)| *value)
        }

        fn axis_value(&self, axis: Axis) -> f32 {
            self.axes
                .iter()
                .find(|(moved, _)| *moved == axis)
                .map_or(0.0, |(_, value)| *value)
        }
    }

    /// Buttons take their Standard Gamepad positions, triggers and sticks
    /// their ranges, and upwards becomes negative.
    #[test]
    fn reports_controls() {
        let controls = TestControls {
            buttons: vec![
                (Button::West, 1.0),
                (Button::LeftTrigger, 1.0),
                (Button::RightTrigger2, 0.5),
            ],
            axes: vec![
                (Axis::LeftStickY, 1.0),
                (Axis::RightStickX, -0.5),
                (Axis::DPadY, -1.0),
            ],
        };
        let sent = report(&controls);
        assert_eq!(
            sent.buttons,
            GamepadButtons(1 << 2 | 1 << 4 | 1 << 7 | 1 << 13)
        );
        assert_eq!(sent.right_trigger, 32_768);
        assert_eq!(sent.left_trigger, 0);
        assert_eq!(sent.left_stick_y, -32_767);
        assert_eq!(sent.right_stick_x, -16_384);
        assert_eq!(report(&TestControls::default()), GamepadReport::default());
    }

    /// The capture starts and stops, with or without gamepads.
    #[test]
    fn starts_and_stops() {
        let (input, _queue) = qdp_runtime::input_channel();
        let capture = GamepadCapture::start(input).expect("the thread starts");
        capture.set_focused(true);
        std::thread::sleep(POLL_INTERVAL * 10);
        drop(capture);
    }
}
