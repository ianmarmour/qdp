//! Report layouts of the standard input devices (Section 8.4).
//!
//! Each report states its device's complete current state. Relative motion, wheel
//! and pan fields are running totals that wrap around modulo 2^32. Receivers ignore
//! bytes after the fields defined here, except in text reports, whose data runs to
//! the end.

use crate::wire::{Reader, WireError, Writer};

/// The number of bytes in the bitmap of held keyboard keys.
const KEY_BITMAP_LENGTH: usize = 32;

/// The number of consumer key slots in a keyboard report.
const CONSUMER_KEY_SLOTS: usize = 4;

/// The largest number of contacts in a touchscreen or touchpad report.
pub const MAX_CONTACTS: usize = 16;

/// The number of bytes in a touch contact defined by this version.
const TOUCH_CONTACT_SIZE: u8 = 12;

/// The number of bytes in a touchpad contact defined by this version.
const TOUCHPAD_CONTACT_SIZE: u8 = 6;

/// The largest text report, in bytes.
pub const MAX_TEXT_REPORT_LENGTH: usize = 256;

/// The highest keyboard usage that is an error code rather than a key.
const LAST_KEYBOARD_ERROR_USAGE: u8 = 0x03;

/// A held-key report of a `KEYBOARD` device (Section 8.4.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct KeyboardReport {
    /// Held keys of the Keyboard/Keypad page: usage u is bit `u mod 8` of byte
    /// `u / 8`.
    pub keys: [u8; KEY_BITMAP_LENGTH],
    /// Up to four held keys of the Consumer page; unused slots are 0.
    pub consumer_keys: [u16; CONSUMER_KEY_SLOTS],
}

impl KeyboardReport {
    /// Returns whether the key with Keyboard/Keypad page `usage` is held.
    pub const fn is_held(&self, usage: u8) -> bool {
        self.keys[usage as usize / 8] & (1 << (usage % 8)) != 0
    }

    /// Marks the key with Keyboard/Keypad page `usage` as held or released.
    /// Usages 0x00 to 0x03 are error codes and are never held.
    pub fn set_held(&mut self, usage: u8, held: bool) {
        if usage <= LAST_KEYBOARD_ERROR_USAGE {
            return;
        }
        let mask = 1 << (usage % 8);
        let byte = &mut self.keys[usage as usize / 8];
        if held {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }

    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_bytes(&self.keys);
        for key in self.consumer_keys {
            writer.write_u16(key);
        }
        writer.into_bytes()
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(data);
        let mut report = Self::default();
        report
            .keys
            .copy_from_slice(reader.read_bytes(KEY_BITMAP_LENGTH)?);
        for slot in &mut report.consumer_keys {
            *slot = reader.read_u16()?;
        }
        Ok(report)
    }
}

/// A mask of keyboard LEDs (Section 8.4.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct KeyboardLeds(pub u8);

impl KeyboardLeds {
    /// Num Lock.
    pub const NUM_LOCK: Self = Self(0x01);
    /// Caps Lock.
    pub const CAPS_LOCK: Self = Self(0x02);
    /// Scroll Lock.
    pub const SCROLL_LOCK: Self = Self(0x04);
    /// Compose.
    pub const COMPOSE: Self = Self(0x08);
    /// Kana.
    pub const KANA: Self = Self(0x10);

    /// Returns whether every LED in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The output report of a `KEYBOARD` device (Section 8.4.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct KeyboardOutput {
    /// The lit LEDs.
    pub leds: KeyboardLeds,
}

impl KeyboardOutput {
    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        vec![self.leds.0]
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        Ok(Self {
            leds: KeyboardLeds(Reader::new(data).read_u8()?),
        })
    }
}

/// A mask of pointer and mouse buttons (Sections 8.4.2 and 8.4.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PointerButtons(pub u8);

impl PointerButtons {
    /// The left button.
    pub const LEFT: Self = Self(0x01);
    /// The right button.
    pub const RIGHT: Self = Self(0x02);
    /// The middle button.
    pub const MIDDLE: Self = Self(0x04);
    /// The back button.
    pub const BACK: Self = Self(0x08);
    /// The forward button.
    pub const FORWARD: Self = Self(0x10);

    /// Returns whether every button in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A report of a `POINTER` device: an absolute position (Section 8.4.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PointerReport {
    /// The held buttons.
    pub buttons: PointerButtons,
    /// Horizontal desktop position, in 1/256 pixel.
    pub x: i32,
    /// Vertical desktop position, in 1/256 pixel.
    pub y: i32,
    /// Running total of vertical scrolling, in 1/120 detent; increases away from
    /// the user.
    pub wheel_total: i32,
    /// Running total of horizontal scrolling, in 1/120 detent; increases to the
    /// right.
    pub pan_total: i32,
}

impl PointerReport {
    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_u8(self.buttons.0);
        writer.write_i32(self.x);
        writer.write_i32(self.y);
        writer.write_i32(self.wheel_total);
        writer.write_i32(self.pan_total);
        writer.into_bytes()
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(data);
        Ok(Self {
            buttons: PointerButtons(reader.read_u8()?),
            x: reader.read_i32()?,
            y: reader.read_i32()?,
            wheel_total: reader.read_i32()?,
            pan_total: reader.read_i32()?,
        })
    }
}

/// A report of a `MOUSE` device: relative motion (Section 8.4.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct MouseReport {
    /// The held buttons.
    pub buttons: PointerButtons,
    /// Running total of horizontal motion, in 1/256 device count.
    pub motion_x_total: i32,
    /// Running total of vertical motion, in 1/256 device count.
    pub motion_y_total: i32,
    /// Running total of vertical scrolling, as for the pointer.
    pub wheel_total: i32,
    /// Running total of horizontal scrolling, as for the pointer.
    pub pan_total: i32,
}

impl MouseReport {
    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_u8(self.buttons.0);
        writer.write_i32(self.motion_x_total);
        writer.write_i32(self.motion_y_total);
        writer.write_i32(self.wheel_total);
        writer.write_i32(self.pan_total);
        writer.into_bytes()
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(data);
        Ok(Self {
            buttons: PointerButtons(reader.read_u8()?),
            motion_x_total: reader.read_i32()?,
            motion_y_total: reader.read_i32()?,
            wheel_total: reader.read_i32()?,
            pan_total: reader.read_i32()?,
        })
    }
}

/// Reads a contact list and returns a reader over each contact, checking the
/// Contact Count against [`MAX_CONTACTS`] and the Contact Size against the
/// `known_size` of the contacts this version defines.
fn read_contacts<'a>(
    reader: &mut Reader<'a>,
    known_size: u8,
) -> Result<Vec<Reader<'a>>, WireError> {
    let count = usize::from(reader.read_u8()?);
    if count > MAX_CONTACTS {
        return Err(WireError::LimitExceeded {
            field: "Contact Count",
            limit: MAX_CONTACTS as u64,
        });
    }
    let size = reader.read_u8()?;
    if size < known_size {
        return Err(WireError::UnexpectedEnd);
    }
    (0..count)
        .map(|_| reader.read_bytes(usize::from(size)).map(Reader::new))
        .collect()
}

/// Writes a contact list header for `count` contacts of `size` bytes.
fn write_contacts_header(writer: &mut Writer, count: usize, size: u8) {
    // Lists longer than MAX_CONTACTS are truncated by the callers.
    writer.write_u8(count.min(MAX_CONTACTS) as u8);
    writer.write_u8(size);
}

/// The flags of a touchscreen contact (Section 8.4.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TouchContactFlags(pub u8);

impl TouchContactFlags {
    /// The contact ends without effect, for example after palm rejection.
    pub const CANCELLED: Self = Self(0x01);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// One finger touching a touchscreen (Section 8.4.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TouchContact {
    /// Identifies the finger while it touches.
    pub id: u8,
    /// Cancellation flag.
    pub flags: TouchContactFlags,
    /// Horizontal desktop position, in 1/256 pixel.
    pub x: i32,
    /// Vertical desktop position, in 1/256 pixel.
    pub y: i32,
    /// Pressure from 0 to 65535, meaningful with the pressure feature.
    pub pressure: u16,
}

/// A report of a `TOUCHSCREEN` device: every contact currently touching
/// (Section 8.4.4).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct TouchscreenReport {
    /// The contacts, at most 16. Only the first 16 are sent.
    pub contacts: Vec<TouchContact>,
}

impl TouchscreenReport {
    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        write_contacts_header(&mut writer, self.contacts.len(), TOUCH_CONTACT_SIZE);
        for contact in self.contacts.iter().take(MAX_CONTACTS) {
            writer.write_u8(contact.id);
            writer.write_u8(contact.flags.0);
            writer.write_i32(contact.x);
            writer.write_i32(contact.y);
            writer.write_u16(contact.pressure);
        }
        writer.into_bytes()
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let contacts = read_contacts(&mut Reader::new(data), TOUCH_CONTACT_SIZE)?
            .into_iter()
            .map(|mut contact| {
                Ok(TouchContact {
                    id: contact.read_u8()?,
                    flags: TouchContactFlags(contact.read_u8()?),
                    x: contact.read_i32()?,
                    y: contact.read_i32()?,
                    pressure: contact.read_u16()?,
                })
            })
            .collect::<Result<_, WireError>>()?;
        Ok(Self { contacts })
    }
}

/// The flags of a pen report (Section 8.4.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PenFlags(pub u8);

impl PenFlags {
    /// The pen is hovering or touching.
    pub const IN_RANGE: Self = Self(0x01);
    /// The writing tip is touching.
    pub const TIP_SWITCH: Self = Self(0x02);
    /// The barrel button is pressed.
    pub const BARREL_SWITCH: Self = Self(0x04);
    /// The eraser end is touching.
    pub const ERASER: Self = Self(0x08);
    /// The eraser end is towards the surface.
    pub const INVERT: Self = Self(0x10);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A report of a `PEN` device (Section 8.4.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PenReport {
    /// Range, tip, barrel, eraser and invert flags.
    pub flags: PenFlags,
    /// Horizontal desktop position, in 1/256 pixel.
    pub x: i32,
    /// Vertical desktop position, in 1/256 pixel.
    pub y: i32,
    /// Pressure from 0 to 65535.
    pub pressure: u16,
    /// Tilt in 1/100 degree, from −9000 to 9000.
    pub tilt_x: i16,
    /// Tilt in 1/100 degree, from −9000 to 9000.
    pub tilt_y: i16,
    /// Rotation in 1/100 degree, from 0 to 35999.
    pub twist: u16,
}

impl PenReport {
    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_u8(self.flags.0);
        writer.write_i32(self.x);
        writer.write_i32(self.y);
        writer.write_u16(self.pressure);
        writer.write_i16(self.tilt_x);
        writer.write_i16(self.tilt_y);
        writer.write_u16(self.twist);
        writer.into_bytes()
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(data);
        Ok(Self {
            flags: PenFlags(reader.read_u8()?),
            x: reader.read_i32()?,
            y: reader.read_i32()?,
            pressure: reader.read_u16()?,
            tilt_x: reader.read_i16()?,
            tilt_y: reader.read_i16()?,
            twist: reader.read_u16()?,
        })
    }
}

/// A mask of touchpad buttons (Section 8.4.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TouchpadButtons(pub u8);

impl TouchpadButtons {
    /// The primary button.
    pub const PRIMARY: Self = Self(0x01);
    /// The secondary button.
    pub const SECONDARY: Self = Self(0x02);

    /// Returns whether every button in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The flags of a touchpad contact (Section 8.4.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TouchpadContactFlags(pub u8);

impl TouchpadContactFlags {
    /// The contact is a finger rather than a palm.
    pub const CONFIDENCE: Self = Self(0x01);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// One finger on a touchpad (Section 8.4.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TouchpadContact {
    /// Identifies the finger while it touches.
    pub id: u8,
    /// Confidence flag.
    pub flags: TouchpadContactFlags,
    /// From 0 at the left edge to 65535 at the right edge.
    pub x: u16,
    /// From 0 at the top edge to 65535 at the bottom edge.
    pub y: u16,
}

/// A report of a `TOUCHPAD` device (Section 8.4.6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct TouchpadReport {
    /// The held buttons.
    pub buttons: TouchpadButtons,
    /// The contacts, at most 16. Only the first 16 are sent.
    pub contacts: Vec<TouchpadContact>,
}

impl TouchpadReport {
    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_u8(self.buttons.0);
        write_contacts_header(&mut writer, self.contacts.len(), TOUCHPAD_CONTACT_SIZE);
        for contact in self.contacts.iter().take(MAX_CONTACTS) {
            writer.write_u8(contact.id);
            writer.write_u8(contact.flags.0);
            writer.write_u16(contact.x);
            writer.write_u16(contact.y);
        }
        writer.into_bytes()
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(data);
        let buttons = TouchpadButtons(reader.read_u8()?);
        let contacts = read_contacts(&mut reader, TOUCHPAD_CONTACT_SIZE)?
            .into_iter()
            .map(|mut contact| {
                Ok(TouchpadContact {
                    id: contact.read_u8()?,
                    flags: TouchpadContactFlags(contact.read_u8()?),
                    x: contact.read_u16()?,
                    y: contact.read_u16()?,
                })
            })
            .collect::<Result<_, WireError>>()?;
        Ok(Self { buttons, contacts })
    }
}

/// A mask of gamepad buttons in the Standard Gamepad layout: bit `1 << n` is
/// button n (Section 8.4.7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct GamepadButtons(pub u32);

impl GamepadButtons {
    /// Returns the mask of Standard Gamepad button `index`, from 0 to 16.
    pub const fn button(index: u32) -> Self {
        Self(1 << index)
    }

    /// Returns whether every button in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A report of a `GAMEPAD` device (Section 8.4.7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct GamepadReport {
    /// The held buttons.
    pub buttons: GamepadButtons,
    /// From 0 released to 65535 fully pressed.
    pub left_trigger: u16,
    /// From 0 released to 65535 fully pressed.
    pub right_trigger: u16,
    /// Negative is left.
    pub left_stick_x: i16,
    /// Negative is up.
    pub left_stick_y: i16,
    /// Negative is left.
    pub right_stick_x: i16,
    /// Negative is up.
    pub right_stick_y: i16,
    /// Angular rate about X, Y and Z, in 1/16 degree per second.
    pub gyro: [i16; 3],
    /// Acceleration along X, Y and Z, in 1/4096 of standard gravity.
    pub accel: [i16; 3],
}

impl GamepadReport {
    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_u32(self.buttons.0);
        writer.write_u16(self.left_trigger);
        writer.write_u16(self.right_trigger);
        writer.write_i16(self.left_stick_x);
        writer.write_i16(self.left_stick_y);
        writer.write_i16(self.right_stick_x);
        writer.write_i16(self.right_stick_y);
        for value in self.gyro.into_iter().chain(self.accel) {
            writer.write_i16(value);
        }
        writer.into_bytes()
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(data);
        let mut report = Self {
            buttons: GamepadButtons(reader.read_u32()?),
            left_trigger: reader.read_u16()?,
            right_trigger: reader.read_u16()?,
            left_stick_x: reader.read_i16()?,
            left_stick_y: reader.read_i16()?,
            right_stick_x: reader.read_i16()?,
            right_stick_y: reader.read_i16()?,
            ..Self::default()
        };
        for value in report.gyro.iter_mut().chain(report.accel.iter_mut()) {
            *value = reader.read_i16()?;
        }
        Ok(report)
    }
}

/// The output report of a `GAMEPAD` device (Section 8.4.7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct GamepadOutput {
    /// From 0 off to 65535 full.
    pub low_frequency_rumble: u16,
    /// From 0 off to 65535 full.
    pub high_frequency_rumble: u16,
    /// From 0 off to 65535 full.
    pub left_trigger_rumble: u16,
    /// From 0 off to 65535 full.
    pub right_trigger_rumble: u16,
    /// The LED's red component.
    pub led_red: u8,
    /// The LED's green component.
    pub led_green: u8,
    /// The LED's blue component.
    pub led_blue: u8,
}

impl GamepadOutput {
    /// Returns whether any rumble motor is running.
    pub const fn is_rumbling(&self) -> bool {
        self.low_frequency_rumble != 0
            || self.high_frequency_rumble != 0
            || self.left_trigger_rumble != 0
            || self.right_trigger_rumble != 0
    }

    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_u16(self.low_frequency_rumble);
        writer.write_u16(self.high_frequency_rumble);
        writer.write_u16(self.left_trigger_rumble);
        writer.write_u16(self.right_trigger_rumble);
        writer.write_u8(self.led_red);
        writer.write_u8(self.led_green);
        writer.write_u8(self.led_blue);
        writer.into_bytes()
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(data);
        Ok(Self {
            low_frequency_rumble: reader.read_u16()?,
            high_frequency_rumble: reader.read_u16()?,
            left_trigger_rumble: reader.read_u16()?,
            right_trigger_rumble: reader.read_u16()?,
            led_red: reader.read_u8()?,
            led_green: reader.read_u8()?,
            led_blue: reader.read_u8()?,
        })
    }
}

/// A report of a `TEXT` device: committed Unicode text (Section 8.4.8).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextReport {
    /// From 1 to 256 bytes of UTF-8.
    pub text: String,
}

impl TextReport {
    /// Splits `text` into reports of at most 256 bytes at character boundaries.
    pub fn split(text: &str) -> Vec<Self> {
        let mut reports = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let mut end = rest.len().min(MAX_TEXT_REPORT_LENGTH);
            while !rest.is_char_boundary(end) {
                end -= 1;
            }
            let (head, tail) = rest.split_at(end);
            reports.push(Self {
                text: head.to_owned(),
            });
            rest = tail;
        }
        reports
    }

    /// Returns the encoded report.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.text.as_bytes().to_vec()
    }

    /// Decodes a report.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        if data.is_empty() {
            return Err(WireError::UnexpectedEnd);
        }
        if data.len() > MAX_TEXT_REPORT_LENGTH {
            return Err(WireError::LimitExceeded {
                field: "Text",
                limit: MAX_TEXT_REPORT_LENGTH as u64,
            });
        }
        let text = std::str::from_utf8(data).map_err(|_| WireError::InvalidUtf8)?;
        Ok(Self {
            text: text.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A keyboard report is 40 bytes and tracks held keys by usage.
    #[test]
    fn keyboard_report() {
        let mut report = KeyboardReport::default();
        report.set_held(0x04, true);
        report.set_held(0xe1, true);
        report.set_held(0x02, true);
        report.consumer_keys[0] = 0x00e9;
        assert!(report.is_held(0x04));
        assert!(report.is_held(0xe1));
        assert!(!report.is_held(0x02));

        let bytes = report.to_bytes();
        assert_eq!(bytes.len(), 40);
        assert_eq!(bytes[0], 0x10);
        assert_eq!(KeyboardReport::decode(&bytes).unwrap(), report);

        report.set_held(0x04, false);
        assert!(!report.is_held(0x04));
    }

    /// Fixed-layout reports round-trip and ignore trailing bytes.
    #[test]
    fn fixed_reports_round_trip() {
        let pointer = PointerReport {
            buttons: PointerButtons::LEFT,
            x: 256 * 100,
            y: -256 * 20,
            wheel_total: 120,
            pan_total: i32::MIN,
        };
        let mut bytes = pointer.to_bytes();
        assert_eq!(bytes.len(), 17);
        bytes.push(0xff);
        assert_eq!(PointerReport::decode(&bytes).unwrap(), pointer);

        let mouse = MouseReport {
            buttons: PointerButtons::RIGHT,
            motion_x_total: i32::MAX,
            motion_y_total: -5,
            wheel_total: 0,
            pan_total: 240,
        };
        assert_eq!(MouseReport::decode(&mouse.to_bytes()).unwrap(), mouse);

        let pen = PenReport {
            flags: PenFlags::IN_RANGE,
            x: 1,
            y: 2,
            pressure: 65535,
            tilt_x: -9000,
            tilt_y: 9000,
            twist: 35999,
        };
        assert_eq!(PenReport::decode(&pen.to_bytes()).unwrap(), pen);

        let gamepad = GamepadReport {
            buttons: GamepadButtons::button(0),
            left_trigger: 40000,
            right_trigger: 0,
            left_stick_x: -32768,
            left_stick_y: 32767,
            right_stick_x: 0,
            right_stick_y: 1,
            gyro: [1, -2, 3],
            accel: [0, 4096, 0],
        };
        let bytes = gamepad.to_bytes();
        assert_eq!(bytes.len(), 28);
        assert_eq!(GamepadReport::decode(&bytes).unwrap(), gamepad);

        let output = GamepadOutput {
            low_frequency_rumble: 1000,
            led_blue: 255,
            ..GamepadOutput::default()
        };
        assert!(output.is_rumbling());
        assert_eq!(GamepadOutput::decode(&output.to_bytes()).unwrap(), output);

        let leds = KeyboardOutput {
            leds: KeyboardLeds::CAPS_LOCK,
        };
        assert_eq!(KeyboardOutput::decode(&leds.to_bytes()).unwrap(), leds);
    }

    /// A short fixed-layout report is an error.
    #[test]
    fn short_report() {
        assert_eq!(
            PointerReport::decode(&[0x01, 0x00]),
            Err(WireError::UnexpectedEnd)
        );
    }

    /// Contact lists round-trip, and contacts larger than this version's are read
    /// by their Contact Size.
    #[test]
    fn contact_reports() {
        let touch = TouchscreenReport {
            contacts: vec![
                TouchContact {
                    id: 1,
                    flags: TouchContactFlags::default(),
                    x: 100,
                    y: 200,
                    pressure: 300,
                },
                TouchContact {
                    id: 2,
                    flags: TouchContactFlags::CANCELLED,
                    x: -1,
                    y: -2,
                    pressure: 0,
                },
            ],
        };
        assert_eq!(TouchscreenReport::decode(&touch.to_bytes()).unwrap(), touch);

        let touchpad = TouchpadReport {
            buttons: TouchpadButtons::PRIMARY,
            contacts: vec![TouchpadContact {
                id: 0,
                flags: TouchpadContactFlags::CONFIDENCE,
                x: 65535,
                y: 0,
            }],
        };
        assert_eq!(
            TouchpadReport::decode(&touchpad.to_bytes()).unwrap(),
            touchpad
        );

        // One touchpad contact of 8 bytes: the last two are a future field.
        let extended = [
            0x00, 0x01, 0x08, 0x05, 0x01, 0x00, 0x10, 0x00, 0x20, 0xaa, 0xbb,
        ];
        let report = TouchpadReport::decode(&extended).unwrap();
        assert_eq!(report.contacts[0].id, 5);
        assert_eq!(report.contacts[0].y, 0x20);
    }

    /// Contact counts above 16 and contact sizes below this version's are errors.
    #[test]
    fn invalid_contact_lists() {
        assert!(matches!(
            TouchscreenReport::decode(&[17, 12]),
            Err(WireError::LimitExceeded { .. })
        ));
        assert_eq!(
            TouchscreenReport::decode(&[1, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            Err(WireError::UnexpectedEnd)
        );
    }

    /// Text is split at character boundaries into reports of at most 256 bytes.
    #[test]
    fn text_reports() {
        let text = "é".repeat(200);
        let reports = TextReport::split(&text);
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].text.len(), 256);
        assert_eq!(reports[1].text.len(), 144);
        assert_eq!(
            reports
                .iter()
                .map(|report| report.text.as_str())
                .collect::<String>(),
            text
        );
        for report in &reports {
            assert_eq!(&TextReport::decode(&report.to_bytes()).unwrap(), report);
        }
        assert!(TextReport::split("").is_empty());
    }

    /// Empty, oversized and invalid text reports are errors.
    #[test]
    fn invalid_text_reports() {
        assert_eq!(TextReport::decode(&[]), Err(WireError::UnexpectedEnd));
        assert!(matches!(
            TextReport::decode(&[b'a'; 257]),
            Err(WireError::LimitExceeded { .. })
        ));
        assert_eq!(TextReport::decode(&[0xff]), Err(WireError::InvalidUtf8));
    }
}
