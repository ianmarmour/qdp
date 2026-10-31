//! Capabilities exchanged in the hello messages (Section 4.6).

use crate::registry::CapabilityId;
use crate::wire::{Reader, VarInt, WireError, Writer};

/// The largest number of capabilities a hello message may carry (Section 4.3).
pub const MAX_CAPABILITIES: u64 = 64;

/// The largest Capability Length a receiver accepts (Section 4.6.1).
pub const MAX_CAPABILITY_LENGTH: usize = 4096;

/// The Max Level value that means the endpoint supports every AV1 level
/// (Section 4.6.2).
pub const NO_LEVEL_LIMIT: u8 = 31;

/// A video codec (Section 12.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VideoCodec(pub VarInt);

impl VideoCodec {
    /// AV1.
    pub const AV1: Self = Self(VarInt::from_u32(0x01));
}

/// A mask of AV1 profiles (Section 4.6.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Av1Profiles(pub u8);

impl Av1Profiles {
    /// Profile 0 (Main).
    pub const MAIN: Self = Self(0x01);
    /// Profile 1 (High).
    pub const HIGH: Self = Self(0x02);

    /// Returns whether every profile in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The flags of a `VIDEO_CODEC` capability (Section 4.6.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct VideoCodecFlags(pub u8);

impl VideoCodecFlags {
    /// Profile 0 is hardware-accelerated.
    pub const HARDWARE_PROFILE_0: Self = Self(0x01);
    /// Profile 1 is hardware-accelerated.
    pub const HARDWARE_PROFILE_1: Self = Self(0x02);
    /// The endpoint can encode or decode lossless frames.
    pub const LOSSLESS: Self = Self(0x04);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A `VIDEO_CODEC` capability: what the host can encode or the client can decode
/// (Section 4.6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoCodecCapability {
    /// The codec. Entries with an unknown codec are ignored.
    pub codec: VideoCodec,
    /// The AV1 profiles supported.
    pub profiles: Av1Profiles,
    /// The largest bit depth supported, 8 or 10.
    pub max_bit_depth: u8,
    /// The largest frame width, in pixels.
    pub max_width: VarInt,
    /// The largest frame height, in pixels.
    pub max_height: VarInt,
    /// The luma samples per second sustained across all video streams, or 0 if not
    /// stated.
    pub max_pixel_rate: VarInt,
    /// Hardware acceleration and lossless support.
    pub flags: VideoCodecFlags,
    /// The highest AV1 level (`seq_level_idx`) supported, or [`NO_LEVEL_LIMIT`].
    pub max_level: u8,
}

impl VideoCodecCapability {
    /// Appends the capability value to `writer`.
    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.codec.0);
        writer.write_u8(self.profiles.0);
        writer.write_u8(self.max_bit_depth);
        writer.write_varint(self.max_width);
        writer.write_varint(self.max_height);
        writer.write_varint(self.max_pixel_rate);
        writer.write_u8(self.flags.0);
        writer.write_u8(self.max_level);
    }

    /// Reads the capability value from `reader`.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            codec: VideoCodec(reader.read_varint()?),
            profiles: Av1Profiles(reader.read_u8()?),
            max_bit_depth: reader.read_u8()?,
            max_width: reader.read_varint()?,
            max_height: reader.read_varint()?,
            max_pixel_rate: reader.read_varint()?,
            flags: VideoCodecFlags(reader.read_u8()?),
            max_level: reader.read_u8()?,
        })
    }
}

/// An audio codec (Sections 7.5 and 12.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AudioCodec(pub VarInt);

impl AudioCodec {
    /// Opus.
    pub const OPUS: Self = Self(VarInt::from_u32(0x01));
    /// 16-bit linear PCM.
    pub const PCM_L16: Self = Self(VarInt::from_u32(0x02));
    /// 24-bit linear PCM.
    pub const PCM_L24: Self = Self(VarInt::from_u32(0x03));
}

/// A mask of the directions in which an endpoint handles a codec or its clipboard
/// (Sections 4.6.4 and 4.6.6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Directions(pub u8);

impl Directions {
    /// The endpoint can encode and send, or offers its clipboard.
    pub const SEND: Self = Self(0x01);
    /// The endpoint can receive and decode, or accepts the peer's clipboard.
    pub const RECEIVE: Self = Self(0x02);

    /// Returns whether every direction in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// An `AUDIO_CODEC` capability (Section 4.6.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioCodecCapability {
    /// The codec.
    pub codec: AudioCodec,
    /// The largest number of channels, from 1 to 8.
    pub max_channels: VarInt,
    /// Whether the endpoint can send or receive the codec.
    pub directions: Directions,
}

impl AudioCodecCapability {
    /// Appends the capability value to `writer`.
    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.codec.0);
        writer.write_varint(self.max_channels);
        writer.write_u8(self.directions.0);
    }

    /// Reads the capability value from `reader`.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            codec: AudioCodec(reader.read_varint()?),
            max_channels: reader.read_varint()?,
            directions: Directions(reader.read_u8()?),
        })
    }
}

/// A mask of input device classes (Section 4.6.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct InputClasses(pub VarInt);

impl InputClasses {
    /// Keyboards.
    pub const KEYBOARD: Self = Self(VarInt::from_u32(0x01));
    /// Absolute pointers.
    pub const POINTER: Self = Self(VarInt::from_u32(0x02));
    /// Relative mice.
    pub const MOUSE: Self = Self(VarInt::from_u32(0x04));
    /// Touchscreens.
    pub const TOUCHSCREEN: Self = Self(VarInt::from_u32(0x08));
    /// Pens.
    pub const PEN: Self = Self(VarInt::from_u32(0x10));
    /// Touchpads.
    pub const TOUCHPAD: Self = Self(VarInt::from_u32(0x20));
    /// Gamepads.
    pub const GAMEPAD: Self = Self(VarInt::from_u32(0x40));
    /// Text input.
    pub const TEXT: Self = Self(VarInt::from_u32(0x80));
    /// HID passthrough devices.
    pub const PASSTHROUGH: Self = Self(VarInt::from_u32(0x100));

    /// Returns whether every class in `other` is also in `self`.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the classes in both `self` and `other`: those a session can use.
    pub fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Returns the classes in either `self` or `other`.
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// A `CLIPBOARD` capability (Section 4.6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardCapability {
    /// Whether the endpoint offers its clipboard, accepts the peer's, or both.
    pub directions: Directions,
    /// The largest clipboard data, in bytes, the endpoint accepts.
    pub max_size: VarInt,
}

/// A mask of high dynamic range formats (Section 4.6.8).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct HdrFormats(pub u8);

impl HdrFormats {
    /// BT.2020 primaries with the PQ transfer function.
    pub const BT2020_PQ: Self = Self(0x01);
    /// BT.2020 primaries with the HLG transfer function.
    pub const BT2020_HLG: Self = Self(0x02);

    /// Returns whether every format in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A capability (Section 4.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Capability {
    /// `VIDEO_CODEC` (Section 4.6.2).
    VideoCodec(VideoCodecCapability),
    /// `REFINEMENT` (Section 4.6.3).
    Refinement,
    /// `AUDIO_CODEC` (Section 4.6.4).
    AudioCodec(AudioCodecCapability),
    /// `INPUT` (Section 4.6.5).
    Input(InputClasses),
    /// `CLIPBOARD` (Section 4.6.6).
    Clipboard(ClipboardCapability),
    /// `MONITORS`: the number of monitors the endpoint can stream or display
    /// (Section 4.6.7).
    Monitors(VarInt),
    /// `HDR` (Section 4.6.8).
    Hdr(HdrFormats),
    /// A capability this implementation does not know, which the receiver ignores.
    Unknown {
        /// The Capability ID.
        id: CapabilityId,
        /// The Capability Value.
        value: Vec<u8>,
    },
}

impl Capability {
    /// Returns the Capability ID of this capability.
    pub fn id(&self) -> CapabilityId {
        match self {
            Self::VideoCodec(_) => CapabilityId::VIDEO_CODEC,
            Self::Refinement => CapabilityId::REFINEMENT,
            Self::AudioCodec(_) => CapabilityId::AUDIO_CODEC,
            Self::Input(_) => CapabilityId::INPUT,
            Self::Clipboard(_) => CapabilityId::CLIPBOARD,
            Self::Monitors(_) => CapabilityId::MONITORS,
            Self::Hdr(_) => CapabilityId::HDR,
            Self::Unknown { id, .. } => *id,
        }
    }

    /// Appends the capability, with its ID and length, to `writer`.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.id().0);
        writer.write_length_prefixed(|value| match self {
            Self::VideoCodec(capability) => capability.encode(value),
            Self::Refinement => {}
            Self::AudioCodec(capability) => capability.encode(value),
            Self::Input(classes) => value.write_varint(classes.0),
            Self::Clipboard(capability) => {
                value.write_u8(capability.directions.0);
                value.write_varint(capability.max_size);
            }
            Self::Monitors(max_monitors) => value.write_varint(*max_monitors),
            Self::Hdr(formats) => value.write_u8(formats.0),
            Self::Unknown { value: bytes, .. } => value.write_bytes(bytes),
        });
    }

    /// Reads a capability, with its ID and length, from `reader`.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let id = CapabilityId(reader.read_varint()?);
        let bytes = reader.read_octets_at_most("Capability Length", MAX_CAPABILITY_LENGTH)?;
        let mut value = Reader::new(bytes);
        let capability = match id {
            CapabilityId::VIDEO_CODEC => {
                Self::VideoCodec(VideoCodecCapability::decode(&mut value)?)
            }
            CapabilityId::REFINEMENT => Self::Refinement,
            CapabilityId::AUDIO_CODEC => {
                Self::AudioCodec(AudioCodecCapability::decode(&mut value)?)
            }
            CapabilityId::INPUT => Self::Input(InputClasses(value.read_varint()?)),
            CapabilityId::CLIPBOARD => Self::Clipboard(ClipboardCapability {
                directions: Directions(value.read_u8()?),
                max_size: value.read_varint()?,
            }),
            CapabilityId::MONITORS => Self::Monitors(value.read_varint()?),
            CapabilityId::HDR => Self::Hdr(HdrFormats(value.read_u8()?)),
            _ => Self::Unknown {
                id,
                value: bytes.to_vec(),
            },
        };
        Ok(capability)
    }

    /// Appends a Capability Count followed by `capabilities` to `writer`.
    pub(crate) fn encode_list(capabilities: &[Self], writer: &mut Writer) {
        writer.write_count(capabilities.len());
        for capability in capabilities {
            capability.encode(writer);
        }
    }

    /// Reads a Capability Count followed by that many capabilities from `reader`.
    pub(crate) fn decode_list(reader: &mut Reader<'_>) -> Result<Vec<Self>, WireError> {
        let count = reader.read_count("Capability Count", MAX_CAPABILITIES)?;
        (0..count).map(|_| Self::decode(reader)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns one capability of every known kind.
    fn every_capability() -> Vec<Capability> {
        vec![
            Capability::VideoCodec(VideoCodecCapability {
                codec: VideoCodec::AV1,
                profiles: Av1Profiles(Av1Profiles::MAIN.0 | Av1Profiles::HIGH.0),
                max_bit_depth: 10,
                max_width: VarInt::from_u32(7680),
                max_height: VarInt::from_u32(4320),
                max_pixel_rate: VarInt::from_u32(1_990_656_000),
                flags: VideoCodecFlags::HARDWARE_PROFILE_0,
                max_level: NO_LEVEL_LIMIT,
            }),
            Capability::Refinement,
            Capability::AudioCodec(AudioCodecCapability {
                codec: AudioCodec::OPUS,
                max_channels: VarInt::from_u32(8),
                directions: Directions(Directions::SEND.0 | Directions::RECEIVE.0),
            }),
            Capability::Input(InputClasses::KEYBOARD.union(InputClasses::PASSTHROUGH)),
            Capability::Clipboard(ClipboardCapability {
                directions: Directions::RECEIVE,
                max_size: VarInt::from_u32(64 << 20),
            }),
            Capability::Monitors(VarInt::from_u32(4)),
            Capability::Hdr(HdrFormats::BT2020_PQ),
            Capability::Unknown {
                id: CapabilityId(VarInt::from_u32(0x5f)),
                value: vec![1, 2, 3],
            },
        ]
    }

    /// Every capability round-trips through a list.
    #[test]
    fn list_round_trip() {
        let capabilities = every_capability();
        let mut writer = Writer::new();
        Capability::encode_list(&capabilities, &mut writer);
        let mut reader = Reader::new(writer.as_bytes());
        assert_eq!(Capability::decode_list(&mut reader).unwrap(), capabilities);
        assert!(reader.is_empty());
    }

    /// `REFINEMENT` has an empty value.
    #[test]
    fn refinement_encoding() {
        let mut writer = Writer::new();
        Capability::Refinement.encode(&mut writer);
        assert_eq!(writer.as_bytes(), [0x02, 0x00]);
    }

    /// Bytes after the known fields of a capability value are ignored.
    #[test]
    fn trailing_value_bytes_ignored() {
        let mut reader = Reader::new(&[0x06, 0x03, 0x02, 0xaa, 0xbb, 0x07]);
        assert_eq!(
            Capability::decode(&mut reader).unwrap(),
            Capability::Monitors(VarInt::from_u32(2))
        );
        assert_eq!(reader.read_u8().unwrap(), 0x07);
    }

    /// A capability value shorter than its fields is an error.
    #[test]
    fn short_value() {
        let mut reader = Reader::new(&[0x05, 0x01, 0x03]);
        assert_eq!(
            Capability::decode(&mut reader),
            Err(WireError::UnexpectedEnd)
        );
    }

    /// A Capability Length above 4096 is an error.
    #[test]
    fn oversized_capability() {
        let mut writer = Writer::new();
        writer.write_varint(VarInt::from_u32(0x40));
        writer.write_octets(&[0; MAX_CAPABILITY_LENGTH + 1]);
        let mut reader = Reader::new(writer.as_bytes());
        assert!(matches!(
            Capability::decode(&mut reader),
            Err(WireError::LimitExceeded { .. })
        ));
    }

    /// More than 64 capabilities is an error.
    #[test]
    fn too_many_capabilities() {
        let capabilities = vec![Capability::Refinement; MAX_CAPABILITIES as usize + 1];
        let mut writer = Writer::new();
        Capability::encode_list(&capabilities, &mut writer);
        let mut reader = Reader::new(writer.as_bytes());
        assert!(matches!(
            Capability::decode_list(&mut reader),
            Err(WireError::LimitExceeded { .. })
        ));
    }

    /// Input classes combine as masks.
    #[test]
    fn input_classes() {
        let client = InputClasses::KEYBOARD.union(InputClasses::GAMEPAD);
        let host = InputClasses::KEYBOARD.union(InputClasses::MOUSE);
        assert_eq!(client.intersection(host), InputClasses::KEYBOARD);
        assert!(client.contains(InputClasses::GAMEPAD));
        assert!(!host.contains(InputClasses::GAMEPAD));
    }
}
