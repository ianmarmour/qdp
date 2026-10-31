//! Windows keys for HID usages, as Microsoft's own HID keyboard driver
//! translates them (the USB HID to PS/2 scan code table of Microsoft's
//! Keyboard Scan Code Specification), so that injected keys behave exactly
//! like the same keys on a USB keyboard under every keyboard layout.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    VIRTUAL_KEY, VK_MEDIA_NEXT_TRACK, VK_MEDIA_PLAY_PAUSE, VK_MEDIA_PREV_TRACK, VK_MEDIA_STOP,
    VK_PAUSE, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP,
};

/// A scan code set 1 make code. Extended keys, which keyboards send after an
/// `E0` prefix, carry that prefix in the high byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ScanCode(pub(super) u16);

impl ScanCode {
    /// The prefix byte of extended keys, in the high byte.
    const EXTENDED_PREFIX: u16 = 0xe000;

    /// Returns whether the key is an extended key.
    pub(super) const fn is_extended(self) -> bool {
        self.0 & Self::EXTENDED_PREFIX == Self::EXTENDED_PREFIX
    }

    /// Returns the make code without its prefix.
    pub(super) const fn code(self) -> u16 {
        self.0 & 0x00ff
    }
}

/// A key as `SendInput` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WindowsKey {
    /// A key named by its scan code, a physical position, which Windows
    /// translates with the active keyboard layout.
    Scan(ScanCode),
    /// A key named by a virtual key code, reported with a scan code: for keys
    /// whose scan code is a sequence a single event cannot carry, and for
    /// media keys, which keyboards report on the Consumer page.
    Virtual(VIRTUAL_KEY, ScanCode),
}

/// The scan code of each Keyboard/Keypad page (0x07) usage that has one,
/// except Pause, whose code is the sequence `E1 1D 45`.
const KEYBOARD_PAGE: [(u8, u16); 134] = [
    (0x04, 0x1e),   // A
    (0x05, 0x30),   // B
    (0x06, 0x2e),   // C
    (0x07, 0x20),   // D
    (0x08, 0x12),   // E
    (0x09, 0x21),   // F
    (0x0a, 0x22),   // G
    (0x0b, 0x23),   // H
    (0x0c, 0x17),   // I
    (0x0d, 0x24),   // J
    (0x0e, 0x25),   // K
    (0x0f, 0x26),   // L
    (0x10, 0x32),   // M
    (0x11, 0x31),   // N
    (0x12, 0x18),   // O
    (0x13, 0x19),   // P
    (0x14, 0x10),   // Q
    (0x15, 0x13),   // R
    (0x16, 0x1f),   // S
    (0x17, 0x14),   // T
    (0x18, 0x16),   // U
    (0x19, 0x2f),   // V
    (0x1a, 0x11),   // W
    (0x1b, 0x2d),   // X
    (0x1c, 0x15),   // Y
    (0x1d, 0x2c),   // Z
    (0x1e, 0x02),   // 1
    (0x1f, 0x03),   // 2
    (0x20, 0x04),   // 3
    (0x21, 0x05),   // 4
    (0x22, 0x06),   // 5
    (0x23, 0x07),   // 6
    (0x24, 0x08),   // 7
    (0x25, 0x09),   // 8
    (0x26, 0x0a),   // 9
    (0x27, 0x0b),   // 0
    (0x28, 0x1c),   // Enter
    (0x29, 0x01),   // Escape
    (0x2a, 0x0e),   // Backspace
    (0x2b, 0x0f),   // Tab
    (0x2c, 0x39),   // Space
    (0x2d, 0x0c),   // -
    (0x2e, 0x0d),   // =
    (0x2f, 0x1a),   // [
    (0x30, 0x1b),   // ]
    (0x31, 0x2b),   // \
    (0x32, 0x2b),   // Non-US #
    (0x33, 0x27),   // ;
    (0x34, 0x28),   // '
    (0x35, 0x29),   // `
    (0x36, 0x33),   // ,
    (0x37, 0x34),   // .
    (0x38, 0x35),   // /
    (0x39, 0x3a),   // Caps Lock
    (0x3a, 0x3b),   // F1
    (0x3b, 0x3c),   // F2
    (0x3c, 0x3d),   // F3
    (0x3d, 0x3e),   // F4
    (0x3e, 0x3f),   // F5
    (0x3f, 0x40),   // F6
    (0x40, 0x41),   // F7
    (0x41, 0x42),   // F8
    (0x42, 0x43),   // F9
    (0x43, 0x44),   // F10
    (0x44, 0x57),   // F11
    (0x45, 0x58),   // F12
    (0x46, 0xe037), // Print Screen
    (0x47, 0x46),   // Scroll Lock
    (0x49, 0xe052), // Insert
    (0x4a, 0xe047), // Home
    (0x4b, 0xe049), // Page Up
    (0x4c, 0xe053), // Delete
    (0x4d, 0xe04f), // End
    (0x4e, 0xe051), // Page Down
    (0x4f, 0xe04d), // Right
    (0x50, 0xe04b), // Left
    (0x51, 0xe050), // Down
    (0x52, 0xe048), // Up
    (0x53, 0x45),   // Num Lock
    (0x54, 0xe035), // Keypad /
    (0x55, 0x37),   // Keypad *
    (0x56, 0x4a),   // Keypad -
    (0x57, 0x4e),   // Keypad +
    (0x58, 0xe01c), // Keypad Enter
    (0x59, 0x4f),   // Keypad 1
    (0x5a, 0x50),   // Keypad 2
    (0x5b, 0x51),   // Keypad 3
    (0x5c, 0x4b),   // Keypad 4
    (0x5d, 0x4c),   // Keypad 5
    (0x5e, 0x4d),   // Keypad 6
    (0x5f, 0x47),   // Keypad 7
    (0x60, 0x48),   // Keypad 8
    (0x61, 0x49),   // Keypad 9
    (0x62, 0x52),   // Keypad 0
    (0x63, 0x53),   // Keypad .
    (0x64, 0x56),   // Non-US \
    (0x65, 0xe05d), // Application
    (0x66, 0xe05e), // Power
    (0x67, 0x59),   // Keypad =
    (0x68, 0x64),   // F13
    (0x69, 0x65),   // F14
    (0x6a, 0x66),   // F15
    (0x6b, 0x67),   // F16
    (0x6c, 0x68),   // F17
    (0x6d, 0x69),   // F18
    (0x6e, 0x6a),   // F19
    (0x6f, 0x6b),   // F20
    (0x70, 0x6c),   // F21
    (0x71, 0x6d),   // F22
    (0x72, 0x6e),   // F23
    (0x73, 0x76),   // F24
    (0x7f, 0xe020), // Mute
    (0x80, 0xe030), // Volume Up
    (0x81, 0xe02e), // Volume Down
    (0x85, 0x7e),   // Keypad , (Brazilian)
    (0x87, 0x73),   // International 1: Ro
    (0x88, 0x70),   // International 2: Katakana/Hiragana
    (0x89, 0x7d),   // International 3: Yen
    (0x8a, 0x79),   // International 4: Henkan
    (0x8b, 0x7b),   // International 5: Muhenkan
    (0x8c, 0x5c),   // International 6
    (0x90, 0xf2),   // LANG1: Hangul/English
    (0x91, 0xf1),   // LANG2: Hanja
    (0x92, 0x78),   // LANG3: Katakana
    (0x93, 0x77),   // LANG4: Hiragana
    (0x94, 0x76),   // LANG5: Zenkaku/Hankaku
    (0xe0, 0x1d),   // Left Control
    (0xe1, 0x2a),   // Left Shift
    (0xe2, 0x38),   // Left Alt
    (0xe3, 0xe05b), // Left GUI
    (0xe4, 0xe01d), // Right Control
    (0xe5, 0x36),   // Right Shift
    (0xe6, 0xe038), // Right Alt
    (0xe7, 0xe05c), // Right GUI
];

/// The Pause key's usage.
const PAUSE_USAGE: u8 = 0x48;

/// The scan code Windows reports with the Pause key: the last byte of its
/// sequence.
const PAUSE_SCAN_CODE: ScanCode = ScanCode(0x45);

/// The Consumer page (0x0C) usages QDP clients send, with their virtual keys
/// and the scan codes keyboards with media keys report for them.
const CONSUMER_PAGE: [(u16, VIRTUAL_KEY, u16); 7] = [
    (0x00b5, VK_MEDIA_NEXT_TRACK, 0xe019), // Scan Next Track
    (0x00b6, VK_MEDIA_PREV_TRACK, 0xe010), // Scan Previous Track
    (0x00b7, VK_MEDIA_STOP, 0xe024),       // Stop
    (0x00cd, VK_MEDIA_PLAY_PAUSE, 0xe022), // Play/Pause
    (0x00e2, VK_VOLUME_MUTE, 0xe020),      // Mute
    (0x00e9, VK_VOLUME_UP, 0xe030),        // Volume Increment
    (0x00ea, VK_VOLUME_DOWN, 0xe02e),      // Volume Decrement
];

/// Returns the key of a Keyboard/Keypad page usage, if it has one.
pub(super) fn keyboard_key(usage: u8) -> Option<WindowsKey> {
    if usage == PAUSE_USAGE {
        return Some(WindowsKey::Virtual(VK_PAUSE, PAUSE_SCAN_CODE));
    }
    KEYBOARD_PAGE
        .iter()
        .find(|(key, _)| *key == usage)
        .map(|&(_, code)| WindowsKey::Scan(ScanCode(code)))
}

/// Returns the key of a Consumer page usage, if it has one.
pub(super) fn consumer_key(usage: u16) -> Option<WindowsKey> {
    CONSUMER_PAGE
        .iter()
        .find(|(consumer, _, _)| *consumer == usage)
        .map(|&(_, key, code)| WindowsKey::Virtual(key, ScanCode(code)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Usages map to the scan codes of Microsoft's translation table, and
    /// each usage appears once.
    #[test]
    fn maps_usages() {
        assert_eq!(keyboard_key(0x04), Some(WindowsKey::Scan(ScanCode(0x1e))));
        assert_eq!(keyboard_key(0x28), Some(WindowsKey::Scan(ScanCode(0x1c))));
        assert_eq!(keyboard_key(0x52), Some(WindowsKey::Scan(ScanCode(0xe048))));
        assert_eq!(
            keyboard_key(0x48),
            Some(WindowsKey::Virtual(VK_PAUSE, ScanCode(0x45)))
        );
        assert_eq!(
            consumer_key(0x00e9),
            Some(WindowsKey::Virtual(VK_VOLUME_UP, ScanCode(0xe030)))
        );
        assert_eq!(keyboard_key(0x00), None);
        assert_eq!(consumer_key(0x0001), None);
        let mut usages: Vec<u8> = KEYBOARD_PAGE.iter().map(|&(usage, _)| usage).collect();
        let count = usages.len();
        usages.dedup();
        assert_eq!(usages.len(), count);
        assert!(!usages.contains(&PAUSE_USAGE));
    }

    /// Extended keys keep their prefix apart from their make code.
    #[test]
    fn splits_extended_codes() {
        assert!(ScanCode(0xe048).is_extended());
        assert_eq!(ScanCode(0xe048).code(), 0x48);
        assert!(!ScanCode(0x48).is_extended());
        assert!(!ScanCode(0xf2).is_extended());
        assert_eq!(ScanCode(0xf2).code(), 0xf2);
    }
}
