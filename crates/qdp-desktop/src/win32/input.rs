//! Input injection on Windows with `SendInput`.
//!
//! Windows keeps processes from sending input to windows of processes with a
//! higher integrity level, such as programs run as administrator, and
//! `SendInput` reports no error when it drops input for that reason; running
//! the host as administrator lets it control those windows too. The secure
//! desktop (the sign-in screen, Ctrl+Alt+Delete and elevation prompts) takes
//! no injected input at all.

use std::io;

use qdp::message::KeyboardLeds;
use qdp_runtime::{InputAction, InputInjector, PointerButton};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE,
    MAPVK_VSC_TO_VK_EX, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL,
    MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, MapVirtualKeyW, SendInput, VIRTUAL_KEY,
    VK_CAPITAL, VK_NUMLOCK, VK_SCROLL,
};

use super::keycodes::{ScanCode, WindowsKey, consumer_key, keyboard_key};

/// QDP positions and motion are in 1/256 pixel or count (Section 1.5.6).
const SUBUNITS: i64 = 256;

/// The largest normalized absolute coordinate: `SendInput` maps 0 to 65535
/// across the primary monitor.
const NORMALIZED_MAX: i64 = 65535;

/// `XBUTTON1`: the Back button's number in `mouseData`.
const XBUTTON1: u32 = 1;

/// `XBUTTON2`: the Forward button's number in `mouseData`.
const XBUTTON2: u32 = 2;

/// The bit of `GetKeyState` that reports a toggle key as on.
const TOGGLED: i16 = 1;

/// The toggle keys whose state the keyboard LEDs show.
const LEDS: [(VIRTUAL_KEY, KeyboardLeds); 3] = [
    (VK_NUMLOCK, KeyboardLeds::NUM_LOCK),
    (VK_CAPITAL, KeyboardLeds::CAPS_LOCK),
    (VK_SCROLL, KeyboardLeds::SCROLL_LOCK),
];

/// Whether a key or button goes down or up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    /// Pressed.
    Down,
    /// Released.
    Up,
}

/// Returns a keyboard input.
fn keyboard(virtual_key: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: virtual_key,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Returns a mouse input.
fn mouse(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// Returns the input pressing or releasing a key.
fn key(key: WindowsKey, direction: Direction) -> INPUT {
    let release = match direction {
        Direction::Down => KEYBD_EVENT_FLAGS(0),
        Direction::Up => KEYEVENTF_KEYUP,
    };
    let extended = |scan: ScanCode| {
        if scan.is_extended() {
            KEYEVENTF_EXTENDEDKEY
        } else {
            KEYBD_EVENT_FLAGS(0)
        }
    };
    match key {
        WindowsKey::Scan(scan) => {
            // Windows ignores the virtual key of scan code input and
            // translates the scan code with the receiving window's layout.
            // The virtual key is filled in for software that reads it
            // regardless, such as Wine.
            // SAFETY: MapVirtualKeyW takes any code, returning 0 for none.
            let translated = unsafe { MapVirtualKeyW(u32::from(scan.0), MAPVK_VSC_TO_VK_EX) };
            keyboard(
                VIRTUAL_KEY(translated as u16),
                scan.code(),
                KEYEVENTF_SCANCODE | extended(scan) | release,
            )
        }
        WindowsKey::Virtual(virtual_key, scan) => {
            keyboard(virtual_key, scan.code(), extended(scan) | release)
        }
    }
}

/// Returns the mouse flags and `mouseData` pressing or releasing a button.
fn button(button: PointerButton, direction: Direction) -> (MOUSE_EVENT_FLAGS, u32) {
    let (down, up, data) = match button {
        PointerButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, 0),
        PointerButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, 0),
        PointerButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
        PointerButton::Back => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, XBUTTON1),
        PointerButton::Forward => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, XBUTTON2),
    };
    match direction {
        Direction::Down => (down, data),
        Direction::Up => (up, data),
    }
}

/// Turns input actions into `SendInput` inputs for a `width × height` pixel
/// primary monitor.
#[derive(Debug)]
struct Translator {
    /// The monitor's size in pixels.
    size: (i64, i64),
    /// Relative motion not yet sent as whole counts, in 1/256 count.
    motion_remainder: (i64, i64),
}

impl Translator {
    /// Returns a translator for a `width × height` pixel primary monitor.
    fn new(width: u32, height: u32) -> Self {
        Self {
            size: (i64::from(width.max(2)), i64::from(height.max(2))),
            motion_remainder: (0, 0),
        }
    }

    /// Returns the normalized absolute coordinate of a position in 1/256
    /// pixel along an axis `extent` pixels long.
    fn normalize(position: i32, extent: i64) -> i32 {
        let normalized = i64::from(position) * NORMALIZED_MAX / (SUBUNITS * (extent - 1));
        normalized.clamp(0, NORMALIZED_MAX) as i32
    }

    /// Returns the inputs performing `action`, or an error if Windows has
    /// no equivalent.
    fn inputs(&mut self, action: &InputAction) -> Result<Vec<INPUT>, String> {
        Ok(match action {
            &InputAction::KeyDown(usage) | &InputAction::KeyUp(usage) => {
                let windows_key = keyboard_key(usage).ok_or("the key has no Windows scan code")?;
                let direction = match action {
                    InputAction::KeyDown(_) => Direction::Down,
                    _ => Direction::Up,
                };
                vec![key(windows_key, direction)]
            }
            &InputAction::ConsumerKeyDown(usage) | &InputAction::ConsumerKeyUp(usage) => {
                let windows_key = consumer_key(usage).ok_or("the media key has no Windows key")?;
                let direction = match action {
                    InputAction::ConsumerKeyDown(_) => Direction::Down,
                    _ => Direction::Up,
                };
                vec![key(windows_key, direction)]
            }
            &InputAction::PointerMoveTo { x, y } => vec![mouse(
                Self::normalize(x, self.size.0),
                Self::normalize(y, self.size.1),
                0,
                MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
            )],
            &InputAction::MouseMoveBy { dx, dy } => {
                let dx = self.motion_remainder.0 + i64::from(dx);
                let dy = self.motion_remainder.1 + i64::from(dy);
                self.motion_remainder = (dx % SUBUNITS, dy % SUBUNITS);
                let counts = ((dx / SUBUNITS) as i32, (dy / SUBUNITS) as i32);
                if counts == (0, 0) {
                    return Ok(Vec::new());
                }
                vec![mouse(counts.0, counts.1, 0, MOUSEEVENTF_MOVE)]
            }
            &InputAction::ButtonDown(pressed) => {
                let (flags, data) = button(pressed, Direction::Down);
                vec![mouse(0, 0, data, flags)]
            }
            &InputAction::ButtonUp(released) => {
                let (flags, data) = button(released, Direction::Up);
                vec![mouse(0, 0, data, flags)]
            }
            // Windows scrolls in 1/120 detent (`WHEEL_DELTA`) as QDP does, so
            // high-resolution scrolling passes through unchanged. A negative
            // amount is sent as its two's complement.
            &InputAction::Scroll {
                vertical,
                horizontal,
            } => [
                (vertical, MOUSEEVENTF_WHEEL),
                (horizontal, MOUSEEVENTF_HWHEEL),
            ]
            .into_iter()
            .filter(|&(amount, _)| amount != 0)
            .map(|(amount, flags)| mouse(0, 0, amount as u32, flags))
            .collect(),
            InputAction::Text(text) => text
                .encode_utf16()
                .flat_map(|unit| {
                    [
                        keyboard(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE),
                        keyboard(VIRTUAL_KEY(0), unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                    ]
                })
                .collect(),
        })
    }
}

/// Injects input with `SendInput` on the primary monitor, which is the
/// monitor Desktop Duplication captures.
#[derive(Debug)]
pub struct SendInputInjector {
    /// Turns actions into inputs.
    translator: Translator,
}

impl SendInputInjector {
    /// Returns an injector for the primary monitor, captured at `width ×
    /// height` pixels.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            translator: Translator::new(width, height),
        }
    }
}

impl InputInjector for SendInputInjector {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        let inputs = self.translator.inputs(action)?;
        if inputs.is_empty() {
            return Ok(());
        }
        // SAFETY: each input is a fully initialised `INPUT` of the size
        // given.
        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
        if sent as usize != inputs.len() {
            return Err(format!(
                "Windows refused the input: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    fn description(&self) -> &str {
        "SendInput"
    }

    fn keyboard_leds(&mut self) -> KeyboardLeds {
        let lit = LEDS
            .iter()
            // SAFETY: GetKeyState takes any virtual key code.
            .filter(|(key, _)| unsafe { GetKeyState(i32::from(key.0)) } & TOGGLED != 0)
            .fold(0, |leds, (_, led)| leds | led.0);
        KeyboardLeds(lit)
    }

    fn supports_text(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns the keyboard part of an input.
    fn keyboard_part(input: &INPUT) -> KEYBDINPUT {
        assert_eq!(input.r#type, INPUT_KEYBOARD);
        // SAFETY: keyboard inputs hold the `ki` variant.
        unsafe { input.Anonymous.ki }
    }

    /// Returns the mouse part of an input.
    fn mouse_part(input: &INPUT) -> MOUSEINPUT {
        assert_eq!(input.r#type, INPUT_MOUSE);
        // SAFETY: mouse inputs hold the `mi` variant.
        unsafe { input.Anonymous.mi }
    }

    /// Keys become scan codes, flagged as extended where they are.
    #[test]
    fn translates_keys() {
        let mut translator = Translator::new(1920, 1080);
        let inputs = translator.inputs(&InputAction::KeyDown(0x04)).unwrap();
        let a = keyboard_part(&inputs[0]);
        assert_eq!((a.wScan, a.dwFlags), (0x1e, KEYEVENTF_SCANCODE));
        assert_ne!(a.wVk, VIRTUAL_KEY(0));
        let inputs = translator.inputs(&InputAction::KeyUp(0x52)).unwrap();
        let up = keyboard_part(&inputs[0]);
        assert_eq!((up.wVk.0, up.wScan), (0x26, 0x48)); // VK_UP
        assert_eq!(
            up.dwFlags,
            KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP
        );
        let inputs = translator
            .inputs(&InputAction::ConsumerKeyDown(0x00cd))
            .unwrap();
        let play = keyboard_part(&inputs[0]);
        assert_eq!(play.wVk.0, 0xb3); // VK_MEDIA_PLAY_PAUSE
        assert_eq!(play.dwFlags, KEYEVENTF_EXTENDEDKEY);
        assert!(translator.inputs(&InputAction::KeyDown(0x00)).is_err());
    }

    /// Text becomes a press and release of each UTF-16 unit.
    #[test]
    fn translates_text() {
        let mut translator = Translator::new(1920, 1080);
        let inputs = translator
            .inputs(&InputAction::Text("é😀".to_owned()))
            .unwrap();
        let units: Vec<(u16, KEYBD_EVENT_FLAGS)> = inputs
            .iter()
            .map(keyboard_part)
            .map(|key| (key.wScan, key.dwFlags))
            .collect();
        let up = KEYEVENTF_UNICODE | KEYEVENTF_KEYUP;
        assert_eq!(
            units,
            [
                (0x00e9, KEYEVENTF_UNICODE),
                (0x00e9, up),
                (0xd83d, KEYEVENTF_UNICODE),
                (0xd83d, up),
                (0xde00, KEYEVENTF_UNICODE),
                (0xde00, up),
            ]
        );
    }

    /// Absolute positions are normalized across the monitor, and relative
    /// motion keeps its fractions.
    #[test]
    fn translates_motion() {
        let mut translator = Translator::new(1921, 1081);
        let inputs = translator
            .inputs(&InputAction::PointerMoveTo {
                x: 960 * 256,
                y: 1080 * 256,
            })
            .unwrap();
        let moved = mouse_part(&inputs[0]);
        assert_eq!((moved.dx, moved.dy), (32767, 65535));
        assert_eq!(moved.dwFlags, MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE);
        let inputs = translator
            .inputs(&InputAction::PointerMoveTo { x: -5, y: i32::MAX })
            .unwrap();
        let clamped = mouse_part(&inputs[0]);
        assert_eq!((clamped.dx, clamped.dy), (0, 65535));

        let half = InputAction::MouseMoveBy { dx: 128, dy: -128 };
        assert!(translator.inputs(&half).unwrap().is_empty());
        let inputs = translator.inputs(&half).unwrap();
        let relative = mouse_part(&inputs[0]);
        assert_eq!((relative.dx, relative.dy), (1, -1));
        assert_eq!(relative.dwFlags, MOUSEEVENTF_MOVE);
    }

    /// Buttons and scrolling become mouse events.
    #[test]
    fn translates_buttons_and_scrolling() {
        let mut translator = Translator::new(1920, 1080);
        let inputs = translator
            .inputs(&InputAction::ButtonDown(PointerButton::Forward))
            .unwrap();
        let forward = mouse_part(&inputs[0]);
        assert_eq!(
            (forward.dwFlags, forward.mouseData),
            (MOUSEEVENTF_XDOWN, XBUTTON2)
        );
        let inputs = translator
            .inputs(&InputAction::ButtonUp(PointerButton::Left))
            .unwrap();
        assert_eq!(mouse_part(&inputs[0]).dwFlags, MOUSEEVENTF_LEFTUP);

        let inputs = translator
            .inputs(&InputAction::Scroll {
                vertical: -60,
                horizontal: 240,
            })
            .unwrap();
        let wheel = mouse_part(&inputs[0]);
        assert_eq!(
            (wheel.dwFlags, wheel.mouseData as i32),
            (MOUSEEVENTF_WHEEL, -60)
        );
        let tilt = mouse_part(&inputs[1]);
        assert_eq!((tilt.dwFlags, tilt.mouseData), (MOUSEEVENTF_HWHEEL, 240));
        let none = InputAction::Scroll {
            vertical: 0,
            horizontal: 0,
        };
        assert!(translator.inputs(&none).unwrap().is_empty());
    }
}
