//! Typed QDP control messages and datagrams.
//!
//! Each message is a struct that implements [`MessagePayload`], and each datagram a
//! struct that implements [`DatagramPayload`]. [`ControlMessage`] and [`Datagram`]
//! dispatch on the type code of a received frame. Decoding follows the extensibility
//! rules of Section 1.6: bytes after the last known field of an extensible structure
//! are ignored, and a structure shorter than its fields is an error.

mod capability;
mod session;
mod timing;

pub use capability::{
    AudioCodec, AudioCodecCapability, Av1Profiles, Capability, ClipboardCapability, Directions,
    HdrFormats, InputClasses, MAX_CAPABILITIES, MAX_CAPABILITY_LENGTH, NO_LEVEL_LIMIT, VideoCodec,
    VideoCodecCapability, VideoCodecFlags,
};
pub use session::{
    AuthContinue, AuthMethod, AuthResult, AuthStart, AuthStatus, ClientHello, ClientReady,
    HostHello, MAX_AUTH_DATA_LENGTH, MAX_NAME_LENGTH, MAX_RESUMPTION_TOKEN_LENGTH, MinorVersion,
    PasswordAuthData, ResumptionToken, SessionStart,
};
pub use timing::{PingId, TimePing, TimePong, Timestamp};

use crate::registry::{DatagramType, MessageType};
use crate::wire::{ControlMessageFrame, DatagramFrame, Reader, WireError, Writer};

/// The payload of a control message of one type.
pub trait MessagePayload: Sized {
    /// The Message Type that carries this payload.
    const MESSAGE_TYPE: MessageType;

    /// Appends the payload's fields to `writer`.
    fn encode(&self, writer: &mut Writer);

    /// Reads the payload's fields from `reader`, ignoring any bytes after them.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError>;

    /// Returns the control message frame that carries this payload.
    fn to_frame(&self) -> ControlMessageFrame {
        let mut writer = Writer::new();
        self.encode(&mut writer);
        ControlMessageFrame::new(Self::MESSAGE_TYPE, writer.into_bytes())
    }
}

/// The payload of a datagram of one type.
pub trait DatagramPayload: Sized {
    /// The Datagram Type that carries this payload.
    const DATAGRAM_TYPE: DatagramType;

    /// Appends the payload's fields to `writer`.
    fn encode(&self, writer: &mut Writer);

    /// Reads the payload's fields from `reader`.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError>;

    /// Returns the encoded datagram, including its type.
    fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(Self::DATAGRAM_TYPE.0);
        self.encode(&mut writer);
        writer.into_bytes()
    }
}

/// Decodes the payload of `frame` as the message type `M`.
fn decode_message<M: MessagePayload>(frame: &ControlMessageFrame) -> Result<M, WireError> {
    M::decode(&mut Reader::new(&frame.payload))
}

/// Any control message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    /// `CLIENT_HELLO` (Section 4.3.1).
    ClientHello(ClientHello),
    /// `HOST_HELLO` (Section 4.3.2).
    HostHello(HostHello),
    /// `AUTH_START` (Section 4.4.1).
    AuthStart(AuthStart),
    /// `AUTH_CONTINUE` (Section 4.4.1).
    AuthContinue(AuthContinue),
    /// `AUTH_RESULT` (Section 4.4.1).
    AuthResult(AuthResult),
    /// `CLIENT_READY` (Section 4.5).
    ClientReady(ClientReady),
    /// `SESSION_START` (Section 4.5).
    SessionStart(SessionStart),
    /// `RESUMPTION_TOKEN` (Section 4.4.7).
    ResumptionToken(ResumptionToken),
    /// A message of a type this implementation does not decode, which the receiver
    /// ignores (Section 3.6.2).
    Unknown(ControlMessageFrame),
}

impl ControlMessage {
    /// Decodes a received control message frame.
    pub fn decode(frame: &ControlMessageFrame) -> Result<Self, WireError> {
        let message = match frame.message_type {
            MessageType::CLIENT_HELLO => Self::ClientHello(decode_message(frame)?),
            MessageType::HOST_HELLO => Self::HostHello(decode_message(frame)?),
            MessageType::AUTH_START => Self::AuthStart(decode_message(frame)?),
            MessageType::AUTH_CONTINUE => Self::AuthContinue(decode_message(frame)?),
            MessageType::AUTH_RESULT => Self::AuthResult(decode_message(frame)?),
            MessageType::CLIENT_READY => Self::ClientReady(decode_message(frame)?),
            MessageType::SESSION_START => Self::SessionStart(decode_message(frame)?),
            MessageType::RESUMPTION_TOKEN => Self::ResumptionToken(decode_message(frame)?),
            _ => Self::Unknown(frame.clone()),
        };
        Ok(message)
    }

    /// Returns the control message frame that carries this message.
    pub fn to_frame(&self) -> ControlMessageFrame {
        match self {
            Self::ClientHello(message) => message.to_frame(),
            Self::HostHello(message) => message.to_frame(),
            Self::AuthStart(message) => message.to_frame(),
            Self::AuthContinue(message) => message.to_frame(),
            Self::AuthResult(message) => message.to_frame(),
            Self::ClientReady(message) => message.to_frame(),
            Self::SessionStart(message) => message.to_frame(),
            Self::ResumptionToken(message) => message.to_frame(),
            Self::Unknown(frame) => frame.clone(),
        }
    }

    /// Returns the Message Type of this message.
    pub fn message_type(&self) -> MessageType {
        match self {
            Self::ClientHello(_) => ClientHello::MESSAGE_TYPE,
            Self::HostHello(_) => HostHello::MESSAGE_TYPE,
            Self::AuthStart(_) => AuthStart::MESSAGE_TYPE,
            Self::AuthContinue(_) => AuthContinue::MESSAGE_TYPE,
            Self::AuthResult(_) => AuthResult::MESSAGE_TYPE,
            Self::ClientReady(_) => ClientReady::MESSAGE_TYPE,
            Self::SessionStart(_) => SessionStart::MESSAGE_TYPE,
            Self::ResumptionToken(_) => ResumptionToken::MESSAGE_TYPE,
            Self::Unknown(frame) => frame.message_type,
        }
    }
}

/// Any datagram, borrowing the bytes of the QUIC DATAGRAM frame it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Datagram<'a> {
    /// `TIME_PING` (Section 5.2.1).
    TimePing(TimePing),
    /// `TIME_PONG` (Section 5.2.1).
    TimePong(TimePong),
    /// A datagram of a type this implementation does not decode, which the receiver
    /// drops (Section 3.7.3).
    Unknown(DatagramFrame<'a>),
}

impl<'a> Datagram<'a> {
    /// Decodes the contents of a received QUIC DATAGRAM frame.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, WireError> {
        let frame = DatagramFrame::decode(bytes)?;
        let mut payload = Reader::new(frame.payload);
        let datagram = match frame.datagram_type {
            DatagramType::TIME_PING => Self::TimePing(TimePing::decode(&mut payload)?),
            DatagramType::TIME_PONG => Self::TimePong(TimePong::decode(&mut payload)?),
            _ => Self::Unknown(frame),
        };
        Ok(datagram)
    }

    /// Returns the encoded datagram, including its type.
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Self::TimePing(datagram) => datagram.to_bytes(),
            Self::TimePong(datagram) => datagram.to_bytes(),
            Self::Unknown(frame) => {
                let mut writer = Writer::with_capacity(frame.encoded_len());
                frame.encode(&mut writer);
                writer.into_bytes()
            }
        }
    }

    /// Returns the Datagram Type of this datagram.
    pub fn datagram_type(&self) -> DatagramType {
        match self {
            Self::TimePing(_) => TimePing::DATAGRAM_TYPE,
            Self::TimePong(_) => TimePong::DATAGRAM_TYPE,
            Self::Unknown(frame) => frame.datagram_type,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{ControlStreamDecoder, VarInt};

    /// A message survives encoding, framing, stream decoding and dispatch.
    #[test]
    fn control_message_round_trip() {
        let message = ControlMessage::AuthContinue(AuthContinue {
            auth_data: b"r=nonce".to_vec(),
        });
        let bytes = message.to_frame().to_bytes().unwrap();

        let mut decoder = ControlStreamDecoder::new();
        decoder.push(&bytes);
        let frame = decoder.next_message().unwrap().unwrap();
        assert_eq!(ControlMessage::decode(&frame).unwrap(), message);
        assert_eq!(message.message_type(), MessageType::AUTH_CONTINUE);
    }

    /// `CLIENT_READY` matches the example of Section 3.12.
    #[test]
    fn client_ready_example() {
        let bytes = ControlMessage::ClientReady(ClientReady)
            .to_frame()
            .to_bytes()
            .unwrap();
        assert_eq!(bytes, [0x06, 0x00]);
    }

    /// Unknown message types are kept as frames so the receiver can ignore them.
    #[test]
    fn unknown_message() {
        let frame = ControlMessageFrame::new(MessageType(VarInt::from_u32(0x40)), vec![1, 2]);
        let message = ControlMessage::decode(&frame).unwrap();
        assert_eq!(message, ControlMessage::Unknown(frame.clone()));
        assert_eq!(message.to_frame(), frame);
    }

    /// Bytes after the known fields of a message are ignored.
    #[test]
    fn trailing_bytes_ignored() {
        let mut frame = AuthContinue { auth_data: vec![7] }.to_frame();
        frame.payload.extend([0xff, 0xff]);
        let message = ControlMessage::decode(&frame).unwrap();
        assert_eq!(
            message,
            ControlMessage::AuthContinue(AuthContinue { auth_data: vec![7] })
        );
    }

    /// `TIME_PING` matches the example of Section 3.12.
    #[test]
    fn time_ping_example() {
        let bytes = [0x07, 0x07, 0x80, 0x16, 0xe3, 0x60];
        let datagram = Datagram::decode(&bytes).unwrap();
        assert_eq!(
            datagram,
            Datagram::TimePing(TimePing {
                ping_id: PingId(VarInt::from_u32(7)),
                origin_timestamp: Timestamp(VarInt::from_u32(1_500_000)),
            })
        );
        assert_eq!(datagram.to_bytes(), bytes);
        assert_eq!(datagram.datagram_type(), DatagramType::TIME_PING);
    }

    /// Unknown datagram types decode without error so the receiver can drop them.
    #[test]
    fn unknown_datagram() {
        let bytes = [0x21, 0xde, 0xad];
        let datagram = Datagram::decode(&bytes).unwrap();
        assert!(matches!(datagram, Datagram::Unknown(_)));
        assert_eq!(datagram.to_bytes(), bytes);
    }
}
