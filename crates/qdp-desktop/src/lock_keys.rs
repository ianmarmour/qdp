//! The state of the user's lock keys, which the host's follow when the
//! session gains focus (Section 8.4.1). The window library does not tell
//! it, so it is read from the platform.

use qdp::message::KeyboardLeds;
use qdp_runtime::LockKeys;

/// Returns the state of the lock keys, where the platform tells it: all
/// three on Windows, Caps Lock on macOS, which has no other, and the X11
/// keyboard's LEDs on Linux.
pub fn lock_keys() -> Option<LockKeys> {
    platform::lock_keys()
}

/// Every lock key a keyboard's LEDs show.
#[cfg(any(windows, target_os = "linux", test))]
const ALL: KeyboardLeds = KeyboardLeds(
    KeyboardLeds::NUM_LOCK.0 | KeyboardLeds::CAPS_LOCK.0 | KeyboardLeds::SCROLL_LOCK.0,
);

/// Returns the LEDs of the lock keys that are on, by whether each is.
#[cfg_attr(target_os = "android", allow(dead_code))]
fn leds(num: bool, caps: bool, scroll: bool) -> KeyboardLeds {
    let led = |on: bool, led: KeyboardLeds| if on { led.0 } else { 0 };
    KeyboardLeds(
        led(num, KeyboardLeds::NUM_LOCK)
            | led(caps, KeyboardLeds::CAPS_LOCK)
            | led(scroll, KeyboardLeds::SCROLL_LOCK),
    )
}

/// The lock keys on Windows. `GetKeyState` is a C interface, so this module
/// uses `unsafe`.
#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VIRTUAL_KEY, VK_CAPITAL, VK_NUMLOCK, VK_SCROLL,
    };

    use super::{ALL, LockKeys, leds};

    /// The bit of `GetKeyState` that says a toggle key is on.
    const TOGGLED: i16 = 0x0001;

    /// Returns the state of all three lock keys.
    pub(super) fn lock_keys() -> Option<LockKeys> {
        // SAFETY: GetKeyState takes any virtual key and cannot fail.
        let on = |key: VIRTUAL_KEY| unsafe { GetKeyState(i32::from(key.0)) } & TOGGLED != 0;
        Some(LockKeys {
            known: ALL,
            on: leds(on(VK_NUMLOCK), on(VK_CAPITAL), on(VK_SCROLL)),
        })
    }
}

/// The lock keys on macOS: Caps Lock alone.
#[cfg(target_os = "macos")]
mod platform {
    use objc2_core_graphics::{CGEventFlags, CGEventSource, CGEventSourceStateID};
    use qdp::message::KeyboardLeds;

    use super::{LockKeys, leds};

    /// Returns the state of Caps Lock.
    pub(super) fn lock_keys() -> Option<LockKeys> {
        let flags = CGEventSource::flags_state(CGEventSourceStateID::CombinedSessionState);
        Some(LockKeys {
            known: KeyboardLeds::CAPS_LOCK,
            on: leds(false, flags.contains(CGEventFlags::MaskAlphaShift), false),
        })
    }
}

/// The lock keys on Linux, from the X11 keyboard's LEDs, which XWayland
/// keeps too.
#[cfg(target_os = "linux")]
mod platform {
    use x11rb::protocol::xproto::ConnectionExt;

    use super::{ALL, LockKeys, leds};

    /// The X11 LED masks of the lock keys, as xkeyboard-config numbers the
    /// indicators: 1 Caps Lock, 2 Num Lock, 3 Scroll Lock.
    const CAPS_LED: u32 = 1 << 0;
    /// Num Lock's LED.
    const NUM_LED: u32 = 1 << 1;
    /// Scroll Lock's LED.
    const SCROLL_LED: u32 = 1 << 2;

    /// Returns the state of the lock keys from the X11 keyboard's LEDs, if
    /// an X11 display is there.
    pub(super) fn lock_keys() -> Option<LockKeys> {
        let (connection, _) = x11rb::connect(None).ok()?;
        let mask = connection
            .get_keyboard_control()
            .ok()?
            .reply()
            .ok()?
            .led_mask;
        Some(LockKeys {
            known: ALL,
            on: leds(
                mask & NUM_LED != 0,
                mask & CAPS_LED != 0,
                mask & SCROLL_LED != 0,
            ),
        })
    }
}

/// The lock keys elsewhere: not known.
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod platform {
    use super::LockKeys;

    /// Returns nothing: the platform does not tell.
    pub(super) fn lock_keys() -> Option<LockKeys> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lock keys become their LEDs.
    #[test]
    fn maps_lock_keys_to_leds() {
        assert_eq!(leds(true, false, true), KeyboardLeds(0x05));
        assert_eq!(leds(false, true, false), KeyboardLeds::CAPS_LOCK);
        assert_eq!(ALL, KeyboardLeds(0x07));
    }
}
