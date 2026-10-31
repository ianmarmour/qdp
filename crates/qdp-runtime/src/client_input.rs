//! The client's side of input: what the user does in the client's window,
//! turned into standard device reports (Section 8.4) for the input sender,
//! which delivers them reliably (Section 8.6).

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use qdp::input::{DeviceRest, ReportKind};
use qdp::message::{
    DeviceId, KeyboardReport, MouseReport, PointerButtons, PointerKind, PointerReport, TextReport,
};
use qdp::wire::VarInt;
use tokio::sync::Notify;

use crate::host_input::PointerButton;

/// The keyboard's Device ID.
pub(crate) const KEYBOARD_ID: DeviceId = DeviceId(VarInt::from_u32(1));

/// The absolute pointer's Device ID.
pub(crate) const POINTER_ID: DeviceId = DeviceId(VarInt::from_u32(2));

/// The relative mouse's Device ID.
pub(crate) const MOUSE_ID: DeviceId = DeviceId(VarInt::from_u32(3));

/// The text device's Device ID.
pub(crate) const TEXT_ID: DeviceId = DeviceId(VarInt::from_u32(4));

/// Positions and motion are sent in 1/256 of a pixel or device count
/// (Section 1.5.6).
const SUBUNITS: f64 = 256.0;

/// The most consumer keys a keyboard report holds.
const CONSUMER_KEY_SLOTS: usize = 4;

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
    /// Everything held is released, as when the window loses focus.
    ReleaseAll,
}

/// Returns a handle for the user interface to send input with and the queue
/// the session reads it from.
pub fn input_channel() -> (InputHandle, InputQueue) {
    let (sender, receiver) = mpsc::channel();
    let wake = Arc::new(Notify::new());
    (
        InputHandle {
            sender,
            wake: Arc::clone(&wake),
        },
        InputQueue { receiver, wake },
    )
}

/// Sends the user's input to the session, from any thread.
#[derive(Debug, Clone)]
pub struct InputHandle {
    /// The queue.
    sender: Sender<UserInput>,
    /// Wakes the session.
    wake: Arc<Notify>,
}

impl InputHandle {
    /// Sends one input, returning `false` if the session has ended.
    pub fn send(&self, input: UserInput) -> bool {
        let sent = self.sender.send(input).is_ok();
        self.wake.notify_one();
        sent
    }
}

/// The user's input, waiting for the session.
#[derive(Debug)]
pub struct InputQueue {
    /// The queue.
    receiver: Receiver<UserInput>,
    /// Notified when input arrives.
    wake: Arc<Notify>,
}

impl InputQueue {
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
        }
    }
}

impl ClientDevices {
    /// Sets the mode `POINTER_MODE` announced (Section 8.9).
    pub(crate) fn set_pointer_mode(&mut self, mode: PointerKind) {
        self.pointer_mode = mode;
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
                let x = ((f64::from(self.monitor_origin.0) + x) * SUBUNITS).round() as i32;
                let y = ((f64::from(self.monitor_origin.1) + y) * SUBUNITS).round() as i32;
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
            UserInput::ReleaseAll => {
                let mut reports = Vec::new();
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
                reports
            }
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

        devices.set_pointer_mode(PointerKind::Relative);
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

    /// Long text is split into several reports.
    #[test]
    fn text() {
        let mut devices = ClientDevices::default();
        let reports = devices.reports(UserInput::Text("é".repeat(200)));
        assert_eq!(reports.len(), 2);
        assert!(reports.iter().all(|report| report.device_id == TEXT_ID));
    }
}
