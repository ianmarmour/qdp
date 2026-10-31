//! The client's side of input: what the user does in the client's window,
//! turned into standard device reports (Section 8.4) for the input sender,
//! which delivers them reliably (Section 8.6).

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use qdp::input::{DeviceRest, ReportKind};
use qdp::message::{
    DeviceAttach, DeviceFeatures, DeviceId, DeviceKind, FeatureOperation, FeatureRequestId,
    FeatureStatus, GamepadButtons, GamepadOutput, GamepadReport, KeyboardReport, MouseReport,
    PassthroughInfo, PenFlags, PenReport, PointerButtons, PointerKind, PointerReport, TextReport,
    TouchContactFlags, TouchscreenReport,
};
use qdp::wire::VarInt;
use tokio::sync::Notify;

use crate::client_passthrough::PassthroughDevices;
use crate::client_touch::Touchscreen;
use crate::host_input::{PointerButton, TouchPhase};

/// The keyboard's Device ID.
pub(crate) const KEYBOARD_ID: DeviceId = DeviceId(VarInt::from_u32(1));

/// The absolute pointer's Device ID.
pub(crate) const POINTER_ID: DeviceId = DeviceId(VarInt::from_u32(2));

/// The relative mouse's Device ID.
pub(crate) const MOUSE_ID: DeviceId = DeviceId(VarInt::from_u32(3));

/// The text device's Device ID.
pub(crate) const TEXT_ID: DeviceId = DeviceId(VarInt::from_u32(4));

/// The touchscreen's Device ID.
pub(crate) const TOUCHSCREEN_ID: DeviceId = DeviceId(VarInt::from_u32(5));

/// The pen's Device ID.
pub(crate) const PEN_ID: DeviceId = DeviceId(VarInt::from_u32(6));

/// Pen tilt and twist are sent in 1/100 degree (Section 8.4.5).
const CENTIDEGREES: f64 = 100.0;

/// The largest pen tilt, in degrees either way.
const MAX_TILT_DEGREES: f64 = 90.0;

/// The degrees in a full turn of the pen.
const FULL_TURN_DEGREES: f64 = 360.0;

/// The pressure the touchscreen and pen report for a full press.
const FULL_PRESSURE: f64 = 65_535.0;

/// The largest pen twist, in 1/100 degree.
const MAX_TWIST: u16 = 35_999;

/// Positions and motion are sent in 1/256 of a pixel or device count
/// (Section 1.5.6).
const SUBUNITS: f64 = 256.0;

/// The most consumer keys a keyboard report holds.
const CONSUMER_KEY_SLOTS: usize = 4;

/// Where a pen is and how it is held (Section 8.4.5).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PenInput {
    /// The horizontal position on the monitor, in pixels.
    pub x: f64,
    /// The vertical position on the monitor, in pixels.
    pub y: f64,
    /// Whether it is in range, touching with its tip or eraser, and has its
    /// barrel button pressed.
    pub flags: PenFlags,
    /// How hard the tip or eraser presses, from 0 to 1.
    pub pressure: f64,
    /// The tilt towards the right, in degrees from -90 to 90.
    pub tilt_x: f64,
    /// The tilt towards the user, in degrees from -90 to 90.
    pub tilt_y: f64,
    /// The rotation about its own axis, clockwise, in degrees from 0 to 360.
    pub twist: f64,
}

/// Something the user did.
#[derive(Debug, Clone, PartialEq)]
pub enum UserInput {
    /// A key on the Keyboard/Keypad page (0x07) was pressed or released.
    Key {
        /// The key's usage: its physical position, not its character.
        usage: u8,
        /// Whether it is now held.
        pressed: bool,
    },
    /// A key on the Consumer page (0x0C), such as a volume key, was pressed or
    /// released.
    ConsumerKey {
        /// The key's usage.
        usage: u16,
        /// Whether it is now held.
        pressed: bool,
    },
    /// The pointer moved to a position on the monitor, in pixels.
    PointerMoved {
        /// The horizontal position.
        x: f64,
        /// The vertical position.
        y: f64,
    },
    /// A button was pressed or released.
    Button {
        /// The button.
        button: PointerButton,
        /// Whether it is now held.
        pressed: bool,
    },
    /// The user scrolled, in 1/120 of a wheel detent.
    Scroll {
        /// Vertical scrolling; positive is away from the user.
        vertical: i32,
        /// Horizontal scrolling; positive is to the right.
        horizontal: i32,
    },
    /// The mouse moved by a relative amount, in device counts.
    MouseMoved {
        /// The horizontal motion.
        dx: f64,
        /// The vertical motion.
        dy: f64,
    },
    /// Text was committed without key presses, as by an input method.
    Text(String),
    /// A finger touched, moved on, or left the monitor.
    Touch {
        /// Identifies the finger while it touches.
        finger: u64,
        /// What it did.
        phase: TouchPhase,
        /// The horizontal position on the monitor, in pixels.
        x: f64,
        /// The vertical position on the monitor, in pixels.
        y: f64,
        /// How hard it presses, from 0 to 1, if the screen senses it.
        pressure: Option<f64>,
    },
    /// The pen moved, came into or left range, or touched or lifted.
    Pen(PenInput),
    /// A gamepad was connected; `slot` names it in later input.
    GamepadConnected {
        /// The gamepad's number, unique while it is connected.
        slot: u32,
        /// What it can do, such as rumble.
        features: DeviceFeatures,
        /// Its name.
        name: String,
    },
    /// A gamepad was disconnected.
    GamepadDisconnected {
        /// The gamepad's number.
        slot: u32,
    },
    /// A gamepad's controls changed.
    Gamepad {
        /// The gamepad's number.
        slot: u32,
        /// Its complete state.
        report: GamepadReport,
    },
    /// A device the user chose to pass through was connected; `slot` names
    /// it in later input (Section 8.2.5).
    PassthroughConnected {
        /// The device's number, unique while it is connected.
        slot: u32,
        /// Its product name.
        name: String,
        /// Its bus, IDs, report descriptor and rest reports.
        info: PassthroughInfo,
    },
    /// A passed-through device was disconnected.
    PassthroughDisconnected {
        /// The device's number.
        slot: u32,
    },
    /// A passed-through device produced an input report.
    PassthroughReport {
        /// The device's number.
        slot: u32,
        /// The report as the device produced it, starting with its report ID
        /// byte, 0 if its descriptor uses no report IDs.
        report: Vec<u8>,
    },
    /// The outcome of a feature report operation the host asked for
    /// (Section 8.8.3).
    FeatureResponse {
        /// The request answered.
        request_id: FeatureRequestId,
        /// Whether it succeeded.
        status: FeatureStatus,
        /// The whole report, starting with its report ID byte, for a
        /// successful read; otherwise empty.
        report: Vec<u8>,
    },
    /// Everything held is released, as when the window loses focus.
    ReleaseAll,
}

/// A gamepad's rumble and LED, from the host's applications (Section 8.4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GamepadFeedback {
    /// The gamepad's number.
    pub slot: u32,
    /// The output to apply.
    pub output: GamepadOutput,
}

/// What the session sends back to passed-through devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PassthroughFeedback {
    /// An output report for a passed-through device, to write to it in order
    /// without delaying anything else (Section 8.8.2).
    Output {
        /// The device's number.
        slot: u32,
        /// The report, starting with its report ID byte.
        report: Vec<u8>,
    },
    /// A feature report operation the host's applications asked of a
    /// passed-through device, to answer with
    /// [`UserInput::FeatureResponse`] (Section 8.8.3).
    FeatureRequest {
        /// The device's number.
        slot: u32,
        /// The request, to answer with.
        request_id: FeatureRequestId,
        /// Read or write.
        operation: FeatureOperation,
        /// The report ID byte for a read, or the whole report for a write.
        report: Vec<u8>,
    },
}

/// Returns a handle for the user interface to send input with and the queue
/// the session reads it from.
pub fn input_channel() -> (InputHandle, InputQueue) {
    let (sender, receiver) = mpsc::channel();
    let (gamepad_sender, gamepad_feedback) = mpsc::channel();
    let (passthrough_sender, passthrough_feedback) = mpsc::channel();
    let wake = Arc::new(Notify::new());
    (
        InputHandle {
            sender,
            wake: Arc::clone(&wake),
            gamepad_feedback: Arc::new(Mutex::new(gamepad_feedback)),
            passthrough_feedback: Arc::new(Mutex::new(passthrough_feedback)),
        },
        InputQueue {
            receiver,
            wake,
            gamepad_feedback: gamepad_sender,
            passthrough_feedback: passthrough_sender,
        },
    )
}

/// Sends the user's input to the session, from any thread.
#[derive(Debug, Clone)]
pub struct InputHandle {
    /// The queue.
    sender: Sender<UserInput>,
    /// Wakes the session.
    wake: Arc<Notify>,
    /// What the session sends back to gamepads.
    gamepad_feedback: Arc<Mutex<Receiver<GamepadFeedback>>>,
    /// What the session sends back to passed-through devices.
    passthrough_feedback: Arc<Mutex<Receiver<PassthroughFeedback>>>,
}

impl InputHandle {
    /// Sends one input, returning `false` if the session has ended.
    pub fn send(&self, input: UserInput) -> bool {
        let sent = self.sender.send(input).is_ok();
        self.wake.notify_one();
        sent
    }

    /// Returns the next feedback for the user's gamepads, if any is waiting.
    pub fn try_gamepad_feedback(&self) -> Option<GamepadFeedback> {
        self.gamepad_feedback.lock().ok()?.try_recv().ok()
    }

    /// Returns the next feedback for passed-through devices, if any is
    /// waiting.
    pub fn try_passthrough_feedback(&self) -> Option<PassthroughFeedback> {
        self.passthrough_feedback.lock().ok()?.try_recv().ok()
    }
}

/// The user's input, waiting for the session.
#[derive(Debug)]
pub struct InputQueue {
    /// The queue.
    receiver: Receiver<UserInput>,
    /// Notified when input arrives.
    wake: Arc<Notify>,
    /// Sends feedback to gamepads.
    gamepad_feedback: Sender<GamepadFeedback>,
    /// Sends feedback to passed-through devices.
    passthrough_feedback: Sender<PassthroughFeedback>,
}

impl InputQueue {
    /// Sends feedback to the user's gamepads.
    pub(crate) fn send_gamepad_feedback(&self, feedback: GamepadFeedback) {
        let _ = self.gamepad_feedback.send(feedback);
    }

    /// Sends feedback to passed-through devices.
    pub(crate) fn send_passthrough_feedback(&self, feedback: PassthroughFeedback) {
        let _ = self.passthrough_feedback.send(feedback);
    }

    /// Returns the next input waiting, if any.
    pub(crate) fn try_next(&self) -> Option<UserInput> {
        // An empty queue and a closed one both mean nothing is waiting.
        self.receiver.try_recv().ok()
    }

    /// Returns the signal notified when input arrives.
    pub(crate) fn wake(&self) -> &Notify {
        &self.wake
    }
}

/// A report for the input sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingReport {
    /// The device it belongs to.
    pub(crate) device_id: DeviceId,
    /// The report data.
    pub(crate) data: Vec<u8>,
    /// Whether it changes discrete state or only moves.
    pub(crate) kind: ReportKind,
    /// Whether the device is at rest afterwards.
    pub(crate) rest: DeviceRest,
}

/// The state of the client's standard devices.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClientDevices {
    /// The keyboard's held keys.
    keyboard: KeyboardReport,
    /// The pointer's state.
    pointer: PointerReport,
    /// The mouse's state.
    mouse: MouseReport,
    /// Mouse motion not yet sent, below one subunit.
    mouse_remainder: (f64, f64),
    /// Whether buttons and scrolling go to the pointer or the mouse.
    pointer_mode: PointerKind,
    /// The monitor's top-left corner on the host's desktop, in pixels.
    monitor_origin: (i32, i32),
    /// The fingers touching the monitor.
    touchscreen: Touchscreen,
    /// The pen's state.
    pen: PenReport,
    /// The connected gamepads, by slot: their Device IDs and states.
    gamepads: HashMap<u32, (DeviceId, GamepadReport)>,
    /// The passed-through devices.
    passthrough: PassthroughDevices,
    /// The Device ID of the last gamepad or passthrough device attached.
    last_device_id: u64,
}

impl Default for ClientDevices {
    fn default() -> Self {
        Self {
            keyboard: KeyboardReport::default(),
            pointer: PointerReport::default(),
            mouse: MouseReport::default(),
            mouse_remainder: (0.0, 0.0),
            pointer_mode: PointerKind::Absolute,
            monitor_origin: (0, 0),
            touchscreen: Touchscreen::default(),
            pen: PenReport::default(),
            gamepads: HashMap::new(),
            passthrough: PassthroughDevices::default(),
            last_device_id: FIRST_DYNAMIC_ID - 1,
        }
    }
}

/// The Device ID of the first gamepad or passthrough device; the standard
/// devices use lower ones.
const FIRST_DYNAMIC_ID: u64 = 16;

/// The Standard Gamepad buttons the triggers set (Section 8.4.7).
const TRIGGER_BUTTONS: [u32; 2] = [6, 7];

/// The trigger value from which its button is held.
const TRIGGER_HELD: u16 = 32_768;

impl ClientDevices {
    /// Starts tracking a connected gamepad, returning the `DEVICE_ATTACH`
    /// that offers it to the host.
    pub(crate) fn connect_gamepad(
        &mut self,
        slot: u32,
        features: DeviceFeatures,
        name: String,
    ) -> DeviceAttach {
        let device_id = self.next_device_id();
        self.gamepads
            .insert(slot, (device_id, GamepadReport::default()));
        DeviceAttach {
            device_id,
            kind: DeviceKind::GAMEPAD,
            features,
            name,
            touchpad: None,
            passthrough: None,
        }
    }

    /// Returns the Device ID of the next device the user connects: they are
    /// never reused within a session (Section 8.2.1).
    fn next_device_id(&mut self) -> DeviceId {
        self.last_device_id += 1;
        DeviceId(VarInt::new(self.last_device_id).unwrap_or(VarInt::MAX))
    }

    /// Starts tracking a passthrough device, returning the `DEVICE_ATTACH`
    /// that offers it to the host.
    pub(crate) fn connect_passthrough(
        &mut self,
        slot: u32,
        name: String,
        info: PassthroughInfo,
    ) -> DeviceAttach {
        let device_id = self.next_device_id();
        self.passthrough.connect(slot, device_id, name, info)
    }

    /// Stops tracking a passthrough device, returning its Device ID to
    /// detach.
    pub(crate) fn disconnect_passthrough(&mut self, slot: u32) -> Option<DeviceId> {
        self.passthrough.disconnect(slot)
    }

    /// Returns the slot of a passthrough device's Device ID.
    pub(crate) fn passthrough_slot(&self, device_id: DeviceId) -> Option<u32> {
        self.passthrough.slot(device_id)
    }

    /// Returns the reports a passthrough device's state needs once the host
    /// accepts it.
    pub(crate) fn passthrough_state(&self, device_id: DeviceId) -> Vec<PendingReport> {
        self.passthrough.state(device_id)
    }

    /// Stops tracking a gamepad, returning its Device ID to detach.
    pub(crate) fn disconnect_gamepad(&mut self, slot: u32) -> Option<DeviceId> {
        self.gamepads.remove(&slot).map(|(device_id, _)| device_id)
    }

    /// Returns the slot of a gamepad's Device ID.
    pub(crate) fn gamepad_slot(&self, device_id: DeviceId) -> Option<u32> {
        self.gamepads
            .iter()
            .find(|(_, (id, _))| *id == device_id)
            .map(|(&slot, _)| slot)
    }

    /// Returns the report of a gamepad's current state if any of its
    /// controls is away from rest: the reports made before the host accepted
    /// the gamepad were not sent.
    pub(crate) fn gamepad_state(&self, device_id: DeviceId) -> Option<PendingReport> {
        let (_, report) = self.gamepads.values().find(|(id, _)| *id == device_id)?;
        (*report != GamepadReport::default())
            .then(|| Self::gamepad_report(device_id, &GamepadReport::default(), report))
    }

    /// Returns a gamepad's report, with its trigger buttons set exactly when
    /// the triggers are pressed past half (Section 8.4.7).
    fn gamepad_report(
        device_id: DeviceId,
        previous: &GamepadReport,
        report: &GamepadReport,
    ) -> PendingReport {
        let kind = if report.buttons == previous.buttons {
            ReportKind::Motion
        } else {
            ReportKind::Transition
        };
        PendingReport {
            device_id,
            data: report.to_bytes(),
            kind,
            rest: rest(*report != GamepadReport::default()),
        }
    }

    /// Returns a report with the trigger buttons following the triggers.
    fn with_trigger_buttons(mut report: GamepadReport) -> GamepadReport {
        for (button, value) in TRIGGER_BUTTONS
            .into_iter()
            .zip([report.left_trigger, report.right_trigger])
        {
            let mask = GamepadButtons::button(button).0;
            if value >= TRIGGER_HELD {
                report.buttons.0 |= mask;
            } else {
                report.buttons.0 &= !mask;
            }
        }
        report
    }

    /// Switches to the mode `POINTER_MODE` announced (Section 8.9), returning
    /// the reports that move held buttons to the new mode's device: first the
    /// new device holding them, then the old one releasing them (Section 8.6,
    /// rule 8).
    pub(crate) fn set_pointer_mode(&mut self, mode: PointerKind) -> Vec<PendingReport> {
        let relative = |kind: PointerKind| kind == PointerKind::Relative;
        if relative(mode) == relative(self.pointer_mode) {
            self.pointer_mode = mode;
            return Vec::new();
        }
        let held = std::mem::take(match self.pointer_mode {
            PointerKind::Relative => &mut self.mouse.buttons,
            _ => &mut self.pointer.buttons,
        });
        self.pointer_mode = mode;
        if held == PointerButtons::default() {
            return Vec::new();
        }
        match mode {
            PointerKind::Relative => {
                self.mouse.buttons = held;
                vec![
                    self.mouse_report(ReportKind::Transition),
                    self.pointer_report(ReportKind::Transition),
                ]
            }
            _ => {
                self.pointer.buttons = held;
                vec![
                    self.pointer_report(ReportKind::Transition),
                    self.mouse_report(ReportKind::Transition),
                ]
            }
        }
    }

    /// Sets the desktop position of the monitor the window shows.
    pub(crate) fn set_monitor_origin(&mut self, x: i32, y: i32) {
        self.monitor_origin = (x, y);
    }

    /// Returns the reports one input produces.
    pub(crate) fn reports(&mut self, input: UserInput) -> Vec<PendingReport> {
        match input {
            UserInput::Key { usage, pressed } => {
                if self.keyboard.is_held(usage) == pressed {
                    return Vec::new();
                }
                self.keyboard.set_held(usage, pressed);
                vec![self.keyboard_report()]
            }
            UserInput::ConsumerKey { usage, pressed } => {
                let slots = &mut self.keyboard.consumer_keys;
                let held = slots.contains(&usage);
                if usage == 0 || held == pressed {
                    return Vec::new();
                }
                if pressed {
                    let Some(free) = slots.iter_mut().find(|slot| **slot == 0) else {
                        return Vec::new();
                    };
                    *free = usage;
                } else if let Some(slot) = slots.iter_mut().find(|slot| **slot == usage) {
                    *slot = 0;
                }
                debug_assert_eq!(slots.len(), CONSUMER_KEY_SLOTS);
                vec![self.keyboard_report()]
            }
            UserInput::PointerMoved { x, y } => {
                let (x, y) = self.desktop_position(x, y);
                if (x, y) == (self.pointer.x, self.pointer.y) {
                    return Vec::new();
                }
                self.pointer.x = x;
                self.pointer.y = y;
                vec![self.pointer_report(ReportKind::Motion)]
            }
            UserInput::Button { button, pressed } => {
                let buttons = match self.pointer_mode {
                    PointerKind::Relative => &mut self.mouse.buttons,
                    _ => &mut self.pointer.buttons,
                };
                let mask = button.mask().0;
                let updated = if pressed {
                    buttons.0 | mask
                } else {
                    buttons.0 & !mask
                };
                if updated == buttons.0 {
                    return Vec::new();
                }
                buttons.0 = updated;
                vec![self.pointing_report(ReportKind::Transition)]
            }
            UserInput::Scroll {
                vertical,
                horizontal,
            } => {
                if vertical == 0 && horizontal == 0 {
                    return Vec::new();
                }
                let (wheel, pan) = match self.pointer_mode {
                    PointerKind::Relative => {
                        (&mut self.mouse.wheel_total, &mut self.mouse.pan_total)
                    }
                    _ => (&mut self.pointer.wheel_total, &mut self.pointer.pan_total),
                };
                *wheel = wheel.wrapping_add(vertical);
                *pan = pan.wrapping_add(horizontal);
                vec![self.pointing_report(ReportKind::Motion)]
            }
            UserInput::MouseMoved { dx, dy } => {
                let x = self.mouse_remainder.0 + dx * SUBUNITS;
                let y = self.mouse_remainder.1 + dy * SUBUNITS;
                let (whole_x, whole_y) = (x.trunc(), y.trunc());
                self.mouse_remainder = (x - whole_x, y - whole_y);
                if whole_x == 0.0 && whole_y == 0.0 {
                    return Vec::new();
                }
                self.mouse.motion_x_total = self.mouse.motion_x_total.wrapping_add(whole_x as i32);
                self.mouse.motion_y_total = self.mouse.motion_y_total.wrapping_add(whole_y as i32);
                vec![self.mouse_report(ReportKind::Motion)]
            }
            UserInput::Text(text) => TextReport::split(&text)
                .into_iter()
                .map(|report| PendingReport {
                    device_id: TEXT_ID,
                    data: report.to_bytes(),
                    kind: ReportKind::Transition,
                    rest: DeviceRest::AtRest,
                })
                .collect(),
            UserInput::Touch {
                finger,
                phase,
                x,
                y,
                pressure,
            } => {
                let pressure = (pressure.unwrap_or(0.0).clamp(0.0, 1.0) * FULL_PRESSURE) as u16;
                let position = self.desktop_position(x, y);
                self.touchscreen
                    .touch(finger, phase, position, pressure)
                    .map(|(report, kind)| Self::touchscreen_report(&report, kind))
                    .into_iter()
                    .collect()
            }
            UserInput::Pen(pen) => {
                let (x, y) = self.desktop_position(pen.x, pen.y);
                let tilt = |degrees: f64| {
                    (degrees.clamp(-MAX_TILT_DEGREES, MAX_TILT_DEGREES) * CENTIDEGREES).round()
                        as i16
                };
                let twist = pen.twist.rem_euclid(FULL_TURN_DEGREES) * CENTIDEGREES;
                let report = PenReport {
                    flags: pen.flags,
                    x,
                    y,
                    pressure: (pen.pressure.clamp(0.0, 1.0) * FULL_PRESSURE) as u16,
                    tilt_x: tilt(pen.tilt_x),
                    tilt_y: tilt(pen.tilt_y),
                    twist: (twist as u16).min(MAX_TWIST),
                };
                if report == self.pen {
                    return Vec::new();
                }
                let kind = if report.flags == self.pen.flags {
                    ReportKind::Motion
                } else {
                    ReportKind::Transition
                };
                self.pen = report;
                vec![self.pen_report(kind)]
            }
            UserInput::PassthroughReport { slot, report } => {
                self.passthrough.report(slot, report).into_iter().collect()
            }
            UserInput::ReleaseAll => {
                let mut reports = self.passthrough.release_all();
                if self.pen.flags != PenFlags::default() {
                    self.pen.flags = PenFlags::default();
                    self.pen.pressure = 0;
                    reports.push(self.pen_report(ReportKind::Transition));
                }
                if let Some(report) = self.touchscreen.cancel_all() {
                    reports.push(Self::touchscreen_report(&report, ReportKind::Transition));
                }
                if self.keyboard != KeyboardReport::default() {
                    self.keyboard = KeyboardReport::default();
                    reports.push(self.keyboard_report());
                }
                if self.pointer.buttons != PointerButtons::default() {
                    self.pointer.buttons = PointerButtons::default();
                    reports.push(self.pointer_report(ReportKind::Transition));
                }
                if self.mouse.buttons != PointerButtons::default() {
                    self.mouse.buttons = PointerButtons::default();
                    reports.push(self.mouse_report(ReportKind::Transition));
                }
                for (device_id, state) in self.gamepads.values_mut() {
                    if *state != GamepadReport::default() {
                        let previous = std::mem::take(state);
                        reports.push(Self::gamepad_report(*device_id, &previous, state));
                    }
                }
                reports
            }
            UserInput::Gamepad { slot, report } => {
                let Some((device_id, state)) = self.gamepads.get_mut(&slot) else {
                    return Vec::new();
                };
                let report = Self::with_trigger_buttons(report);
                if report == *state {
                    return Vec::new();
                }
                let previous = std::mem::replace(state, report);
                vec![Self::gamepad_report(*device_id, &previous, state)]
            }
            // Gamepads and passthrough devices come and go through
            // `DEVICE_ATTACH` and `DEVICE_DETACH`, and feature responses are
            // control messages, not reports.
            UserInput::GamepadConnected { .. }
            | UserInput::GamepadDisconnected { .. }
            | UserInput::PassthroughConnected { .. }
            | UserInput::PassthroughDisconnected { .. }
            | UserInput::FeatureResponse { .. } => Vec::new(),
        }
    }

    /// Returns a position on the monitor, in pixels, as a desktop position in
    /// 1/256 pixel.
    fn desktop_position(&self, x: f64, y: f64) -> (i32, i32) {
        (
            ((f64::from(self.monitor_origin.0) + x) * SUBUNITS).round() as i32,
            ((f64::from(self.monitor_origin.1) + y) * SUBUNITS).round() as i32,
        )
    }

    /// Returns a touchscreen report for the input sender. The touchscreen is
    /// at rest once no contact remains, cancelled ones included.
    fn touchscreen_report(report: &TouchscreenReport, kind: ReportKind) -> PendingReport {
        let touching = report
            .contacts
            .iter()
            .any(|contact| !contact.flags.contains(TouchContactFlags::CANCELLED));
        PendingReport {
            device_id: TOUCHSCREEN_ID,
            data: report.to_bytes(),
            kind,
            rest: rest(touching),
        }
    }

    /// Returns the pen's report of its current state. The pen holds
    /// something while its tip, eraser or barrel button is down.
    fn pen_report(&self, kind: ReportKind) -> PendingReport {
        let holding = [
            PenFlags::TIP_SWITCH,
            PenFlags::ERASER,
            PenFlags::BARREL_SWITCH,
        ]
        .into_iter()
        .any(|flag| self.pen.flags.contains(flag));
        PendingReport {
            device_id: PEN_ID,
            data: self.pen.to_bytes(),
            kind,
            rest: rest(holding),
        }
    }

    /// Returns the keyboard's report of its current state.
    fn keyboard_report(&self) -> PendingReport {
        PendingReport {
            device_id: KEYBOARD_ID,
            data: self.keyboard.to_bytes(),
            kind: ReportKind::Transition,
            rest: rest(self.keyboard != KeyboardReport::default()),
        }
    }

    /// Returns the pointer's report of its current state.
    fn pointer_report(&self, kind: ReportKind) -> PendingReport {
        PendingReport {
            device_id: POINTER_ID,
            data: self.pointer.to_bytes(),
            kind,
            rest: rest(self.pointer.buttons != PointerButtons::default()),
        }
    }

    /// Returns the mouse's report of its current state.
    fn mouse_report(&self, kind: ReportKind) -> PendingReport {
        PendingReport {
            device_id: MOUSE_ID,
            data: self.mouse.to_bytes(),
            kind,
            rest: rest(self.mouse.buttons != PointerButtons::default()),
        }
    }

    /// Returns the report of the device buttons and scrolling go to.
    fn pointing_report(&self, kind: ReportKind) -> PendingReport {
        match self.pointer_mode {
            PointerKind::Relative => self.mouse_report(kind),
            _ => self.pointer_report(kind),
        }
    }
}

/// Returns whether a device holding something is at rest.
fn rest(holding: bool) -> DeviceRest {
    if holding {
        DeviceRest::Active
    } else {
        DeviceRest::AtRest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys produce keyboard reports, once per change.
    #[test]
    fn keys() {
        let mut devices = ClientDevices::default();
        let reports = devices.reports(UserInput::Key {
            usage: 0x04,
            pressed: true,
        });
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].device_id, KEYBOARD_ID);
        assert_eq!(reports[0].rest, DeviceRest::Active);
        assert!(
            KeyboardReport::decode(&reports[0].data)
                .unwrap()
                .is_held(0x04)
        );
        assert!(
            devices
                .reports(UserInput::Key {
                    usage: 0x04,
                    pressed: true
                })
                .is_empty()
        );
        let released = devices.reports(UserInput::ReleaseAll);
        assert_eq!(released[0].rest, DeviceRest::AtRest);
        assert!(devices.reports(UserInput::ReleaseAll).is_empty());
    }

    /// Pointer positions are desktop positions in 1/256 pixel, and buttons and
    /// scrolling go to the pointer in absolute mode and the mouse in relative
    /// mode.
    #[test]
    fn pointer_and_mouse() {
        let mut devices = ClientDevices::default();
        devices.set_monitor_origin(1920, 0);
        let moved = devices.reports(UserInput::PointerMoved { x: 10.5, y: 2.0 });
        let report = PointerReport::decode(&moved[0].data).unwrap();
        assert_eq!((report.x, report.y), ((1920 * 256) + 2688, 512));
        let pressed = devices.reports(UserInput::Button {
            button: PointerButton::Left,
            pressed: true,
        });
        assert_eq!(pressed[0].device_id, POINTER_ID);
        assert_eq!(pressed[0].kind, ReportKind::Transition);

        // The held button moves to the mouse.
        assert_eq!(devices.set_pointer_mode(PointerKind::Relative).len(), 2);
        let scrolled = devices.reports(UserInput::Scroll {
            vertical: 120,
            horizontal: -3,
        });
        let report = MouseReport::decode(&scrolled[0].data).unwrap();
        assert_eq!((report.wheel_total, report.pan_total), (120, -3));
        assert!(
            devices
                .reports(UserInput::MouseMoved { dx: 0.001, dy: 0.0 })
                .is_empty()
        );
        let moved = devices.reports(UserInput::MouseMoved { dx: 1.0, dy: -0.5 });
        let report = MouseReport::decode(&moved[0].data).unwrap();
        assert_eq!((report.motion_x_total, report.motion_y_total), (256, -128));
    }

    /// Gamepads get their own Device IDs, report only changes, set the
    /// trigger buttons from the triggers, and return to rest on release.
    #[test]
    fn gamepads() {
        let mut devices = ClientDevices::default();
        let attach = devices.connect_gamepad(3, DeviceFeatures::GAMEPAD_RUMBLE, "Pad".to_owned());
        assert_eq!(attach.kind, DeviceKind::GAMEPAD);
        assert_eq!(devices.gamepad_slot(attach.device_id), Some(3));
        assert_eq!(devices.gamepad_state(attach.device_id), None);
        let report = GamepadReport {
            left_trigger: 40_000,
            left_stick_y: -500,
            ..GamepadReport::default()
        };
        let reports = devices.reports(UserInput::Gamepad { slot: 3, report });
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].device_id, attach.device_id);
        assert_eq!(reports[0].kind, ReportKind::Transition);
        let sent = GamepadReport::decode(&reports[0].data).unwrap();
        assert!(sent.buttons.contains(GamepadButtons::button(6)));
        assert_eq!(
            devices
                .gamepad_state(attach.device_id)
                .map(|state| state.data),
            Some(sent.to_bytes())
        );
        assert!(
            devices
                .reports(UserInput::Gamepad { slot: 3, report })
                .is_empty()
        );
        assert!(
            devices
                .reports(UserInput::Gamepad { slot: 4, report })
                .is_empty()
        );
        let released = devices.reports(UserInput::ReleaseAll);
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].rest, DeviceRest::AtRest);
        assert_eq!(devices.disconnect_gamepad(3), Some(attach.device_id));
        assert_eq!(devices.gamepad_slot(attach.device_id), None);
    }

    /// Pens report desktop positions, tilt and twist in 1/100 degree, and
    /// leave range when everything is released.
    #[test]
    fn pen() {
        let mut devices = ClientDevices::default();
        let hovering = PenInput {
            x: 10.0,
            y: 20.0,
            flags: PenFlags::IN_RANGE,
            tilt_x: -30.5,
            twist: 361.0,
            ..PenInput::default()
        };
        let reports = devices.reports(UserInput::Pen(hovering));
        assert_eq!(reports[0].device_id, PEN_ID);
        assert_eq!(reports[0].rest, DeviceRest::AtRest);
        let report = PenReport::decode(&reports[0].data).unwrap();
        assert_eq!((report.x, report.tilt_x, report.twist), (2560, -3050, 100));
        assert!(devices.reports(UserInput::Pen(hovering)).is_empty());
        let touching = PenInput {
            flags: PenFlags(PenFlags::IN_RANGE.0 | PenFlags::TIP_SWITCH.0),
            pressure: 0.5,
            ..hovering
        };
        let reports = devices.reports(UserInput::Pen(touching));
        assert_eq!(reports[0].kind, ReportKind::Transition);
        assert_eq!(reports[0].rest, DeviceRest::Active);
        let released = devices.reports(UserInput::ReleaseAll);
        let report = PenReport::decode(&released[0].data).unwrap();
        assert_eq!((report.flags, report.pressure), (PenFlags::default(), 0));
        assert!(devices.reports(UserInput::ReleaseAll).is_empty());
    }

    /// Long text is split into several reports.
    #[test]
    fn text() {
        let mut devices = ClientDevices::default();
        let reports = devices.reports(UserInput::Text("é".repeat(200)));
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|report| report.device_id == TEXT_ID));
    }

    /// Switching modes moves held buttons to the new mode's device before the
    /// old one releases them.
    #[test]
    fn moves_held_buttons_between_modes() {
        let mut devices = ClientDevices::default();
        devices.reports(UserInput::Button {
            button: PointerButton::Left,
            pressed: true,
        });
        let reports = devices.set_pointer_mode(PointerKind::Relative);
        let devices_and_buttons: Vec<(DeviceId, u8)> = reports
            .iter()
            .map(|report| (report.device_id, report.data[0]))
            .collect();
        assert_eq!(devices_and_buttons, [(MOUSE_ID, 0x01), (POINTER_ID, 0x00)]);
        assert!(
            reports
                .iter()
                .all(|report| report.kind == ReportKind::Transition)
        );
        let back = devices.set_pointer_mode(PointerKind::Absolute);
        assert_eq!(back[0].device_id, POINTER_ID);
        assert_eq!(back[1].device_id, MOUSE_ID);
        assert!(devices.set_pointer_mode(PointerKind::Absolute).is_empty());
    }
}
