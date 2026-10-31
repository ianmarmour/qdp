//! Linux key codes for HID usages, as the kernel's own HID driver maps them
//! (`hid_keyboard` in `drivers/hid/hid-input.c`), so that virtual keys behave
//! exactly like the same keys on a USB keyboard.

/// A code that names no key.
const NONE: u16 = 0;

/// The Linux key code of each Keyboard/Keypad page (0x07) usage, or [`NONE`].
#[rustfmt::skip]
const KEYBOARD_PAGE: [u16; 256] = [
    // 0x00: error codes, then A to L.
      0,   0,   0,   0,  30,  48,  46,  32,  18,  33,  34,  35,  23,  36,  37,  38,
    // 0x10: M to Z, then 1 and 2.
     50,  49,  24,  25,  16,  19,  31,  20,  22,  47,  17,  45,  21,  44,   2,   3,
    // 0x20: 3 to 0, Enter, Escape, Backspace, Tab, Space, - = [
      4,   5,   6,   7,   8,   9,  10,  11,  28,   1,  14,  15,  57,  12,  13,  26,
    // 0x30: ] \ (non-US #) ; ' ` , . / Caps Lock, F1 to F6.
     27,  43,  43,  39,  40,  41,  51,  52,  53,  58,  59,  60,  61,  62,  63,  64,
    // 0x40: F7 to F12, Print Screen, Scroll Lock, Pause, Insert, Home, Page Up,
    // Delete, End, Page Down, Right.
     65,  66,  67,  68,  87,  88,  99,  70, 119, 110, 102, 104, 111, 107, 109, 106,
    // 0x50: Left, Down, Up, Num Lock, keypad / * - + Enter 1 to 7.
    105, 108, 103,  69,  98,  55,  74,  78,  96,  79,  80,  81,  75,  76,  77,  71,
    // 0x60: keypad 8 9 0 ., non-US \, Application, Power, keypad =, F13 to F20.
     72,  73,  82,  83,  86, 127, 116, 117, 183, 184, 185, 186, 187, 188, 189, 190,
    // 0x70: F21 to F24, Execute, Help, Menu, Select, Stop, Again, Undo, Cut,
    // Copy, Paste, Find, Mute.
    191, 192, 193, 194, 134, 138, 130, 132, 128, 129, 131, 137, 133, 135, 136, 113,
    // 0x80: Volume Up, Volume Down, locking keys, keypad comma, International 1
    // to 6.
    115, 114,   0,   0,   0, 121,   0,  89,  93, 124,  92,  94,  95,   0,   0,   0,
    // 0x90: LANG1 to LANG5, then reserved; 0x9C is Clear.
    122, 123,  90,  91,  85,   0,   0,   0,   0,   0,   0,   0, 111,   0,   0,   0,
    // 0xA0: reserved.
      0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    // 0xB0: keypad ( and ) at 0xB6 and 0xB7.
      0,   0,   0,   0,   0,   0, 179, 180,   0,   0,   0,   0,   0,   0,   0,   0,
    // 0xC0: reserved.
      0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    // 0xD0: keypad Clear Entry at 0xD8.
      0,   0,   0,   0,   0,   0,   0,   0, 111,   0,   0,   0,   0,   0,   0,   0,
    // 0xE0: Left Control, Shift, Alt, Meta, Right Control, Shift, Alt, Meta,
    // then media keys.
     29,  42,  56, 125,  97,  54, 100, 126, 164, 166, 165, 163, 161, 115, 114, 113,
    // 0xF0: more media keys.
    150, 158, 159, 128, 136, 177, 178, 176, 142, 152, 173, 140,   0,   0,   0,   0,
];

/// The Consumer page (0x0C) usages QDP clients send, with their key codes.
const CONSUMER_PAGE: [(u16, u16); 7] = [
    (0x00b5, 163), // Scan Next Track: KEY_NEXTSONG
    (0x00b6, 165), // Scan Previous Track: KEY_PREVIOUSSONG
    (0x00b7, 166), // Stop: KEY_STOPCD
    (0x00cd, 164), // Play/Pause: KEY_PLAYPAUSE
    (0x00e2, 113), // Mute: KEY_MUTE
    (0x00e9, 115), // Volume Increment: KEY_VOLUMEUP
    (0x00ea, 114), // Volume Decrement: KEY_VOLUMEDOWN
];

/// Returns the key code of a Keyboard/Keypad page usage, if it has one.
pub(super) fn keyboard_key(usage: u8) -> Option<u16> {
    Some(KEYBOARD_PAGE[usize::from(usage)]).filter(|&code| code != NONE)
}

/// Returns the key code of a Consumer page usage, if it has one.
pub(super) fn consumer_key(usage: u16) -> Option<u16> {
    CONSUMER_PAGE
        .iter()
        .find(|(consumer, _)| *consumer == usage)
        .map(|&(_, code)| code)
}

/// Returns every key code the virtual keyboard can produce.
pub(super) fn all_keys() -> impl Iterator<Item = u16> {
    KEYBOARD_PAGE
        .into_iter()
        .chain(CONSUMER_PAGE.into_iter().map(|(_, code)| code))
        .filter(|&code| code != NONE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Usages map to the codes of `linux/input-event-codes.h`.
    #[test]
    fn maps_usages() {
        assert_eq!(keyboard_key(0x04), Some(30)); // KEY_A
        assert_eq!(keyboard_key(0x1d), Some(44)); // KEY_Z
        assert_eq!(keyboard_key(0x27), Some(11)); // KEY_0
        assert_eq!(keyboard_key(0x28), Some(28)); // KEY_ENTER
        assert_eq!(keyboard_key(0x52), Some(103)); // KEY_UP
        assert_eq!(keyboard_key(0xe0), Some(29)); // KEY_LEFTCTRL
        assert_eq!(keyboard_key(0xe7), Some(126)); // KEY_RIGHTMETA
        assert_eq!(keyboard_key(0x00), None);
        assert_eq!(keyboard_key(0xa5), None);
        assert_eq!(consumer_key(0x00e9), Some(115)); // KEY_VOLUMEUP
        assert_eq!(consumer_key(0x0001), None);
        assert!(all_keys().all(|code| code > 0 && code < 0x2ff));
    }
}
