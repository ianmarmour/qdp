//! Input injection through uinput, the kernel's interface for virtual input
//! devices. The devices it creates look like real USB devices to every layer
//! above the kernel, so injection works under any Wayland compositor or X11
//! server, without the compositor's cooperation.
//!
//! Three devices are created: a keyboard, a relative mouse, and an absolute
//! pointer covering the desktop, as virtual machines' tablets do. Creating
//! them needs write access to `/dev/uinput`.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

use qdp::message::KeyboardLeds;
use qdp_runtime::{InputAction, InputInjector, PointerButton};

use super::keycodes::{all_keys, consumer_key, keyboard_key};

/// The uinput device node.
const UINPUT_PATH: &str = "/dev/uinput";

/// `UI_DEV_CREATE`.
const UI_DEV_CREATE: libc::Ioctl = 0x5501;

/// `UI_DEV_DESTROY`.
const UI_DEV_DESTROY: libc::Ioctl = 0x5502;

/// `UI_DEV_SETUP`, for a `struct uinput_setup` of 92 bytes.
const UI_DEV_SETUP: libc::Ioctl = 0x405c_5503;

/// `UI_ABS_SETUP`, for a `struct uinput_abs_setup` of 28 bytes.
const UI_ABS_SETUP: libc::Ioctl = 0x401c_5504;

/// `UI_SET_EVBIT`.
const UI_SET_EVBIT: libc::Ioctl = 0x4004_5564;

/// `UI_SET_KEYBIT`.
const UI_SET_KEYBIT: libc::Ioctl = 0x4004_5565;

/// `UI_SET_RELBIT`.
const UI_SET_RELBIT: libc::Ioctl = 0x4004_5566;

/// `UI_SET_ABSBIT`.
const UI_SET_ABSBIT: libc::Ioctl = 0x4004_5567;

/// `UI_SET_LEDBIT`.
const UI_SET_LEDBIT: libc::Ioctl = 0x4004_5569;

/// `EV_SYN`: the end of a group of events.
const EV_SYN: u16 = 0x00;

/// `EV_KEY`: keys and buttons.
const EV_KEY: u16 = 0x01;

/// `EV_REL`: relative axes.
const EV_REL: u16 = 0x02;

/// `EV_ABS`: absolute axes.
const EV_ABS: u16 = 0x03;

/// `EV_LED`: LEDs, which the compositor sets on keyboards.
const EV_LED: u16 = 0x11;

/// `SYN_REPORT`.
const SYN_REPORT: u16 = 0;

/// `REL_X`.
const REL_X: u16 = 0x00;

/// `REL_Y`.
const REL_Y: u16 = 0x01;

/// `REL_HWHEEL`: horizontal scrolling in detents, positive to the right.
const REL_HWHEEL: u16 = 0x06;

/// `REL_WHEEL`: vertical scrolling in detents, positive away from the user.
const REL_WHEEL: u16 = 0x08;

/// `REL_WHEEL_HI_RES`: vertical scrolling in 1/120 detent.
const REL_WHEEL_HI_RES: u16 = 0x0b;

/// `REL_HWHEEL_HI_RES`: horizontal scrolling in 1/120 detent.
const REL_HWHEEL_HI_RES: u16 = 0x0c;

/// `ABS_X`.
const ABS_X: u16 = 0x00;

/// `ABS_Y`.
const ABS_Y: u16 = 0x01;

/// `BUS_VIRTUAL`.
const BUS_VIRTUAL: u16 = 0x06;

/// The LEDs the keyboard reports, as `(LED code, QDP LED bit)`.
const LEDS: [(u16, u8); 5] = [
    (0x00, 0x01), // LED_NUML: Num Lock
    (0x01, 0x02), // LED_CAPSL: Caps Lock
    (0x02, 0x04), // LED_SCROLLL: Scroll Lock
    (0x03, 0x08), // LED_COMPOSE: Compose
    (0x04, 0x10), // LED_KANA: Kana
];

/// Scroll units per detent: high-resolution wheel events are in 1/120 detent,
/// as QDP's scroll units are.
const UNITS_PER_DETENT: i32 = 120;

/// QDP positions and motion are in 1/256 pixel or count (Section 1.5.6).
const SUBUNITS: i32 = 256;

/// Returns the code of a button.
fn button_code(button: PointerButton) -> u16 {
    match button {
        PointerButton::Left => 0x110,    // BTN_LEFT
        PointerButton::Right => 0x111,   // BTN_RIGHT
        PointerButton::Middle => 0x112,  // BTN_MIDDLE
        PointerButton::Back => 0x113,    // BTN_SIDE
        PointerButton::Forward => 0x114, // BTN_EXTRA
    }
}

/// Every button code.
const BUTTONS: [PointerButton; 5] = [
    PointerButton::Left,
    PointerButton::Right,
    PointerButton::Middle,
    PointerButton::Back,
    PointerButton::Forward,
];

/// One input event: type, code and value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Event {
    /// The event type.
    pub(super) kind: u16,
    /// The event code.
    pub(super) code: u16,
    /// The value.
    pub(super) value: i32,
}

impl Event {
    /// Returns an event.
    const fn new(kind: u16, code: u16, value: i32) -> Self {
        Self { kind, code, value }
    }

    /// Returns the event as a `struct input_event`, with a zero timestamp,
    /// which the kernel fills in.
    fn to_bytes(self) -> Vec<u8> {
        let time_length = size_of::<libc::input_event>() - 8;
        let mut bytes = vec![0; time_length];
        bytes.extend_from_slice(&self.kind.to_ne_bytes());
        bytes.extend_from_slice(&self.code.to_ne_bytes());
        bytes.extend_from_slice(&self.value.to_ne_bytes());
        bytes
    }
}

/// Which virtual device an event goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Device {
    /// The keyboard.
    Keyboard,
    /// The relative mouse.
    Mouse,
    /// The absolute pointer, which also carries buttons and scrolling.
    Pointer,
}

/// Turns actions into events, keeping the fractions of motion and scrolling
/// that whole events cannot carry yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Translator {
    /// Relative motion not yet sent, in 1/256 count.
    motion_remainder: (i32, i32),
    /// Scrolling not yet sent as whole detents, in 1/120 detent.
    detent_remainder: (i32, i32),
}

impl Translator {
    /// Returns the events an action produces, or `None` if uinput cannot
    /// perform it.
    pub(super) fn events(&mut self, action: &InputAction) -> Option<(Device, Vec<Event>)> {
        let events = match *action {
            InputAction::KeyDown(usage) => (
                Device::Keyboard,
                vec![Event::new(EV_KEY, keyboard_key(usage)?, 1)],
            ),
            InputAction::KeyUp(usage) => (
                Device::Keyboard,
                vec![Event::new(EV_KEY, keyboard_key(usage)?, 0)],
            ),
            InputAction::ConsumerKeyDown(usage) => (
                Device::Keyboard,
                vec![Event::new(EV_KEY, consumer_key(usage)?, 1)],
            ),
            InputAction::ConsumerKeyUp(usage) => (
                Device::Keyboard,
                vec![Event::new(EV_KEY, consumer_key(usage)?, 0)],
            ),
            InputAction::PointerMoveTo { x, y } => (
                Device::Pointer,
                vec![
                    Event::new(EV_ABS, ABS_X, x.max(0)),
                    Event::new(EV_ABS, ABS_Y, y.max(0)),
                ],
            ),
            InputAction::MouseMoveBy { dx, dy } => {
                let x = self.motion_remainder.0 + dx;
                let y = self.motion_remainder.1 + dy;
                self.motion_remainder = (x % SUBUNITS, y % SUBUNITS);
                let (x, y) = (x / SUBUNITS, y / SUBUNITS);
                let mut events = Vec::new();
                if x != 0 {
                    events.push(Event::new(EV_REL, REL_X, x));
                }
                if y != 0 {
                    events.push(Event::new(EV_REL, REL_Y, y));
                }
                (Device::Mouse, events)
            }
            InputAction::ButtonDown(button) => (
                Device::Pointer,
                vec![Event::new(EV_KEY, button_code(button), 1)],
            ),
            InputAction::ButtonUp(button) => (
                Device::Pointer,
                vec![Event::new(EV_KEY, button_code(button), 0)],
            ),
            InputAction::Scroll {
                vertical,
                horizontal,
            } => {
                let mut events = Vec::new();
                let vertical_detents = self.detents(vertical, false);
                let horizontal_detents = self.detents(horizontal, true);
                if vertical != 0 {
                    events.push(Event::new(EV_REL, REL_WHEEL_HI_RES, vertical));
                }
                if vertical_detents != 0 {
                    events.push(Event::new(EV_REL, REL_WHEEL, vertical_detents));
                }
                if horizontal != 0 {
                    events.push(Event::new(EV_REL, REL_HWHEEL_HI_RES, horizontal));
                }
                if horizontal_detents != 0 {
                    events.push(Event::new(EV_REL, REL_HWHEEL, horizontal_detents));
                }
                (Device::Pointer, events)
            }
            InputAction::Text(_) => return None,
        };
        Some(events)
    }

    /// Adds scrolling to the detent remainder and returns the whole detents.
    fn detents(&mut self, units: i32, horizontal: bool) -> i32 {
        let remainder = if horizontal {
            &mut self.detent_remainder.1
        } else {
            &mut self.detent_remainder.0
        };
        let total = *remainder + units;
        *remainder = total % UNITS_PER_DETENT;
        total / UNITS_PER_DETENT
    }
}

/// One virtual device.
#[derive(Debug)]
struct VirtualDevice {
    /// The device's uinput file; closing it removes the device.
    file: File,
}

impl VirtualDevice {
    /// Opens `/dev/uinput` for a new device.
    fn open() -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(UINPUT_PATH)
    }

    /// Calls a uinput ioctl that takes an integer.
    fn set_bit(file: &File, request: libc::Ioctl, bit: u16) -> io::Result<()> {
        // SAFETY: the request takes an int by value, and the descriptor is open.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), request, libc::c_int::from(bit)) };
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// Creates the device named `name` on a file whose capabilities are set.
    fn create(file: File, name: &str) -> io::Result<Self> {
        // SAFETY: all zero bits is a valid `uinput_setup`.
        let mut setup: libc::uinput_setup = unsafe { std::mem::zeroed() };
        setup.id.bustype = BUS_VIRTUAL;
        for (slot, byte) in setup
            .name
            .iter_mut()
            .zip(name.bytes().take(libc::UINPUT_MAX_NAME_SIZE - 1))
        {
            *slot = byte as libc::c_char;
        }
        // SAFETY: `setup` is a valid `uinput_setup`, which the call reads.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), UI_DEV_SETUP, &setup) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the request takes no argument, and the descriptor is open.
        let result = unsafe { libc::ioctl(file.as_raw_fd(), UI_DEV_CREATE) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { file })
    }

    /// Writes events followed by a `SYN_REPORT`, so they apply together.
    fn emit(&mut self, events: &[Event]) -> io::Result<()> {
        let mut bytes = Vec::new();
        for event in events.iter().chain([&Event::new(EV_SYN, SYN_REPORT, 0)]) {
            bytes.extend(event.to_bytes());
        }
        self.file.write_all(&bytes)
    }
}

impl Drop for VirtualDevice {
    fn drop(&mut self) {
        // SAFETY: the request takes no argument, and the descriptor is open.
        unsafe { libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) };
    }
}

/// Injects input through three uinput devices.
#[derive(Debug)]
pub struct UinputInjector {
    /// The keyboard.
    keyboard: VirtualDevice,
    /// The relative mouse.
    mouse: VirtualDevice,
    /// The absolute pointer.
    pointer: VirtualDevice,
    /// Turns actions into events.
    translator: Translator,
    /// The keyboard LEDs the compositor last set.
    leds: KeyboardLeds,
}

impl UinputInjector {
    /// Creates the virtual devices, with the absolute pointer covering a
    /// `width × height` pixel desktop.
    pub fn new(width: u32, height: u32) -> io::Result<Self> {
        let keyboard = {
            let file = VirtualDevice::open()?;
            VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_KEY)?;
            for code in all_keys() {
                VirtualDevice::set_bit(&file, UI_SET_KEYBIT, code)?;
            }
            VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_LED)?;
            for (code, _) in LEDS {
                VirtualDevice::set_bit(&file, UI_SET_LEDBIT, code)?;
            }
            VirtualDevice::create(file, "QDP keyboard")?
        };
        let mouse = {
            let file = VirtualDevice::open()?;
            VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_KEY)?;
            for button in BUTTONS {
                VirtualDevice::set_bit(&file, UI_SET_KEYBIT, button_code(button))?;
            }
            VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_REL)?;
            VirtualDevice::set_bit(&file, UI_SET_RELBIT, REL_X)?;
            VirtualDevice::set_bit(&file, UI_SET_RELBIT, REL_Y)?;
            VirtualDevice::create(file, "QDP mouse")?
        };
        let pointer = {
            let file = VirtualDevice::open()?;
            VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_KEY)?;
            for button in BUTTONS {
                VirtualDevice::set_bit(&file, UI_SET_KEYBIT, button_code(button))?;
            }
            VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_REL)?;
            for axis in [REL_WHEEL, REL_HWHEEL, REL_WHEEL_HI_RES, REL_HWHEEL_HI_RES] {
                VirtualDevice::set_bit(&file, UI_SET_RELBIT, axis)?;
            }
            VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_ABS)?;
            for (axis, pixels) in [(ABS_X, width), (ABS_Y, height)] {
                VirtualDevice::set_bit(&file, UI_SET_ABSBIT, axis)?;
                let maximum = i32::try_from(pixels.max(1))
                    .unwrap_or(i32::MAX / SUBUNITS)
                    .saturating_mul(SUBUNITS)
                    - 1;
                // SAFETY: all zero bits is a valid `uinput_abs_setup`.
                let mut setup: libc::uinput_abs_setup = unsafe { std::mem::zeroed() };
                setup.code = axis;
                setup.absinfo.maximum = maximum;
                // SAFETY: `setup` is a valid `uinput_abs_setup`, which the call
                // reads.
                let result = unsafe { libc::ioctl(file.as_raw_fd(), UI_ABS_SETUP, &setup) };
                if result < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            VirtualDevice::create(file, "QDP pointer")?
        };
        Ok(Self {
            keyboard,
            mouse,
            pointer,
            translator: Translator::default(),
            leds: KeyboardLeds::default(),
        })
    }

    /// Reads the LED events the compositor wrote to the keyboard.
    fn read_leds(&mut self) {
        let event_length = size_of::<libc::input_event>();
        let mut buffer = vec![0; event_length * 16];
        while let Ok(read) = self.keyboard.file.read(&mut buffer) {
            if read == 0 {
                break;
            }
            for event in buffer[..read].chunks_exact(event_length) {
                let fields = &event[event_length - 8..];
                let kind = u16::from_ne_bytes([fields[0], fields[1]]);
                let code = u16::from_ne_bytes([fields[2], fields[3]]);
                let value = i32::from_ne_bytes([fields[4], fields[5], fields[6], fields[7]]);
                if kind != EV_LED {
                    continue;
                }
                if let Some(&(_, bit)) = LEDS.iter().find(|(led, _)| *led == code) {
                    if value != 0 {
                        self.leds.0 |= bit;
                    } else {
                        self.leds.0 &= !bit;
                    }
                }
            }
        }
    }
}

impl InputInjector for UinputInjector {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        let Some((device, events)) = self.translator.events(action) else {
            return Err("uinput has no event for this action".to_owned());
        };
        if events.is_empty() {
            return Ok(());
        }
        let device = match device {
            Device::Keyboard => &mut self.keyboard,
            Device::Mouse => &mut self.mouse,
            Device::Pointer => &mut self.pointer,
        };
        device.emit(&events).map_err(|error| error.to_string())
    }

    fn description(&self) -> &str {
        "uinput"
    }

    fn keyboard_leds(&mut self) -> KeyboardLeds {
        self.read_leds();
        self.leds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Events are encoded as `struct input_event`: a zero timestamp, then type,
    /// code and value.
    #[test]
    fn encodes_events() {
        assert_eq!(size_of::<libc::input_event>(), 24);
        assert_eq!(size_of::<libc::uinput_setup>(), 92);
        assert_eq!(size_of::<libc::uinput_abs_setup>(), 28);
        let bytes = Event::new(EV_KEY, 30, 1).to_bytes();
        assert_eq!(bytes.len(), 24);
        assert!(bytes[..16].iter().all(|&byte| byte == 0));
        assert_eq!(bytes[16..], [1, 0, 30, 0, 1, 0, 0, 0]);
    }

    /// Keys, positions and buttons map to the codes of
    /// `linux/input-event-codes.h`.
    #[test]
    fn translates_keys_and_buttons() {
        let mut translator = Translator::default();
        assert_eq!(
            translator.events(&InputAction::KeyDown(0x04)),
            Some((Device::Keyboard, vec![Event::new(EV_KEY, 30, 1)]))
        );
        assert_eq!(
            translator.events(&InputAction::ConsumerKeyUp(0x00e9)),
            Some((Device::Keyboard, vec![Event::new(EV_KEY, 115, 0)]))
        );
        assert_eq!(
            translator.events(&InputAction::PointerMoveTo { x: 2560, y: 512 }),
            Some((
                Device::Pointer,
                vec![
                    Event::new(EV_ABS, ABS_X, 2560),
                    Event::new(EV_ABS, ABS_Y, 512)
                ]
            ))
        );
        assert_eq!(
            translator.events(&InputAction::ButtonDown(PointerButton::Right)),
            Some((Device::Pointer, vec![Event::new(EV_KEY, 0x111, 1)]))
        );
        assert_eq!(translator.events(&InputAction::KeyDown(0xa5)), None);
        assert_eq!(translator.events(&InputAction::Text("a".to_owned())), None);
    }

    /// Relative motion and scrolling keep their fractions until they add up to
    /// whole counts and detents.
    #[test]
    fn keeps_fractions() {
        let mut translator = Translator::default();
        let motion = |translator: &mut Translator, dx| {
            translator
                .events(&InputAction::MouseMoveBy { dx, dy: 0 })
                .unwrap()
                .1
        };
        assert!(motion(&mut translator, 128).is_empty());
        assert_eq!(motion(&mut translator, 128), [Event::new(EV_REL, REL_X, 1)]);
        let scroll = |translator: &mut Translator, vertical| {
            translator
                .events(&InputAction::Scroll {
                    vertical,
                    horizontal: 0,
                })
                .unwrap()
                .1
        };
        assert_eq!(
            scroll(&mut translator, 60),
            [Event::new(EV_REL, REL_WHEEL_HI_RES, 60)]
        );
        assert_eq!(
            scroll(&mut translator, 60),
            [
                Event::new(EV_REL, REL_WHEEL_HI_RES, 60),
                Event::new(EV_REL, REL_WHEEL, 1)
            ]
        );
        assert_eq!(
            scroll(&mut translator, -240),
            [
                Event::new(EV_REL, REL_WHEEL_HI_RES, -240),
                Event::new(EV_REL, REL_WHEEL, -2)
            ]
        );
    }
}
