//! Android's key codes, as `KeyEvent.getKeyCode()` gives them, mapped to the
//! HID usages QDP sends: the keyboard page for keys, the consumer page for
//! media and volume keys.

/// A HID usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidKey {
    /// A usage on the keyboard page (0x07).
    Keyboard(u8),
    /// A usage on the consumer page (0x0C).
    Consumer(u16),
}

/// Android's key code for the A key; B to Z follow it.
const KEYCODE_A: i32 = 29;
/// Android's key code for the Z key.
const KEYCODE_Z: i32 = 54;
/// The HID usage of the A key; B to Z follow it.
const USAGE_A: u8 = 0x04;
/// Android's key code for the 0 key; 1 to 9 follow it.
const KEYCODE_0: i32 = 7;
/// Android's key code for the 9 key.
const KEYCODE_9: i32 = 16;
/// The HID usage of the 1 key; 2 to 9 follow it, then 0.
const USAGE_1: u8 = 0x1E;
/// The HID usage of the 0 key.
const USAGE_0: u8 = 0x27;
/// Android's key code for F1; F2 to F12 follow it.
const KEYCODE_F1: i32 = 131;
/// Android's key code for F12.
const KEYCODE_F12: i32 = 142;
/// The HID usage of F1; F2 to F12 follow it.
const USAGE_F1: u8 = 0x3A;
/// Android's key code for the keypad's 0; 1 to 9 follow it.
const KEYCODE_NUMPAD_0: i32 = 144;
/// Android's key code for the keypad's 9.
const KEYCODE_NUMPAD_9: i32 = 153;
/// The HID usage of the keypad's 1; 2 to 9 follow it, then 0.
const USAGE_KEYPAD_1: u8 = 0x59;
/// The HID usage of the keypad's 0.
const USAGE_KEYPAD_0: u8 = 0x62;

/// The other keys: Android's key code, and the HID usage.
const KEYS: &[(i32, HidKey)] = &[
    (66, HidKey::Keyboard(0x28)),    // ENTER
    (111, HidKey::Keyboard(0x29)),   // ESCAPE
    (67, HidKey::Keyboard(0x2A)),    // DEL: backspace
    (61, HidKey::Keyboard(0x2B)),    // TAB
    (62, HidKey::Keyboard(0x2C)),    // SPACE
    (69, HidKey::Keyboard(0x2D)),    // MINUS
    (70, HidKey::Keyboard(0x2E)),    // EQUALS
    (71, HidKey::Keyboard(0x2F)),    // LEFT_BRACKET
    (72, HidKey::Keyboard(0x30)),    // RIGHT_BRACKET
    (73, HidKey::Keyboard(0x31)),    // BACKSLASH
    (74, HidKey::Keyboard(0x33)),    // SEMICOLON
    (75, HidKey::Keyboard(0x34)),    // APOSTROPHE
    (68, HidKey::Keyboard(0x35)),    // GRAVE
    (55, HidKey::Keyboard(0x36)),    // COMMA
    (56, HidKey::Keyboard(0x37)),    // PERIOD
    (76, HidKey::Keyboard(0x38)),    // SLASH
    (115, HidKey::Keyboard(0x39)),   // CAPS_LOCK
    (120, HidKey::Keyboard(0x46)),   // SYSRQ: print screen
    (116, HidKey::Keyboard(0x47)),   // SCROLL_LOCK
    (121, HidKey::Keyboard(0x48)),   // BREAK: pause
    (124, HidKey::Keyboard(0x49)),   // INSERT
    (122, HidKey::Keyboard(0x4A)),   // MOVE_HOME
    (92, HidKey::Keyboard(0x4B)),    // PAGE_UP
    (112, HidKey::Keyboard(0x4C)),   // FORWARD_DEL: delete
    (123, HidKey::Keyboard(0x4D)),   // MOVE_END
    (93, HidKey::Keyboard(0x4E)),    // PAGE_DOWN
    (22, HidKey::Keyboard(0x4F)),    // DPAD_RIGHT
    (21, HidKey::Keyboard(0x50)),    // DPAD_LEFT
    (20, HidKey::Keyboard(0x51)),    // DPAD_DOWN
    (19, HidKey::Keyboard(0x52)),    // DPAD_UP
    (143, HidKey::Keyboard(0x53)),   // NUM_LOCK
    (154, HidKey::Keyboard(0x54)),   // NUMPAD_DIVIDE
    (155, HidKey::Keyboard(0x55)),   // NUMPAD_MULTIPLY
    (156, HidKey::Keyboard(0x56)),   // NUMPAD_SUBTRACT
    (157, HidKey::Keyboard(0x57)),   // NUMPAD_ADD
    (160, HidKey::Keyboard(0x58)),   // NUMPAD_ENTER
    (158, HidKey::Keyboard(0x63)),   // NUMPAD_DOT
    (82, HidKey::Keyboard(0x65)),    // MENU: application
    (113, HidKey::Keyboard(0xE0)),   // CTRL_LEFT
    (59, HidKey::Keyboard(0xE1)),    // SHIFT_LEFT
    (57, HidKey::Keyboard(0xE2)),    // ALT_LEFT
    (117, HidKey::Keyboard(0xE3)),   // META_LEFT
    (114, HidKey::Keyboard(0xE4)),   // CTRL_RIGHT
    (60, HidKey::Keyboard(0xE5)),    // SHIFT_RIGHT
    (58, HidKey::Keyboard(0xE6)),    // ALT_RIGHT
    (118, HidKey::Keyboard(0xE7)),   // META_RIGHT
    (24, HidKey::Consumer(0x00E9)),  // VOLUME_UP
    (25, HidKey::Consumer(0x00EA)),  // VOLUME_DOWN
    (164, HidKey::Consumer(0x00E2)), // VOLUME_MUTE
    (85, HidKey::Consumer(0x00CD)),  // MEDIA_PLAY_PAUSE
    (86, HidKey::Consumer(0x00B7)),  // MEDIA_STOP
    (87, HidKey::Consumer(0x00B5)),  // MEDIA_NEXT
    (88, HidKey::Consumer(0x00B6)),  // MEDIA_PREVIOUS
];

/// Returns the HID usage of Android key code `key_code`, if QDP sends the
/// key.
pub fn hid_key(key_code: i32) -> Option<HidKey> {
    let offset = |base: i32| u8::try_from(key_code - base).ok();
    match key_code {
        KEYCODE_A..=KEYCODE_Z => Some(HidKey::Keyboard(USAGE_A + offset(KEYCODE_A)?)),
        KEYCODE_0 => Some(HidKey::Keyboard(USAGE_0)),
        _ if (KEYCODE_0 + 1..=KEYCODE_9).contains(&key_code) => {
            Some(HidKey::Keyboard(USAGE_1 + offset(KEYCODE_0 + 1)?))
        }
        KEYCODE_F1..=KEYCODE_F12 => Some(HidKey::Keyboard(USAGE_F1 + offset(KEYCODE_F1)?)),
        KEYCODE_NUMPAD_0 => Some(HidKey::Keyboard(USAGE_KEYPAD_0)),
        _ if (KEYCODE_NUMPAD_0 + 1..=KEYCODE_NUMPAD_9).contains(&key_code) => Some(
            HidKey::Keyboard(USAGE_KEYPAD_1 + offset(KEYCODE_NUMPAD_0 + 1)?),
        ),
        _ => KEYS
            .iter()
            .find(|(code, _)| *code == key_code)
            .map(|(_, key)| *key),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Letters, digits, function keys and the keypad follow their ranges;
    /// the others come from the table, and unknown keys are not sent.
    #[test]
    fn maps_key_codes() {
        assert_eq!(hid_key(29), Some(HidKey::Keyboard(0x04)));
        assert_eq!(hid_key(54), Some(HidKey::Keyboard(0x1D)));
        assert_eq!(hid_key(7), Some(HidKey::Keyboard(0x27)));
        assert_eq!(hid_key(8), Some(HidKey::Keyboard(0x1E)));
        assert_eq!(hid_key(16), Some(HidKey::Keyboard(0x26)));
        assert_eq!(hid_key(131), Some(HidKey::Keyboard(0x3A)));
        assert_eq!(hid_key(142), Some(HidKey::Keyboard(0x45)));
        assert_eq!(hid_key(144), Some(HidKey::Keyboard(0x62)));
        assert_eq!(hid_key(145), Some(HidKey::Keyboard(0x59)));
        assert_eq!(hid_key(153), Some(HidKey::Keyboard(0x61)));
        assert_eq!(hid_key(66), Some(HidKey::Keyboard(0x28)));
        assert_eq!(hid_key(24), Some(HidKey::Consumer(0x00E9)));
        assert_eq!(hid_key(4), None);
        assert_eq!(hid_key(-1), None);
    }

    /// No key code is mapped twice.
    #[test]
    fn maps_each_key_code_once() {
        for (index, (code, _)) in KEYS.iter().enumerate() {
            assert!(
                KEYS[index + 1..].iter().all(|(other, _)| other != code),
                "{code}"
            );
            assert!(
                !(KEYCODE_A..=KEYCODE_Z).contains(code)
                    && !(KEYCODE_0..=KEYCODE_9).contains(code)
                    && !(KEYCODE_F1..=KEYCODE_F12).contains(code)
                    && !(KEYCODE_NUMPAD_0..=KEYCODE_NUMPAD_9).contains(code),
                "{code}"
            );
        }
    }
}
