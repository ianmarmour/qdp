//! The client's touchpad on Linux, read raw through evdev, for a host that
//! recognises gestures itself (Section 8.4.6).
//!
//! The touchpad is grabbed while the window has focus, so that its contacts
//! move only the host's pointer and not the client's too; switching windows
//! with the keyboard gives it back.
//! Reading `/dev/input/event*` needs membership of the `input` group.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use qdp::message::{
    MAX_CONTACTS, TouchpadButtons, TouchpadContact, TouchpadContactFlags, TouchpadInfo,
    TouchpadReport,
};
use qdp::wire::VarInt;
use qdp_runtime::{InputHandle, UserInput};
use tracing::{debug, info};

use super::touchscreen::{ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_SLOT, ABS_MT_TRACKING_ID};
use super::uinput::{EV_ABS, EV_KEY, EV_SYN, Event, IOC_READ, IOC_WRITE, SYN_REPORT};

/// Where the input devices are.
const INPUT_DIRECTORY: &str = "/dev/input";

/// How long the thread waits for events before checking focus and whether
/// to stop.
const POLL_INTERVAL: Duration = Duration::from_millis(4);

/// The touchpad's slot in the session's input.
const SLOT: u32 = 0;

/// `SYN_DROPPED`: events were lost.
const SYN_DROPPED: u16 = 3;

/// `ABS_MT_TOOL_TYPE`.
const ABS_MT_TOOL_TYPE: u16 = 0x37;

/// `MT_TOOL_PALM`.
const MT_TOOL_PALM: i32 = 2;

/// `BTN_LEFT`.
const BTN_LEFT: u16 = 0x110;

/// `BTN_RIGHT`.
const BTN_RIGHT: u16 = 0x111;

/// `BTN_TOOL_FINGER`, which touchpads have and touchscreens do not.
const BTN_TOOL_FINGER: u16 = 0x145;

/// `INPUT_PROP_POINTER`.
const INPUT_PROP_POINTER: usize = 0x00;

/// `INPUT_PROP_BUTTONPAD`.
const INPUT_PROP_BUTTONPAD: usize = 0x02;

/// The largest coordinate along either edge of a QDP touchpad's surface.
const MAX_COORDINATE: i64 = u16::MAX as i64;

/// The size assumed for a touchpad that does not report its resolution,
/// in hundredths of a millimetre: a common laptop touchpad's.
const DEFAULT_SURFACE: (u32, u32) = (10_000, 6_000);

/// Hundredths of a millimetre per millimetre.
const HUNDREDTHS_PER_MM: i64 = 100;

/// The bytes read for a bit mask of event codes.
const BIT_MASK_BYTES: usize = 96;

/// The type of evdev's ioctl requests.
const EVDEV_IOCTL_TYPE: u8 = b'E';

/// The size of `struct input_absinfo`.
const ABSINFO_SIZE: usize = 24;

/// Returns an evdev ioctl request.
const fn evdev_request(direction: u32, number: u32, size: usize) -> libc::Ioctl {
    (direction << 30 | (size as u32) << 16 | (EVDEV_IOCTL_TYPE as u32) << 8 | number) as libc::Ioctl
}

/// `EVIOCGNAME(len)`.
const EVIOCGNAME: libc::Ioctl = evdev_request(IOC_READ, 0x06, 256);

/// `EVIOCGPROP(len)`.
const EVIOCGPROP: libc::Ioctl = evdev_request(IOC_READ, 0x09, BIT_MASK_BYTES);

/// `EVIOCGRAB`.
const EVIOCGRAB: libc::Ioctl = evdev_request(IOC_WRITE, 0x90, size_of::<libc::c_int>());

/// Returns `EVIOCGBIT(kind, len)`: the codes a device reports of a type.
const fn eviocgbit(kind: u16) -> libc::Ioctl {
    evdev_request(IOC_READ, 0x20 + kind as u32, BIT_MASK_BYTES)
}

/// Returns `EVIOCGABS(axis)`: an absolute axis's range and resolution.
const fn eviocgabs(axis: u16) -> libc::Ioctl {
    evdev_request(IOC_READ, 0x40 + axis as u32, ABSINFO_SIZE)
}

/// Returns whether bit `bit` is set in a mask.
fn has_bit(mask: &[u8], bit: usize) -> bool {
    mask.get(bit / 8)
        .is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
}

/// Reads a bit mask with an ioctl.
fn read_mask(file: &File, request: libc::Ioctl) -> Option<Vec<u8>> {
    let mut mask = vec![0; BIT_MASK_BYTES];
    // SAFETY: the request's length is the mask's, which the call fills.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request, mask.as_mut_ptr()) };
    (result >= 0).then_some(mask)
}

/// An absolute axis's range and resolution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Axis {
    /// The smallest value.
    minimum: i32,
    /// The largest value.
    maximum: i32,
    /// Units per millimetre, or 0 if unknown.
    resolution: i32,
}

impl Axis {
    /// Reads an axis of a device.
    fn read(file: &File, code: u16) -> Option<Self> {
        let mut info = [0_i32; ABSINFO_SIZE / 4];
        // SAFETY: the buffer is a `struct input_absinfo`'s size, which the
        // call fills: value, minimum, maximum, fuzz, flat, resolution.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), eviocgabs(code), info.as_mut_ptr()) };
        (result >= 0).then_some(Self {
            minimum: info[1],
            maximum: info[2],
            resolution: info[5],
        })
    }

    /// Returns a value as a QDP surface coordinate, from 0 to 65535.
    fn coordinate(self, value: i32) -> u16 {
        let span = (i64::from(self.maximum) - i64::from(self.minimum)).max(1);
        let offset = (i64::from(value) - i64::from(self.minimum)).clamp(0, span);
        (offset * MAX_COORDINATE / span) as u16
    }

    /// Returns the axis's length in hundredths of a millimetre, if known.
    fn length(self) -> Option<u32> {
        let span = i64::from(self.maximum) - i64::from(self.minimum);
        (self.resolution > 0)
            .then(|| span * HUNDREDTHS_PER_MM / i64::from(self.resolution))
            .and_then(|length| u32::try_from(length).ok())
    }
}

/// One multitouch slot's finger.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Slot {
    /// The finger's tracking ID, or -1 if the slot is free.
    tracking_id: i32,
    /// Its horizontal position.
    x: i32,
    /// Its vertical position.
    y: i32,
    /// Whether the touchpad takes it for a palm.
    palm: bool,
}

/// Turns a touchpad's evdev events into reports.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Tracker {
    /// The horizontal axis.
    x: Axis,
    /// The vertical axis.
    y: Axis,
    /// The slots.
    slots: [Slot; MAX_CONTACTS],
    /// The slot the next events change.
    current: usize,
    /// The buttons held.
    buttons: TouchpadButtons,
    /// Whether events were lost until the next report.
    dropped: bool,
}

impl Tracker {
    /// Returns a tracker for a touchpad with these axes.
    fn new(x: Axis, y: Axis) -> Self {
        Self {
            x,
            y,
            slots: [Slot {
                tracking_id: -1,
                ..Slot::default()
            }; MAX_CONTACTS],
            current: 0,
            buttons: TouchpadButtons::default(),
            dropped: false,
        }
    }

    /// Takes one event, returning the touchpad's state when a report ends.
    fn event(&mut self, event: Event) -> Option<TouchpadReport> {
        match (event.kind, event.code) {
            (EV_SYN, SYN_DROPPED) => {
                // The kernel's state is unknown until the next report: lift
                // every finger rather than leave one stuck.
                self.dropped = true;
                for slot in &mut self.slots {
                    slot.tracking_id = -1;
                }
            }
            (EV_SYN, SYN_REPORT) => {
                if std::mem::take(&mut self.dropped) {
                    return None;
                }
                return Some(self.report());
            }
            _ if self.dropped => {}
            (EV_ABS, ABS_MT_SLOT) => {
                self.current = usize::try_from(event.value)
                    .unwrap_or(0)
                    .min(MAX_CONTACTS - 1);
            }
            (EV_ABS, ABS_MT_TRACKING_ID) => self.slots[self.current].tracking_id = event.value,
            (EV_ABS, ABS_MT_POSITION_X) => self.slots[self.current].x = event.value,
            (EV_ABS, ABS_MT_POSITION_Y) => self.slots[self.current].y = event.value,
            (EV_ABS, ABS_MT_TOOL_TYPE) => {
                self.slots[self.current].palm = event.value == MT_TOOL_PALM;
            }
            (EV_KEY, BTN_LEFT) => self.set_button(TouchpadButtons::PRIMARY, event.value != 0),
            (EV_KEY, BTN_RIGHT) => self.set_button(TouchpadButtons::SECONDARY, event.value != 0),
            _ => {}
        }
        None
    }

    /// Presses or releases a button.
    fn set_button(&mut self, button: TouchpadButtons, held: bool) {
        if held {
            self.buttons.0 |= button.0;
        } else {
            self.buttons.0 &= !button.0;
        }
    }

    /// Returns the touchpad's state.
    fn report(&self) -> TouchpadReport {
        let contacts = (0_u8..)
            .zip(&self.slots)
            .filter(|(_, slot)| slot.tracking_id >= 0)
            .map(|(id, slot)| TouchpadContact {
                id,
                flags: if slot.palm {
                    TouchpadContactFlags::default()
                } else {
                    TouchpadContactFlags::CONFIDENCE
                },
                x: self.x.coordinate(slot.x),
                y: self.y.coordinate(slot.y),
            })
            .collect();
        TouchpadReport {
            buttons: self.buttons,
            contacts,
        }
    }
}

/// A touchpad found among the input devices.
#[derive(Debug)]
struct Found {
    /// The device.
    file: File,
    /// Its name.
    name: String,
    /// Its horizontal axis.
    x: Axis,
    /// Its vertical axis.
    y: Axis,
    /// Whether the surface is the button.
    clickpad: bool,
}

/// Opens the first touchpad among the input devices.
fn find_touchpad() -> Option<Found> {
    let mut nodes: Vec<_> = fs::read_dir(INPUT_DIRECTORY)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .collect();
    nodes.sort();
    nodes.iter().find_map(|node| open_touchpad(node))
}

/// Opens `node` if it is a touchpad: a pointing device with multitouch
/// positions and finger tools.
fn open_touchpad(node: &Path) -> Option<Found> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(node)
        .ok()?;
    let properties = read_mask(&file, EVIOCGPROP)?;
    let axes = read_mask(&file, eviocgbit(EV_ABS))?;
    let keys = read_mask(&file, eviocgbit(EV_KEY))?;
    let touchpad = has_bit(&properties, INPUT_PROP_POINTER)
        && has_bit(&axes, usize::from(ABS_MT_POSITION_X))
        && has_bit(&keys, usize::from(BTN_TOOL_FINGER));
    if !touchpad {
        return None;
    }
    let mut name = vec![0_u8; 256];
    // SAFETY: the request's length is the buffer's, which the call fills.
    unsafe { libc::ioctl(file.as_raw_fd(), EVIOCGNAME, name.as_mut_ptr()) };
    let name = String::from_utf8_lossy(&name)
        .trim_end_matches('\0')
        .to_owned();
    Some(Found {
        x: Axis::read(&file, ABS_MT_POSITION_X)?,
        y: Axis::read(&file, ABS_MT_POSITION_Y)?,
        clickpad: has_bit(&properties, INPUT_PROP_BUTTONPAD),
        name,
        file,
    })
}

/// Grabs the touchpad for this process alone, or gives it back.
fn grab(file: &File, grabbed: bool) {
    // SAFETY: the request takes an int by value.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), EVIOCGRAB, libc::c_int::from(grabbed)) };
    if result < 0 {
        debug!(error = %io::Error::last_os_error(), grabbed, "touchpad grab not changed");
    }
}

/// Reads the touchpad until `stop` is raised or the session ends, grabbing
/// it while the window has focus and sending its state then.
fn run(mut found: Found, input: &InputHandle, focused: &AtomicBool, stop: &AtomicBool) {
    let mut tracker = Tracker::new(found.x, found.y);
    let mut grabbed = false;
    let event_length = size_of::<libc::input_event>();
    let mut buffer = vec![0; event_length * 64];
    while !stop.load(Ordering::Relaxed) {
        let wanted = focused.load(Ordering::Relaxed);
        if wanted != grabbed {
            grab(&found.file, wanted);
            grabbed = wanted;
        }
        let mut poll = libc::pollfd {
            fd: found.file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll` is one valid `pollfd`.
        if unsafe { libc::poll(&mut poll, 1, POLL_INTERVAL.as_millis() as i32) } <= 0 {
            continue;
        }
        let read = match found.file.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
            Err(error) => {
                info!(%error, "touchpad gone");
                break;
            }
        };
        for bytes in buffer[..read].chunks_exact(event_length) {
            let Some(report) = tracker.event(Event::from_bytes(bytes)) else {
                continue;
            };
            if grabbed && !input.send(UserInput::Touchpad { slot: SLOT, report }) {
                return;
            }
        }
    }
    if grabbed {
        grab(&found.file, false);
    }
    input.send(UserInput::TouchpadDisconnected { slot: SLOT });
}

/// Finds the touchpad, attaches it, and starts reading it on a thread that
/// stops when `stop` is raised.
pub(crate) fn start(
    input: &InputHandle,
    focused: &Arc<AtomicBool>,
    stop: &Arc<AtomicBool>,
) -> Result<JoinHandle<()>, String> {
    let found = find_touchpad()
        .ok_or("no touchpad can be read; reading one needs membership of the input group")?;
    let (width, height) = match (found.x.length(), found.y.length()) {
        (Some(width), Some(height)) => (width, height),
        _ => DEFAULT_SURFACE,
    };
    info!(
        name = found.name,
        width, height, "sending the touchpad's contacts"
    );
    input.send(UserInput::TouchpadConnected {
        slot: SLOT,
        name: found.name.clone(),
        surface: TouchpadInfo {
            width: VarInt::from_u32(width),
            height: VarInt::from_u32(height),
        },
        clickpad: found.clickpad,
    });
    let (input, focused, stop) = (input.clone(), Arc::clone(focused), Arc::clone(stop));
    std::thread::Builder::new()
        .name("qdp-touchpad".to_owned())
        .spawn(move || run(found, &input, &focused, &stop))
        .map_err(|error| format!("cannot read the touchpad: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The touchpad's axes in the tests: 1000 units over 100 mm by 600
    /// units over 60 mm.
    fn tracker() -> Tracker {
        let x = Axis {
            minimum: 0,
            maximum: 1000,
            resolution: 10,
        };
        let y = Axis {
            minimum: 0,
            maximum: 600,
            resolution: 10,
        };
        Tracker::new(x, y)
    }

    /// Returns an absolute axis event.
    fn abs(code: u16, value: i32) -> Event {
        Event::new(EV_ABS, code, value)
    }

    /// The report that ends a group of events.
    const SYN: Event = Event::new(EV_SYN, SYN_REPORT, 0);

    /// Fingers in slots become contacts at surface coordinates, palms lose
    /// their confidence, and buttons follow the clicks.
    #[test]
    fn tracks_fingers() {
        let mut tracker = tracker();
        for event in [
            abs(ABS_MT_SLOT, 0),
            abs(ABS_MT_TRACKING_ID, 7),
            abs(ABS_MT_POSITION_X, 500),
            abs(ABS_MT_POSITION_Y, 600),
            abs(ABS_MT_SLOT, 1),
            abs(ABS_MT_TRACKING_ID, 8),
            abs(ABS_MT_TOOL_TYPE, MT_TOOL_PALM),
            Event::new(EV_KEY, BTN_LEFT, 1),
        ] {
            assert_eq!(tracker.event(event), None);
        }
        let report = tracker.event(SYN).unwrap();
        assert_eq!(report.buttons, TouchpadButtons::PRIMARY);
        assert_eq!(report.contacts.len(), 2);
        assert_eq!(
            (report.contacts[0].x, report.contacts[0].y),
            (32_767, 65_535)
        );
        assert!(
            report.contacts[0]
                .flags
                .contains(TouchpadContactFlags::CONFIDENCE)
        );
        assert!(
            !report.contacts[1]
                .flags
                .contains(TouchpadContactFlags::CONFIDENCE)
        );
        tracker.event(abs(ABS_MT_TRACKING_ID, -1));
        tracker.event(Event::new(EV_KEY, BTN_LEFT, 0));
        let report = tracker.event(SYN).unwrap();
        assert_eq!(report.contacts.len(), 1);
        assert_eq!(report.buttons, TouchpadButtons::default());
    }

    /// Lost events lift every finger, and nothing is reported until the
    /// next complete report.
    #[test]
    fn recovers_from_dropped_events() {
        let mut tracker = tracker();
        tracker.event(abs(ABS_MT_TRACKING_ID, 1));
        tracker.event(SYN);
        tracker.event(Event::new(EV_SYN, SYN_DROPPED, 0));
        tracker.event(abs(ABS_MT_TRACKING_ID, 2));
        assert_eq!(tracker.event(SYN), None);
        assert!(tracker.event(SYN).unwrap().contacts.is_empty());
    }

    /// Surface sizes come from the axes' resolution, and the ioctls have
    /// the numbers of `linux/input.h`.
    #[test]
    fn measures_the_surface() {
        let tracker = tracker();
        assert_eq!(tracker.x.length(), Some(10_000));
        assert_eq!(tracker.y.length(), Some(6_000));
        assert_eq!(Axis::default().length(), None);
        assert_eq!(EVIOCGRAB, 0x4004_4590);
        assert_eq!(eviocgabs(ABS_MT_POSITION_X), 0x8018_4575_u32 as libc::Ioctl);
        assert_eq!(eviocgbit(EV_ABS), 0x8060_4523_u32 as libc::Ioctl);
    }
}
