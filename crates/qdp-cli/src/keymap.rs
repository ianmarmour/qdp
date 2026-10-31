//! The physical keys of the window system, as HID usages (Section 8.4.1).
//!
//! Keys are reported by position, not by character: the key labelled A on a
//! US layout is usage 0x04 whatever layout the user has, and the host applies
//! its own layout.

use winit::keyboard::KeyCode;

/// A key as QDP reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HidKey {
    /// A key on the Keyboard/Keypad page (0x07).
    Keyboard(u8),
    /// A key on the Consumer page (0x0C), such as a volume key.
    Consumer(u16),
}

/// Returns the HID usage of a physical key, if it has one.
pub fn hid_key(code: KeyCode) -> Option<HidKey> {
    use HidKey::{Consumer, Keyboard};
    let key = match code {
        KeyCode::KeyA => Keyboard(0x04),
        KeyCode::KeyB => Keyboard(0x05),
        KeyCode::KeyC => Keyboard(0x06),
        KeyCode::KeyD => Keyboard(0x07),
        KeyCode::KeyE => Keyboard(0x08),
        KeyCode::KeyF => Keyboard(0x09),
        KeyCode::KeyG => Keyboard(0x0a),
        KeyCode::KeyH => Keyboard(0x0b),
        KeyCode::KeyI => Keyboard(0x0c),
        KeyCode::KeyJ => Keyboard(0x0d),
        KeyCode::KeyK => Keyboard(0x0e),
        KeyCode::KeyL => Keyboard(0x0f),
        KeyCode::KeyM => Keyboard(0x10),
        KeyCode::KeyN => Keyboard(0x11),
        KeyCode::KeyO => Keyboard(0x12),
        KeyCode::KeyP => Keyboard(0x13),
        KeyCode::KeyQ => Keyboard(0x14),
        KeyCode::KeyR => Keyboard(0x15),
        KeyCode::KeyS => Keyboard(0x16),
        KeyCode::KeyT => Keyboard(0x17),
        KeyCode::KeyU => Keyboard(0x18),
        KeyCode::KeyV => Keyboard(0x19),
        KeyCode::KeyW => Keyboard(0x1a),
        KeyCode::KeyX => Keyboard(0x1b),
        KeyCode::KeyY => Keyboard(0x1c),
        KeyCode::KeyZ => Keyboard(0x1d),
        KeyCode::Digit1 => Keyboard(0x1e),
        KeyCode::Digit2 => Keyboard(0x1f),
        KeyCode::Digit3 => Keyboard(0x20),
        KeyCode::Digit4 => Keyboard(0x21),
        KeyCode::Digit5 => Keyboard(0x22),
        KeyCode::Digit6 => Keyboard(0x23),
        KeyCode::Digit7 => Keyboard(0x24),
        KeyCode::Digit8 => Keyboard(0x25),
        KeyCode::Digit9 => Keyboard(0x26),
        KeyCode::Digit0 => Keyboard(0x27),
        KeyCode::Enter => Keyboard(0x28),
        KeyCode::Escape => Keyboard(0x29),
        KeyCode::Backspace => Keyboard(0x2a),
        KeyCode::Tab => Keyboard(0x2b),
        KeyCode::Space => Keyboard(0x2c),
        KeyCode::Minus => Keyboard(0x2d),
        KeyCode::Equal => Keyboard(0x2e),
        KeyCode::BracketLeft => Keyboard(0x2f),
        KeyCode::BracketRight => Keyboard(0x30),
        KeyCode::Backslash => Keyboard(0x31),
        KeyCode::Semicolon => Keyboard(0x33),
        KeyCode::Quote => Keyboard(0x34),
        KeyCode::Backquote => Keyboard(0x35),
        KeyCode::Comma => Keyboard(0x36),
        KeyCode::Period => Keyboard(0x37),
        KeyCode::Slash => Keyboard(0x38),
        KeyCode::CapsLock => Keyboard(0x39),
        KeyCode::F1 => Keyboard(0x3a),
        KeyCode::F2 => Keyboard(0x3b),
        KeyCode::F3 => Keyboard(0x3c),
        KeyCode::F4 => Keyboard(0x3d),
        KeyCode::F5 => Keyboard(0x3e),
        KeyCode::F6 => Keyboard(0x3f),
        KeyCode::F7 => Keyboard(0x40),
        KeyCode::F8 => Keyboard(0x41),
        KeyCode::F9 => Keyboard(0x42),
        KeyCode::F10 => Keyboard(0x43),
        KeyCode::F11 => Keyboard(0x44),
        KeyCode::F12 => Keyboard(0x45),
        KeyCode::PrintScreen => Keyboard(0x46),
        KeyCode::ScrollLock => Keyboard(0x47),
        KeyCode::Pause => Keyboard(0x48),
        KeyCode::Insert => Keyboard(0x49),
        KeyCode::Home => Keyboard(0x4a),
        KeyCode::PageUp => Keyboard(0x4b),
        KeyCode::Delete => Keyboard(0x4c),
        KeyCode::End => Keyboard(0x4d),
        KeyCode::PageDown => Keyboard(0x4e),
        KeyCode::ArrowRight => Keyboard(0x4f),
        KeyCode::ArrowLeft => Keyboard(0x50),
        KeyCode::ArrowDown => Keyboard(0x51),
        KeyCode::ArrowUp => Keyboard(0x52),
        KeyCode::NumLock => Keyboard(0x53),
        KeyCode::NumpadDivide => Keyboard(0x54),
        KeyCode::NumpadMultiply => Keyboard(0x55),
        KeyCode::NumpadSubtract => Keyboard(0x56),
        KeyCode::NumpadAdd => Keyboard(0x57),
        KeyCode::NumpadEnter => Keyboard(0x58),
        KeyCode::Numpad1 => Keyboard(0x59),
        KeyCode::Numpad2 => Keyboard(0x5a),
        KeyCode::Numpad3 => Keyboard(0x5b),
        KeyCode::Numpad4 => Keyboard(0x5c),
        KeyCode::Numpad5 => Keyboard(0x5d),
        KeyCode::Numpad6 => Keyboard(0x5e),
        KeyCode::Numpad7 => Keyboard(0x5f),
        KeyCode::Numpad8 => Keyboard(0x60),
        KeyCode::Numpad9 => Keyboard(0x61),
        KeyCode::Numpad0 => Keyboard(0x62),
        KeyCode::NumpadDecimal => Keyboard(0x63),
        KeyCode::IntlBackslash => Keyboard(0x64),
        KeyCode::ContextMenu => Keyboard(0x65),
        KeyCode::Power => Keyboard(0x66),
        KeyCode::NumpadEqual => Keyboard(0x67),
        KeyCode::F13 => Keyboard(0x68),
        KeyCode::F14 => Keyboard(0x69),
        KeyCode::F15 => Keyboard(0x6a),
        KeyCode::F16 => Keyboard(0x6b),
        KeyCode::F17 => Keyboard(0x6c),
        KeyCode::F18 => Keyboard(0x6d),
        KeyCode::F19 => Keyboard(0x6e),
        KeyCode::F20 => Keyboard(0x6f),
        KeyCode::F21 => Keyboard(0x70),
        KeyCode::F22 => Keyboard(0x71),
        KeyCode::F23 => Keyboard(0x72),
        KeyCode::F24 => Keyboard(0x73),
        KeyCode::Help => Keyboard(0x75),
        KeyCode::NumpadComma => Keyboard(0x85),
        KeyCode::IntlRo => Keyboard(0x87),
        KeyCode::KanaMode => Keyboard(0x88),
        KeyCode::IntlYen => Keyboard(0x89),
        KeyCode::Convert => Keyboard(0x8a),
        KeyCode::NonConvert => Keyboard(0x8b),
        KeyCode::Lang1 => Keyboard(0x90),
        KeyCode::Lang2 => Keyboard(0x91),
        KeyCode::ControlLeft => Keyboard(0xe0),
        KeyCode::ShiftLeft => Keyboard(0xe1),
        KeyCode::AltLeft => Keyboard(0xe2),
        KeyCode::SuperLeft => Keyboard(0xe3),
        KeyCode::ControlRight => Keyboard(0xe4),
        KeyCode::ShiftRight => Keyboard(0xe5),
        KeyCode::AltRight => Keyboard(0xe6),
        KeyCode::SuperRight => Keyboard(0xe7),
        KeyCode::MediaTrackNext => Consumer(0x00b5),
        KeyCode::MediaTrackPrevious => Consumer(0x00b6),
        KeyCode::MediaStop => Consumer(0x00b7),
        KeyCode::MediaPlayPause => Consumer(0x00cd),
        KeyCode::AudioVolumeMute => Consumer(0x00e2),
        KeyCode::AudioVolumeUp => Consumer(0x00e9),
        KeyCode::AudioVolumeDown => Consumer(0x00ea),
        _ => return None,
    };
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys map to their positions on the HID Keyboard/Keypad and Consumer
    /// pages.
    #[test]
    fn maps_keys() {
        assert_eq!(hid_key(KeyCode::KeyA), Some(HidKey::Keyboard(0x04)));
        assert_eq!(hid_key(KeyCode::Digit0), Some(HidKey::Keyboard(0x27)));
        assert_eq!(hid_key(KeyCode::Enter), Some(HidKey::Keyboard(0x28)));
        assert_eq!(hid_key(KeyCode::ArrowUp), Some(HidKey::Keyboard(0x52)));
        assert_eq!(hid_key(KeyCode::SuperRight), Some(HidKey::Keyboard(0xe7)));
        assert_eq!(
            hid_key(KeyCode::AudioVolumeUp),
            Some(HidKey::Consumer(0x00e9))
        );
        assert_eq!(hid_key(KeyCode::Fn), None);
    }
}
