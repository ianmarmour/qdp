//! Code points of the QDP registries (Section 12).
//!
//! Each type and identifier space is a newtype over [`VarInt`], so that a message type
//! cannot be confused with a datagram type or an error code. Values outside the
//! registry are representable, because receivers must ignore types they do not know
//! (Section 1.6).

use std::fmt;

use crate::wire::VarInt;

/// The first value reserved for greasing (Section 1.6).
const GREASE_BASE: u64 = 0x21;

/// The distance between consecutive values reserved for greasing (Section 1.6).
const GREASE_STRIDE: u64 = 0x1f;

/// Returns whether `value` is reserved for greasing: a value of the form
/// `0x1f × N + 0x21` for some integer N ≥ 0.
///
/// Reserved values apply only to type and identifier spaces: control message types,
/// datagram types, unidirectional stream types, capability IDs and record types.
pub const fn is_grease(value: VarInt) -> bool {
    let value = value.into_inner();
    value >= GREASE_BASE && (value - GREASE_BASE).is_multiple_of(GREASE_STRIDE)
}

/// Returns the `index`-th value reserved for greasing, or `None` if it would exceed
/// [`VarInt::MAX`].
pub const fn grease_value(index: u64) -> Option<VarInt> {
    let Some(offset) = index.checked_mul(GREASE_STRIDE) else {
        return None;
    };
    let Some(value) = offset.checked_add(GREASE_BASE) else {
        return None;
    };
    match VarInt::new(value) {
        Ok(value) => Some(value),
        Err(_) => None,
    }
}

/// Writes a registry code as its name and value, or as a bare hexadecimal value if it
/// has no name.
fn fmt_code(formatter: &mut fmt::Formatter<'_>, name: Option<&str>, value: VarInt) -> fmt::Result {
    match name {
        Some(name) => write!(formatter, "{name} ({:#04x})", value.into_inner()),
        None => write!(formatter, "{:#04x}", value.into_inner()),
    }
}

/// A control message type (Section 12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MessageType(pub VarInt);

impl MessageType {
    /// `CLIENT_HELLO`, client to host (Section 4.3.1).
    pub const CLIENT_HELLO: Self = Self(VarInt::from_u32(0x01));
    /// `HOST_HELLO`, host to client (Section 4.3.2).
    pub const HOST_HELLO: Self = Self(VarInt::from_u32(0x02));
    /// `AUTH_START`, client to host (Section 4.4.1).
    pub const AUTH_START: Self = Self(VarInt::from_u32(0x03));
    /// `AUTH_CONTINUE`, both directions (Section 4.4.1).
    pub const AUTH_CONTINUE: Self = Self(VarInt::from_u32(0x04));
    /// `AUTH_RESULT`, host to client (Section 4.4.1).
    pub const AUTH_RESULT: Self = Self(VarInt::from_u32(0x05));
    /// `CLIENT_READY`, client to host (Section 4.5).
    pub const CLIENT_READY: Self = Self(VarInt::from_u32(0x06));
    /// `SESSION_START`, host to client (Section 4.5).
    pub const SESSION_START: Self = Self(VarInt::from_u32(0x07));
    /// `RESUMPTION_TOKEN`, host to client (Section 4.4.7).
    pub const RESUMPTION_TOKEN: Self = Self(VarInt::from_u32(0x08));
    /// `DISPLAY_REQUEST`, client to host (Section 6.2).
    pub const DISPLAY_REQUEST: Self = Self(VarInt::from_u32(0x10));
    /// `DISPLAY_CONFIG`, host to client (Section 6.3).
    pub const DISPLAY_CONFIG: Self = Self(VarInt::from_u32(0x11));
    /// `KEYFRAME_REQUEST`, client to host (Section 6.10).
    pub const KEYFRAME_REQUEST: Self = Self(VarInt::from_u32(0x12));
    /// `AUDIO_REQUEST`, client to host (Section 7.2).
    pub const AUDIO_REQUEST: Self = Self(VarInt::from_u32(0x18));
    /// `AUDIO_CONFIG`, host to client (Section 7.3).
    pub const AUDIO_CONFIG: Self = Self(VarInt::from_u32(0x19));
    /// `KEYBOARD_INFO`, client to host (Section 8.2.3).
    pub const KEYBOARD_INFO: Self = Self(VarInt::from_u32(0x30));
    /// `DEVICE_ATTACH`, client to host (Section 8.2.1).
    pub const DEVICE_ATTACH: Self = Self(VarInt::from_u32(0x31));
    /// `DEVICE_DETACH`, client to host (Section 8.2.2).
    pub const DEVICE_DETACH: Self = Self(VarInt::from_u32(0x32));
    /// `POINTER_MODE`, host to client (Section 8.9).
    pub const POINTER_MODE: Self = Self(VarInt::from_u32(0x33));
    /// `HID_FEATURE_REQUEST`, host to client (Section 8.8.3).
    pub const HID_FEATURE_REQUEST: Self = Self(VarInt::from_u32(0x35));
    /// `HID_FEATURE_RESPONSE`, client to host (Section 8.8.3).
    pub const HID_FEATURE_RESPONSE: Self = Self(VarInt::from_u32(0x36));
    /// `DEVICE_STATUS`, host to client (Section 8.2.4).
    pub const DEVICE_STATUS: Self = Self(VarInt::from_u32(0x37));
    /// `CURSOR_SET`, host to client (Section 9.3).
    pub const CURSOR_SET: Self = Self(VarInt::from_u32(0x38));
    /// `CURSOR_WARP`, host to client (Section 9.4).
    pub const CURSOR_WARP: Self = Self(VarInt::from_u32(0x39));
    /// `CLIPBOARD_OFFER`, both directions (Section 10.2).
    pub const CLIPBOARD_OFFER: Self = Self(VarInt::from_u32(0x48));
    /// `CLIPBOARD_REQUEST`, both directions (Section 10.3).
    pub const CLIPBOARD_REQUEST: Self = Self(VarInt::from_u32(0x49));
    /// `CLIPBOARD_REJECT`, both directions (Section 10.3).
    pub const CLIPBOARD_REJECT: Self = Self(VarInt::from_u32(0x4a));

    /// Returns the registry name of this type, or `None` if it is not assigned.
    pub const fn name(self) -> Option<&'static str> {
        let name = match self.0.into_inner() {
            0x01 => "CLIENT_HELLO",
            0x02 => "HOST_HELLO",
            0x03 => "AUTH_START",
            0x04 => "AUTH_CONTINUE",
            0x05 => "AUTH_RESULT",
            0x06 => "CLIENT_READY",
            0x07 => "SESSION_START",
            0x08 => "RESUMPTION_TOKEN",
            0x10 => "DISPLAY_REQUEST",
            0x11 => "DISPLAY_CONFIG",
            0x12 => "KEYFRAME_REQUEST",
            0x18 => "AUDIO_REQUEST",
            0x19 => "AUDIO_CONFIG",
            0x30 => "KEYBOARD_INFO",
            0x31 => "DEVICE_ATTACH",
            0x32 => "DEVICE_DETACH",
            0x33 => "POINTER_MODE",
            0x35 => "HID_FEATURE_REQUEST",
            0x36 => "HID_FEATURE_RESPONSE",
            0x37 => "DEVICE_STATUS",
            0x38 => "CURSOR_SET",
            0x39 => "CURSOR_WARP",
            0x48 => "CLIPBOARD_OFFER",
            0x49 => "CLIPBOARD_REQUEST",
            0x4a => "CLIPBOARD_REJECT",
            _ => return None,
        };
        Some(name)
    }

    /// Returns whether this type is reserved for greasing.
    pub const fn is_grease(self) -> bool {
        is_grease(self.0)
    }
}

impl fmt::Display for MessageType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_code(formatter, self.name(), self.0)
    }
}

/// A datagram type (Section 12.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DatagramType(pub VarInt);

impl DatagramType {
    /// `VIDEO_FRAGMENT`, host to client (Section 6.6).
    pub const VIDEO_FRAGMENT: Self = Self(VarInt::from_u32(0x01));
    /// `VIDEO_FEEDBACK`, client to host (Section 6.7).
    pub const VIDEO_FEEDBACK: Self = Self(VarInt::from_u32(0x02));
    /// `AUDIO_PACKET`, both directions (Section 7.4).
    pub const AUDIO_PACKET: Self = Self(VarInt::from_u32(0x03));
    /// `INPUT`, client to host (Section 8.3).
    pub const INPUT: Self = Self(VarInt::from_u32(0x04));
    /// `INPUT_ACK`, host to client (Section 8.5).
    pub const INPUT_ACK: Self = Self(VarInt::from_u32(0x05));
    /// `DEVICE_OUTPUT`, host to client (Section 8.8.1).
    pub const DEVICE_OUTPUT: Self = Self(VarInt::from_u32(0x06));
    /// `TIME_PING`, both directions (Section 5.2).
    pub const TIME_PING: Self = Self(VarInt::from_u32(0x07));
    /// `TIME_PONG`, both directions (Section 5.2).
    pub const TIME_PONG: Self = Self(VarInt::from_u32(0x08));

    /// Returns the registry name of this type, or `None` if it is not assigned.
    pub const fn name(self) -> Option<&'static str> {
        let name = match self.0.into_inner() {
            0x01 => "VIDEO_FRAGMENT",
            0x02 => "VIDEO_FEEDBACK",
            0x03 => "AUDIO_PACKET",
            0x04 => "INPUT",
            0x05 => "INPUT_ACK",
            0x06 => "DEVICE_OUTPUT",
            0x07 => "TIME_PING",
            0x08 => "TIME_PONG",
            _ => return None,
        };
        Some(name)
    }

    /// Returns whether this type is reserved for greasing.
    pub const fn is_grease(self) -> bool {
        is_grease(self.0)
    }
}

impl fmt::Display for DatagramType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_code(formatter, self.name(), self.0)
    }
}

/// A unidirectional stream type (Section 12.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamType(pub VarInt);

impl StreamType {
    /// `CURSOR_IMAGE`, host to client (Section 9.2).
    pub const CURSOR_IMAGE: Self = Self(VarInt::from_u32(0x01));
    /// `CLIPBOARD_DATA`, both directions (Section 10.4).
    pub const CLIPBOARD_DATA: Self = Self(VarInt::from_u32(0x02));
    /// `HID_OUTPUT`, host to client (Section 8.8.2).
    pub const HID_OUTPUT: Self = Self(VarInt::from_u32(0x03));

    /// Returns the registry name of this type, or `None` if it is not assigned.
    pub const fn name(self) -> Option<&'static str> {
        let name = match self.0.into_inner() {
            0x01 => "CURSOR_IMAGE",
            0x02 => "CLIPBOARD_DATA",
            0x03 => "HID_OUTPUT",
            _ => return None,
        };
        Some(name)
    }

    /// Returns whether this type is reserved for greasing.
    pub const fn is_grease(self) -> bool {
        is_grease(self.0)
    }
}

impl fmt::Display for StreamType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_code(formatter, self.name(), self.0)
    }
}

/// A capability ID (Section 12.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityId(pub VarInt);

impl CapabilityId {
    /// `VIDEO_CODEC`, repeatable (Section 4.6.2).
    pub const VIDEO_CODEC: Self = Self(VarInt::from_u32(0x01));
    /// `REFINEMENT` (Section 4.6.3).
    pub const REFINEMENT: Self = Self(VarInt::from_u32(0x02));
    /// `AUDIO_CODEC`, repeatable (Section 4.6.4).
    pub const AUDIO_CODEC: Self = Self(VarInt::from_u32(0x03));
    /// `INPUT` (Section 4.6.5).
    pub const INPUT: Self = Self(VarInt::from_u32(0x04));
    /// `CLIPBOARD` (Section 4.6.6).
    pub const CLIPBOARD: Self = Self(VarInt::from_u32(0x05));
    /// `MONITORS` (Section 4.6.7).
    pub const MONITORS: Self = Self(VarInt::from_u32(0x06));
    /// `HDR` (Section 4.6.8).
    pub const HDR: Self = Self(VarInt::from_u32(0x07));

    /// Returns the registry name of this capability, or `None` if it is not assigned.
    pub const fn name(self) -> Option<&'static str> {
        let name = match self.0.into_inner() {
            0x01 => "VIDEO_CODEC",
            0x02 => "REFINEMENT",
            0x03 => "AUDIO_CODEC",
            0x04 => "INPUT",
            0x05 => "CLIPBOARD",
            0x06 => "MONITORS",
            0x07 => "HDR",
            _ => return None,
        };
        Some(name)
    }

    /// Returns whether a capability may appear more than once in a hello message.
    pub const fn is_repeatable(self) -> bool {
        matches!(self, Self::VIDEO_CODEC | Self::AUDIO_CODEC)
    }

    /// Returns whether this capability ID is reserved for greasing.
    pub const fn is_grease(self) -> bool {
        is_grease(self.0)
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_code(formatter, self.name(), self.0)
    }
}

/// A `VIDEO_FEEDBACK` record type (Section 12.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordType(pub VarInt);

impl RecordType {
    /// `DECODED` (Section 6.7).
    pub const DECODED: Self = Self(VarInt::from_u32(0x01));
    /// `NACK` (Section 6.7).
    pub const NACK: Self = Self(VarInt::from_u32(0x02));
    /// `LOSS` (Section 6.7).
    pub const LOSS: Self = Self(VarInt::from_u32(0x03));
    /// `FRAME_TIMING` (Section 6.7).
    pub const FRAME_TIMING: Self = Self(VarInt::from_u32(0x04));
    /// `DISPLAY_TIMING` (Section 6.7).
    pub const DISPLAY_TIMING: Self = Self(VarInt::from_u32(0x05));

    /// Returns the registry name of this record type, or `None` if it is not
    /// assigned.
    pub const fn name(self) -> Option<&'static str> {
        let name = match self.0.into_inner() {
            0x01 => "DECODED",
            0x02 => "NACK",
            0x03 => "LOSS",
            0x04 => "FRAME_TIMING",
            0x05 => "DISPLAY_TIMING",
            _ => return None,
        };
        Some(name)
    }

    /// Returns whether this record type is reserved for greasing.
    pub const fn is_grease(self) -> bool {
        is_grease(self.0)
    }
}

impl fmt::Display for RecordType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_code(formatter, self.name(), self.0)
    }
}

/// A QDP error code, sent as a QUIC application error code (Section 12.6).
///
/// Codes up to [`ErrorCode::MAX_CONNECTION_CODE`] close connections and may also
/// reset streams; higher codes are used only on streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ErrorCode(pub VarInt);

impl ErrorCode {
    /// `QDP_NO_ERROR`: normal end of session.
    pub const NO_ERROR: Self = Self(VarInt::from_u32(0x00));
    /// `QDP_INTERNAL_ERROR`: an implementation fault.
    pub const INTERNAL_ERROR: Self = Self(VarInt::from_u32(0x01));
    /// `QDP_PROTOCOL_VIOLATION`: the peer broke a rule of the specification.
    pub const PROTOCOL_VIOLATION: Self = Self(VarInt::from_u32(0x02));
    /// `QDP_AUTH_FAILED`: authentication failed.
    pub const AUTH_FAILED: Self = Self(VarInt::from_u32(0x03));
    /// `QDP_NO_COMMON_CAPABILITY`: no video codec in common.
    pub const NO_COMMON_CAPABILITY: Self = Self(VarInt::from_u32(0x04));
    /// `QDP_TRANSPORT_REQUIREMENTS`: the peer does not support QUIC datagrams.
    pub const TRANSPORT_REQUIREMENTS: Self = Self(VarInt::from_u32(0x05));
    /// `QDP_CLOSED_CRITICAL_STREAM`: the control stream was closed or reset.
    pub const CLOSED_CRITICAL_STREAM: Self = Self(VarInt::from_u32(0x06));
    /// `QDP_STREAM_CREATION_ERROR`: a stream was opened that is not allowed.
    pub const STREAM_CREATION_ERROR: Self = Self(VarInt::from_u32(0x07));
    /// `QDP_MESSAGE_TOO_LARGE`: a control message exceeded its size limit.
    pub const MESSAGE_TOO_LARGE: Self = Self(VarInt::from_u32(0x08));
    /// `QDP_RESOURCE_LIMIT`: the peer exceeded a resource limit.
    pub const RESOURCE_LIMIT: Self = Self(VarInt::from_u32(0x09));
    /// `QDP_HANDSHAKE_TIMEOUT`: the session did not become active in time.
    pub const HANDSHAKE_TIMEOUT: Self = Self(VarInt::from_u32(0x0a));
    /// `QDP_SESSION_REPLACED`: a newer session for the same user replaced this one.
    pub const SESSION_REPLACED: Self = Self(VarInt::from_u32(0x0b));
    /// `QDP_HOST_SHUTDOWN`: the host is shutting down or logging the user out.
    pub const HOST_SHUTDOWN: Self = Self(VarInt::from_u32(0x0c));
    /// `QDP_POLICY`: administrative policy ended the session.
    pub const POLICY: Self = Self(VarInt::from_u32(0x0d));
    /// `QDP_STREAM_TYPE_UNKNOWN`: unknown unidirectional stream type. Stream only.
    pub const STREAM_TYPE_UNKNOWN: Self = Self(VarInt::from_u32(0x10));
    /// `QDP_REQUEST_CANCELLED`: the receiver no longer needs the stream's data.
    /// Stream only.
    pub const REQUEST_CANCELLED: Self = Self(VarInt::from_u32(0x11));

    /// The highest code that may close a connection.
    pub const MAX_CONNECTION_CODE: Self = Self(VarInt::from_u32(0x0f));

    /// Returns the registry name of this code, or `None` if it is not assigned.
    pub const fn name(self) -> Option<&'static str> {
        let name = match self.0.into_inner() {
            0x00 => "QDP_NO_ERROR",
            0x01 => "QDP_INTERNAL_ERROR",
            0x02 => "QDP_PROTOCOL_VIOLATION",
            0x03 => "QDP_AUTH_FAILED",
            0x04 => "QDP_NO_COMMON_CAPABILITY",
            0x05 => "QDP_TRANSPORT_REQUIREMENTS",
            0x06 => "QDP_CLOSED_CRITICAL_STREAM",
            0x07 => "QDP_STREAM_CREATION_ERROR",
            0x08 => "QDP_MESSAGE_TOO_LARGE",
            0x09 => "QDP_RESOURCE_LIMIT",
            0x0a => "QDP_HANDSHAKE_TIMEOUT",
            0x0b => "QDP_SESSION_REPLACED",
            0x0c => "QDP_HOST_SHUTDOWN",
            0x0d => "QDP_POLICY",
            0x10 => "QDP_STREAM_TYPE_UNKNOWN",
            0x11 => "QDP_REQUEST_CANCELLED",
            _ => return None,
        };
        Some(name)
    }

    /// Returns the meaning of this code when it arrives in a CONNECTION_CLOSE frame.
    ///
    /// An endpoint treats an unknown connection error code, including a stream-only
    /// code, as `QDP_INTERNAL_ERROR` (Section 12.6).
    pub const fn as_connection_error(self) -> Self {
        if self.0.into_inner() <= Self::MAX_CONNECTION_CODE.0.into_inner() && self.name().is_some()
        {
            self
        } else {
            Self::INTERNAL_ERROR
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_code(formatter, self.name(), self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first reserved values match the list in Section 1.6.
    #[test]
    fn grease_values() {
        let expected = [0x21, 0x40, 0x5f, 0x7e];
        for (index, value) in expected.into_iter().enumerate() {
            let grease = grease_value(index as u64).unwrap();
            assert_eq!(grease.into_inner(), value);
            assert!(is_grease(grease));
        }
        assert!(grease_value(u64::MAX).is_none());
    }

    /// Values between reserved values, and below the first one, are not reserved.
    #[test]
    fn non_grease_values() {
        for value in [0x00, 0x01, 0x20, 0x22, 0x3f, 0x41] {
            assert!(!is_grease(VarInt::from_u32(value)), "{value:#x}");
        }
    }

    /// No assigned code in any type or identifier space is reserved for greasing.
    #[test]
    fn assigned_codes_are_not_grease() {
        for value in 0..=0xff {
            let value = VarInt::from_u32(value);
            if MessageType(value).name().is_some() {
                assert!(!MessageType(value).is_grease());
            }
            if DatagramType(value).name().is_some() {
                assert!(!DatagramType(value).is_grease());
            }
            if StreamType(value).name().is_some() {
                assert!(!StreamType(value).is_grease());
            }
            if CapabilityId(value).name().is_some() {
                assert!(!CapabilityId(value).is_grease());
            }
            if RecordType(value).name().is_some() {
                assert!(!RecordType(value).is_grease());
            }
        }
    }

    /// Codes display with their name when they have one.
    #[test]
    fn display() {
        assert_eq!(MessageType::CLIENT_READY.to_string(), "CLIENT_READY (0x06)");
        assert_eq!(MessageType(VarInt::from_u32(0x21)).to_string(), "0x21");
        assert_eq!(ErrorCode::POLICY.to_string(), "QDP_POLICY (0x0d)");
    }

    /// Unknown and stream-only codes close a connection as an internal error.
    #[test]
    fn connection_error_interpretation() {
        assert_eq!(ErrorCode::POLICY.as_connection_error(), ErrorCode::POLICY);
        assert_eq!(
            ErrorCode::STREAM_TYPE_UNKNOWN.as_connection_error(),
            ErrorCode::INTERNAL_ERROR
        );
        assert_eq!(
            ErrorCode(VarInt::from_u32(0x0e)).as_connection_error(),
            ErrorCode::INTERNAL_ERROR
        );
    }

    /// Only the codec capabilities are repeatable.
    #[test]
    fn repeatable_capabilities() {
        assert!(CapabilityId::VIDEO_CODEC.is_repeatable());
        assert!(CapabilityId::AUDIO_CODEC.is_repeatable());
        assert!(!CapabilityId::INPUT.is_repeatable());
    }
}
