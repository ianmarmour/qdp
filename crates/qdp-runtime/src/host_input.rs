//! Turning the client's standard device reports into actions for the host's
//! input injector (Sections 8.4 and 8.7).
//!
//! Standard reports state each device's complete current state, so the host
//! compares every report with the last one it applied for the device and
//! performs only the differences: keys pressed and released, buttons, motion
//! and scrolling.

use std::collections::{BTreeMap, HashMap};

use qdp::input::{InputEvent, total_delta};
use qdp::message::{
    DeviceId, DeviceKind, FeatureStatus, GamepadOutput, GamepadReport, KeyboardInfo, KeyboardLeds,
    KeyboardReport, MouseReport, PenFlags, PenReport, PointerButtons, PointerKind, PointerReport,
    TextReport, TouchContact, TouchContactFlags, TouchpadInfo, TouchpadReport, TouchscreenReport,
};
use tracing::debug;

use crate::host_passthrough::{FeatureRequest, PassthroughDevice};

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

/// What happened to a touch contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TouchPhase {
    /// It started touching.
    Down,
    /// It moved, or stayed where it was while others changed.
    Move,
    /// It lifted.
    Up,
    /// It ended without effect, as after palm rejection or when the session
    /// lost focus.
    Cancel,
}

/// One contact in a touchscreen frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TouchPoint {
    /// Identifies the contact while it touches.
    pub id: u8,
    /// What happened to it.
    pub phase: TouchPhase,
    /// Its horizontal desktop position, in 1/256 pixel.
    pub x: i32,
    /// Its vertical desktop position, in 1/256 pixel.
    pub y: i32,
    /// Its pressure, from 0 to 65535, or 0 if the touchscreen does not sense
    /// pressure.
    pub pressure: u16,
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
    /// Sets a gamepad's controls, creating the gamepad on its first report.
    Gamepad {
        /// The gamepad.
        device_id: DeviceId,
        /// Its complete state.
        report: GamepadReport,
    },
    /// Removes a gamepad, as when the client detaches it.
    GamepadRemoved(DeviceId),
    /// Changes a touchscreen's contacts together: every contact that touched
    /// or touches, each with what happened to it.
    Touch(Vec<TouchPoint>),
    /// Sets the pen's state: where it is, whether it is in range or touching,
    /// and how it is held. Out of range, it has left the surface.
    Pen(PenReport),
    /// Delivers an input report of a passthrough device, starting with its
    /// report ID byte, as the physical device produced it.
    PassthroughReport {
        /// The device.
        device_id: DeviceId,
        /// The report.
        report: Vec<u8>,
    },
    /// Removes a passthrough device, as when the client detaches it.
    PassthroughRemoved(DeviceId),
    /// Sets a touchpad's buttons and contacts, creating the touchpad on its
    /// first report, for the host's own gestures (Section 8.4.6).
    Touchpad {
        /// The touchpad.
        device_id: DeviceId,
        /// The size of its surface.
        surface: TouchpadInfo,
        /// Whether its surface is a button.
        clickpad: bool,
        /// Its complete state.
        report: TouchpadReport,
    },
    /// Removes a touchpad, as when the client detaches it.
    TouchpadRemoved(DeviceId),
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

    /// Returns the pointer mode the host's applications call for (Section
    /// 8.9): relative while one has hidden and captured the pointer, as games
    /// do, and absolute otherwise.
    fn pointer_mode(&mut self) -> PointerKind {
        PointerKind::Absolute
    }

    /// Returns whether the injector can perform touches, so that the host
    /// accepts touchscreens.
    fn supports_touch(&self) -> bool {
        false
    }

    /// Returns whether the injector can perform a pen's input, so that the host
    /// accepts pens.
    fn supports_pen(&self) -> bool {
        false
    }

    /// Returns whether the injector can present passthrough devices to the
    /// host's applications, on a platform that confines them to the
    /// session's user or seat (Section 8.2.5).
    fn supports_passthrough(&self) -> bool {
        false
    }

    /// Presents a passthrough device to the host's applications, at rest.
    fn attach_passthrough(&mut self, device: &PassthroughDevice) -> Result<(), String> {
        let _ = device;
        Err("this host cannot present passthrough devices".to_owned())
    }

    /// Returns the output reports the host's applications sent passthrough
    /// devices since the last call, in order, each starting with its report
    /// ID byte.
    fn passthrough_outputs(&mut self) -> Vec<(DeviceId, Vec<u8>)> {
        Vec::new()
    }

    /// Returns the feature report operations the host's applications asked of
    /// passthrough devices since the last call.
    fn feature_requests(&mut self) -> Vec<FeatureRequest> {
        Vec::new()
    }

    /// Answers a feature report request with the client's outcome: the whole
    /// report, starting with its report ID byte, for a successful read.
    fn answer_feature(&mut self, token: u64, status: FeatureStatus, report: &[u8]) {
        let _ = (token, status, report);
    }

    /// Returns whether the injector can present raw touchpads, whose
    /// contacts the host's own gesture recognition reads, so that the host
    /// accepts touchpads.
    fn supports_touchpad(&self) -> bool {
        false
    }

    /// Returns whether the injector can present gamepads to the host's
    /// applications, so that the host accepts gamepad devices.
    fn supports_gamepads(&self) -> bool {
        false
    }

    /// Returns the gamepads whose rumble or LED the host's applications
    /// changed since the last call, with their new output.
    fn gamepad_outputs(&mut self) -> Vec<(DeviceId, GamepadOutput)> {
        Vec::new()
    }

    /// Selects the keyboard layout matching the client's, where the platform
    /// allows, so that each key produces the character printed on the user's
    /// keyboard (Section 8.2.3).
    fn set_keyboard_layout(&mut self, info: &KeyboardInfo) {
        let _ = info;
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
    /// A gamepad.
    Gamepad(GamepadReport),
    /// A touchscreen, with the contacts touching.
    Touchscreen(Vec<TouchContact>),
    /// A pen.
    Pen(PenReport),
    /// A touchpad.
    Touchpad {
        /// The size of its surface.
        surface: TouchpadInfo,
        /// Whether its surface is a button.
        clickpad: bool,
        /// Its last applied state.
        report: TouchpadReport,
    },
    /// A passthrough device, with its input reports at rest and as last
    /// applied, by report ID.
    Passthrough {
        /// The reports with every control at rest.
        rest: BTreeMap<u8, Vec<u8>>,
        /// The reports last applied.
        current: BTreeMap<u8, Vec<u8>>,
    },
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
            DeviceKind::GAMEPAD => DeviceState::Gamepad(GamepadReport::default()),
            DeviceKind::TOUCHSCREEN => DeviceState::Touchscreen(Vec::new()),
            DeviceKind::PEN => DeviceState::Pen(PenReport::default()),
            _ => return false,
        };
        self.devices.insert(device_id, state);
        true
    }

    /// Adds an accepted passthrough device, at rest, with its rest reports,
    /// each starting with its report ID byte.
    pub(crate) fn attach_passthrough(&mut self, device_id: DeviceId, rest_reports: &[Vec<u8>]) {
        let rest: BTreeMap<u8, Vec<u8>> = rest_reports
            .iter()
            .filter_map(|report| Some((*report.first()?, report.clone())))
            .collect();
        self.devices.insert(
            device_id,
            DeviceState::Passthrough {
                current: rest.clone(),
                rest,
            },
        );
    }

    /// Adds an accepted touchpad, at rest, with the size of its surface.
    pub(crate) fn attach_touchpad(
        &mut self,
        device_id: DeviceId,
        surface: TouchpadInfo,
        clickpad: bool,
    ) {
        self.devices.insert(
            device_id,
            DeviceState::Touchpad {
                surface,
                clickpad,
                report: TouchpadReport::default(),
            },
        );
    }

    /// Returns the accepted keyboards.
    pub(crate) fn keyboards(&self) -> Vec<DeviceId> {
        self.devices
            .iter()
            .filter(|(_, state)| matches!(state, DeviceState::Keyboard(_)))
            .map(|(&id, _)| id)
            .collect()
    }

    /// Returns whether any device holds something: a key, button, contact or
    /// gamepad control away from rest.
    pub(crate) fn is_holding(&self) -> bool {
        self.devices.values().any(holds)
    }

    /// Returns whether a device has been accepted.
    pub(crate) fn contains(&self, device_id: DeviceId) -> bool {
        self.devices.contains_key(&device_id)
    }

    /// Removes a device, returning the actions that release what it held and,
    /// for a gamepad, remove it.
    pub(crate) fn detach(&mut self, device_id: DeviceId) -> Vec<InputAction> {
        let Some(state) = self.devices.remove(&device_id) else {
            return Vec::new();
        };
        let mut actions = release(device_id, &state);
        match state {
            DeviceState::Gamepad(_) => actions.push(InputAction::GamepadRemoved(device_id)),
            DeviceState::Passthrough { .. } => {
                actions.push(InputAction::PassthroughRemoved(device_id));
            }
            DeviceState::Touchpad { .. } => actions.push(InputAction::TouchpadRemoved(device_id)),
            _ => {}
        }
        actions
    }

    /// Returns the actions that release everything every device holds, as when
    /// the session ends, and leaves the devices at rest.
    pub(crate) fn release_all(&mut self) -> Vec<InputAction> {
        let ids: Vec<DeviceId> = self.devices.keys().copied().collect();
        let mut actions = Vec::new();
        for id in ids {
            if let Some(state) = self.devices.get_mut(&id) {
                actions.extend(release(id, state));
                *state = at_rest(state);
            }
        }
        actions
    }

    /// Returns the actions a report calls for in pointer mode `mode`. A report
    /// that does not parse is skipped. In absolute mode the mouse's motion
    /// only updates its baseline, and in relative mode the pointer's
    /// positions are ignored; both apply buttons and scrolling (Section 8.7,
    /// rule 5).
    pub(crate) fn apply(
        &mut self,
        event: &InputEvent,
        positions: Positions,
        mode: PointerKind,
    ) -> Vec<InputAction> {
        let positions = match mode {
            PointerKind::Relative => Positions::Ignore,
            _ => positions,
        };
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
                if (dx != 0 || dy != 0) && mode == PointerKind::Relative {
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
            DeviceState::Gamepad(previous) => match GamepadReport::decode(&event.data) {
                Ok(report) if report != *previous => {
                    actions.push(InputAction::Gamepad {
                        device_id: event.device_id,
                        report,
                    });
                    *previous = report;
                }
                Ok(_) => {}
                Err(_) => debug!("malformed gamepad report skipped"),
            },
            DeviceState::Touchscreen(previous) => match TouchscreenReport::decode(&event.data) {
                Ok(report) => {
                    let points = touch_points(previous, &report.contacts);
                    if !points.is_empty() {
                        actions.push(InputAction::Touch(points));
                    }
                    *previous = report.contacts;
                    previous
                        .retain(|contact| !contact.flags.contains(TouchContactFlags::CANCELLED));
                }
                Err(_) => debug!("malformed touchscreen report skipped"),
            },
            DeviceState::Pen(previous) => match PenReport::decode(&event.data) {
                Ok(report) if report != *previous => {
                    actions.push(InputAction::Pen(report));
                    *previous = report;
                }
                Ok(_) => {}
                Err(_) => debug!("malformed pen report skipped"),
            },
            DeviceState::Touchpad {
                surface,
                clickpad,
                report: previous,
            } => match TouchpadReport::decode(&event.data) {
                Ok(report) if report != *previous => {
                    actions.push(InputAction::Touchpad {
                        device_id: event.device_id,
                        surface: *surface,
                        clickpad: *clickpad,
                        report: report.clone(),
                    });
                    *previous = report;
                }
                Ok(_) => {}
                Err(_) => debug!("malformed touchpad report skipped"),
            },
            DeviceState::Passthrough { current, .. } => {
                let Some(&report_id) = event.data.first() else {
                    debug!("empty passthrough report skipped");
                    return actions;
                };
                current.insert(report_id, event.data.clone());
                actions.push(InputAction::PassthroughReport {
                    device_id: event.device_id,
                    report: event.data.clone(),
                });
            }
        }
        actions
    }
}

/// Returns whether a device holds something: a key, button, contact, pen
/// tip or gamepad control away from rest. A pen merely hovering holds
/// nothing.
fn holds(state: &DeviceState) -> bool {
    match state {
        DeviceState::Pen(report) => [
            PenFlags::TIP_SWITCH,
            PenFlags::ERASER,
            PenFlags::BARREL_SWITCH,
        ]
        .into_iter()
        .any(|flag| report.flags.contains(flag)),
        DeviceState::Passthrough { rest, current } => current
            .iter()
            .any(|(report_id, report)| rest.get(report_id).is_some_and(|rest| rest != report)),
        _ => *state != at_rest(state),
    }
}

/// Returns a pen's state out of range, where it was.
fn pen_at_rest(report: &PenReport) -> PenReport {
    PenReport {
        flags: PenFlags::default(),
        pressure: 0,
        ..*report
    }
}

/// Returns the touch frame between two lists of contacts, or nothing if no
/// contact changed: every contact still touching moves, so that the frame
/// lists them all.
fn touch_points(previous: &[TouchContact], current: &[TouchContact]) -> Vec<TouchPoint> {
    if previous == current {
        return Vec::new();
    }
    let mut points: Vec<TouchPoint> = previous
        .iter()
        .filter(|old| current.iter().all(|contact| contact.id != old.id))
        .map(|old| touch_point(old, TouchPhase::Up))
        .collect();
    for contact in current {
        let touching = previous.iter().any(|old| old.id == contact.id);
        let cancelled = contact.flags.contains(TouchContactFlags::CANCELLED);
        match (touching, cancelled) {
            (true, true) => points.push(touch_point(contact, TouchPhase::Cancel)),
            (true, false) => points.push(touch_point(contact, TouchPhase::Move)),
            (false, false) => points.push(touch_point(contact, TouchPhase::Down)),
            // A contact cancelled before it was seen never touched.
            (false, true) => {}
        }
    }
    points
}

/// Returns a contact as a point of a touch frame.
fn touch_point(contact: &TouchContact, phase: TouchPhase) -> TouchPoint {
    TouchPoint {
        id: contact.id,
        phase,
        x: contact.x,
        y: contact.y,
        pressure: contact.pressure,
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
fn release(device_id: DeviceId, state: &DeviceState) -> Vec<InputAction> {
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
        DeviceState::Touchscreen(contacts) => {
            if !contacts.is_empty() {
                actions.push(InputAction::Touch(
                    contacts
                        .iter()
                        .map(|contact| touch_point(contact, TouchPhase::Cancel))
                        .collect(),
                ));
            }
        }
        DeviceState::Pen(report) => {
            if report.flags != PenFlags::default() {
                actions.push(InputAction::Pen(pen_at_rest(report)));
            }
        }
        DeviceState::Touchpad {
            surface,
            clickpad,
            report,
        } => {
            if *report != TouchpadReport::default() {
                actions.push(InputAction::Touchpad {
                    device_id,
                    surface: *surface,
                    clickpad: *clickpad,
                    report: TouchpadReport::default(),
                });
            }
        }
        DeviceState::Passthrough { rest, current } => {
            // Inject the rest report of every report ID away from rest.
            actions.extend(
                current
                    .iter()
                    .filter_map(|(report_id, report)| {
                        rest.get(report_id).filter(|rest| *rest != report)
                    })
                    .map(|rest| InputAction::PassthroughReport {
                        device_id,
                        report: rest.clone(),
                    }),
            );
        }
        DeviceState::Gamepad(report) => {
            let rest = gamepad_at_rest(report);
            if rest != *report {
                actions.push(InputAction::Gamepad {
                    device_id,
                    report: rest,
                });
            }
        }
    }
    actions
}

/// Returns a gamepad's state with its buttons and triggers released and its
/// sticks centred, keeping its motion sensors' readings.
fn gamepad_at_rest(report: &GamepadReport) -> GamepadReport {
    GamepadReport {
        gyro: report.gyro,
        accel: report.accel,
        ..GamepadReport::default()
    }
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
        DeviceState::Gamepad(report) => DeviceState::Gamepad(gamepad_at_rest(report)),
        DeviceState::Touchscreen(_) => DeviceState::Touchscreen(Vec::new()),
        DeviceState::Pen(report) => DeviceState::Pen(pen_at_rest(report)),
        DeviceState::Touchpad {
            surface, clickpad, ..
        } => DeviceState::Touchpad {
            surface: *surface,
            clickpad: *clickpad,
            report: TouchpadReport::default(),
        },
        DeviceState::Passthrough { rest, current } => DeviceState::Passthrough {
            rest: rest.clone(),
            current: current
                .iter()
                .map(|(report_id, report)| {
                    (*report_id, rest.get(report_id).unwrap_or(report).clone())
                })
                .collect(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::{DisplayConfigId, GamepadButtons, InputSequence, Timestamp, WarpId};
    use qdp::wire::VarInt;

    /// The keyboard's Device ID in the tests.
    const KEYBOARD: DeviceId = DeviceId(VarInt::from_u32(1));

    /// The pointer's Device ID in the tests.
    const POINTER: DeviceId = DeviceId(VarInt::from_u32(2));

    /// The mouse's Device ID in the tests.
    const MOUSE: DeviceId = DeviceId(VarInt::from_u32(3));

    /// The gamepad's Device ID in the tests.
    const GAMEPAD: DeviceId = DeviceId(VarInt::from_u32(16));

    /// The touchscreen's Device ID in the tests.
    const TOUCHSCREEN: DeviceId = DeviceId(VarInt::from_u32(5));

    /// The pen's Device ID in the tests.
    const PEN: DeviceId = DeviceId(VarInt::from_u32(6));

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
        assert!(!input.attach(DeviceId(VarInt::from_u32(9)), DeviceKind::TOUCHPAD));
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
        let actions = input.apply(
            &event(KEYBOARD, report.to_bytes()),
            Positions::Apply,
            PointerKind::Absolute,
        );
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
        let actions = input.apply(
            &event(KEYBOARD, report.to_bytes()),
            Positions::Apply,
            PointerKind::Absolute,
        );
        assert_eq!(
            actions,
            [
                InputAction::KeyUp(0x04),
                InputAction::ConsumerKeyUp(0x00e9),
                InputAction::KeyDown(0x05)
            ]
        );
        assert!(input.is_holding());
        assert_eq!(
            input.release_all(),
            [InputAction::KeyUp(0x05), InputAction::KeyUp(0xe1)]
        );
        assert!(!input.is_holding());
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
            input.apply(
                &event(POINTER, report.to_bytes()),
                Positions::Apply,
                PointerKind::Absolute
            ),
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
            input.apply(
                &event(POINTER, next.to_bytes()),
                Positions::Apply,
                PointerKind::Absolute
            ),
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
            input.apply(
                &event(POINTER, report.to_bytes()),
                Positions::Ignore,
                PointerKind::Absolute
            ),
            [InputAction::ButtonDown(PointerButton::Left)]
        );
        assert_eq!(
            input.apply(
                &event(POINTER, report.to_bytes()),
                Positions::Apply,
                PointerKind::Absolute
            ),
            [InputAction::PointerMoveTo {
                x: 10 * 256,
                y: 20 * 256
            }]
        );
    }

    /// Each mode applies only its own device's motion, and buttons from both.
    #[test]
    fn applies_the_current_modes_motion() {
        let mut input = devices();
        let mouse = MouseReport {
            buttons: PointerButtons::RIGHT,
            motion_x_total: 512,
            motion_y_total: 0,
            wheel_total: 0,
            pan_total: 0,
        };
        assert_eq!(
            input.apply(
                &event(MOUSE, mouse.to_bytes()),
                Positions::Apply,
                PointerKind::Absolute
            ),
            [InputAction::ButtonDown(PointerButton::Right)]
        );
        let pointer = PointerReport {
            buttons: PointerButtons::default(),
            x: 5 * 256,
            y: 5 * 256,
            wheel_total: 0,
            pan_total: 0,
        };
        assert!(
            input
                .apply(
                    &event(POINTER, pointer.to_bytes()),
                    Positions::Apply,
                    PointerKind::Relative
                )
                .is_empty()
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
            input.apply(
                &event(MOUSE, report.to_bytes()),
                Positions::Apply,
                PointerKind::Relative
            ),
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
                .apply(
                    &event(MOUSE, report.to_bytes()),
                    Positions::Apply,
                    PointerKind::Relative
                )
                .is_empty()
        );
        assert!(
            input
                .apply(
                    &event(KEYBOARD, vec![1, 2]),
                    Positions::Apply,
                    PointerKind::Absolute
                )
                .is_empty()
        );
    }

    /// Gamepad reports become actions only when the controls change, and
    /// detaching a gamepad recentres it, then removes it.
    #[test]
    fn gamepad_differences() {
        let mut input = devices();
        assert!(input.attach(GAMEPAD, DeviceKind::GAMEPAD));
        let report = GamepadReport {
            buttons: GamepadButtons::button(0),
            left_stick_x: -1000,
            right_trigger: 40_000,
            ..GamepadReport::default()
        };
        let apply = |input: &mut StandardInput| {
            input.apply(
                &event(GAMEPAD, report.to_bytes()),
                Positions::Apply,
                PointerKind::Absolute,
            )
        };
        assert_eq!(
            apply(&mut input),
            [InputAction::Gamepad {
                device_id: GAMEPAD,
                report
            }]
        );
        assert!(apply(&mut input).is_empty());
        assert_eq!(
            input.detach(GAMEPAD),
            [
                InputAction::Gamepad {
                    device_id: GAMEPAD,
                    report: GamepadReport::default()
                },
                InputAction::GamepadRemoved(GAMEPAD)
            ]
        );
        assert!(apply(&mut input).is_empty());
    }

    /// Returns a touchscreen contact.
    fn contact(id: u8, x: i32, flags: TouchContactFlags) -> TouchContact {
        TouchContact {
            id,
            flags,
            x,
            y: 0,
            pressure: 0,
        }
    }

    /// Returns the contacts and phases of the one touch frame among
    /// `actions`, or nothing if there is none.
    fn phases(actions: &[InputAction]) -> Vec<(u8, TouchPhase, i32)> {
        match actions {
            [InputAction::Touch(points)] => points
                .iter()
                .map(|point| (point.id, point.phase, point.x))
                .collect(),
            [] => Vec::new(),
            other => panic!("{other:?}"),
        }
    }

    /// Touchscreen reports become frames listing every contact that touched
    /// or touches, and releasing cancels the contacts still down.
    #[test]
    fn touchscreen_frames() {
        let mut input = devices();
        assert!(input.attach(TOUCHSCREEN, DeviceKind::TOUCHSCREEN));
        let mut apply = |contacts: Vec<TouchContact>| {
            let actions = input.apply(
                &event(TOUCHSCREEN, TouchscreenReport { contacts }.to_bytes()),
                Positions::Apply,
                PointerKind::Absolute,
            );
            phases(&actions)
        };
        let none = TouchContactFlags::default();
        assert_eq!(
            apply(vec![contact(0, 10, none)]),
            [(0, TouchPhase::Down, 10)]
        );
        assert!(apply(vec![contact(0, 10, none)]).is_empty());
        assert_eq!(
            apply(vec![contact(0, 12, none), contact(1, 50, none)]),
            [(0, TouchPhase::Move, 12), (1, TouchPhase::Down, 50)]
        );
        assert_eq!(
            apply(vec![
                contact(1, 50, TouchContactFlags::CANCELLED),
                contact(2, 70, TouchContactFlags::CANCELLED)
            ]),
            [(0, TouchPhase::Up, 12), (1, TouchPhase::Cancel, 50)]
        );
        assert!(apply(Vec::new()).is_empty());
        assert_eq!(apply(vec![contact(3, 5, none)]), [(3, TouchPhase::Down, 5)]);
        assert_eq!(phases(&input.release_all()), [(3, TouchPhase::Cancel, 5)]);
        assert!(input.release_all().is_empty());
    }

    /// Pen reports become actions when they change; a hovering pen holds
    /// nothing, and releasing takes the pen out of range where it was.
    #[test]
    fn pen_states() {
        let mut input = devices();
        assert!(input.attach(PEN, DeviceKind::PEN));
        let mut apply = |report: PenReport| {
            input.apply(
                &event(PEN, report.to_bytes()),
                Positions::Apply,
                PointerKind::Absolute,
            )
        };
        let hovering = PenReport {
            flags: PenFlags::IN_RANGE,
            x: 1000,
            y: 2000,
            tilt_x: 1500,
            ..PenReport::default()
        };
        assert_eq!(apply(hovering), [InputAction::Pen(hovering)]);
        assert!(apply(hovering).is_empty());
        let touching = PenReport {
            flags: PenFlags(PenFlags::IN_RANGE.0 | PenFlags::TIP_SWITCH.0),
            pressure: 30_000,
            ..hovering
        };
        assert_eq!(apply(touching), [InputAction::Pen(touching)]);
        assert!(input.is_holding());
        assert_eq!(
            input.release_all(),
            [InputAction::Pen(PenReport {
                flags: PenFlags::default(),
                pressure: 0,
                ..touching
            })]
        );
        assert!(!input.is_holding());
        assert!(input.release_all().is_empty());
        input.apply(
            &event(PEN, hovering.to_bytes()),
            Positions::Apply,
            PointerKind::Absolute,
        );
        assert!(!input.is_holding());
    }
}
