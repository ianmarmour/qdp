//! Framing of control messages and datagrams (Sections 3.6 and 3.7).

use crate::registry::{DatagramType, MessageType};
use crate::wire::{Reader, VarInt, WireError, Writer};

/// The largest Message Length a control message may have, 1 MiB (Section 3.6.1).
pub const MAX_CONTROL_MESSAGE_LENGTH: u64 = 1 << 20;

/// The number of bytes of control messages a host accepts before authentication
/// succeeds, 64 KiB (Section 11.7).
pub const PRE_AUTHENTICATION_BUDGET: u64 = 64 << 10;

/// A control message whose payload has not yet been decoded (Section 3.6.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlMessageFrame {
    /// The Message Type.
    pub message_type: MessageType,
    /// The Message Payload, which the Message Length delimits.
    pub payload: Vec<u8>,
}

impl ControlMessageFrame {
    /// Creates a control message frame.
    pub fn new(message_type: MessageType, payload: Vec<u8>) -> Self {
        Self {
            message_type,
            payload,
        }
    }

    /// Appends the encoded message to `writer`.
    ///
    /// Fails if the payload is longer than [`MAX_CONTROL_MESSAGE_LENGTH`]; such data
    /// belongs on a unidirectional stream.
    pub fn encode(&self, writer: &mut Writer) -> Result<(), WireError> {
        let length = self.payload.len() as u64;
        if length > MAX_CONTROL_MESSAGE_LENGTH {
            return Err(WireError::MessageTooLarge {
                length,
                limit: MAX_CONTROL_MESSAGE_LENGTH,
            });
        }
        writer.write_varint(self.message_type.0);
        writer.write_octets(&self.payload);
        Ok(())
    }

    /// Returns the encoded message.
    pub fn to_bytes(&self) -> Result<Vec<u8>, WireError> {
        let mut writer = Writer::with_capacity(self.payload.len() + 2 * VarInt::MAX.encoded_len());
        self.encode(&mut writer)?;
        Ok(writer.into_bytes())
    }
}

/// The header of a control message: its type and the length of its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ControlMessageHeader {
    /// The Message Type.
    message_type: MessageType,
    /// The Message Length.
    length: u64,
    /// The number of bytes the two header fields take on the wire.
    encoded_len: usize,
}

/// Splits the byte stream of one direction of the control stream into control
/// messages.
///
/// Bytes arrive from QUIC in arbitrary chunks. The caller passes each chunk to
/// [`ControlStreamDecoder::push`] and then calls
/// [`ControlStreamDecoder::next_message`] until it returns `Ok(None)`. A message
/// whose Message Length exceeds the limit is rejected as soon as its header is
/// complete, before any of its payload is buffered for it.
#[derive(Debug, Clone, Default)]
pub struct ControlStreamDecoder {
    /// Received bytes, of which those before `start` have been consumed.
    buffer: Vec<u8>,
    /// The offset in `buffer` of the first byte not yet consumed.
    start: usize,
    /// The bytes of control messages still accepted before authentication, if the
    /// budget of Section 11.7 applies.
    budget: Option<u64>,
}

impl ControlStreamDecoder {
    /// Creates a decoder that accepts messages up to the 1 MiB limit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a decoder that also applies the pre-authentication budget, as a host
    /// does for the client's direction of the control stream until authentication
    /// succeeds (Section 11.7).
    pub fn with_pre_authentication_budget() -> Self {
        Self {
            budget: Some(PRE_AUTHENTICATION_BUDGET),
            ..Self::default()
        }
    }

    /// Stops applying the pre-authentication budget, once authentication succeeds.
    pub fn clear_budget(&mut self) {
        self.budget = None;
    }

    /// Returns the bytes of control messages the decoder still accepts before
    /// authentication, or `None` if the budget does not apply.
    pub const fn remaining_budget(&self) -> Option<u64> {
        self.budget
    }

    /// Returns the number of bytes received but not yet returned as messages.
    pub fn buffered_len(&self) -> usize {
        self.buffer.len() - self.start
    }

    /// Adds bytes received on the control stream.
    pub fn push(&mut self, data: &[u8]) {
        if self.start > 0 {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        self.buffer.extend_from_slice(data);
    }

    /// Returns the next complete control message, or `Ok(None)` if more bytes are
    /// needed.
    ///
    /// An error is a connection error with the code [`WireError::error_code`] gives;
    /// the decoder should not be used after it.
    pub fn next_message(&mut self) -> Result<Option<ControlMessageFrame>, WireError> {
        let pending = &self.buffer[self.start..];
        let Some(header) = Self::read_header(pending)? else {
            return Ok(None);
        };
        self.check_limits(header)?;

        // The length fits in `usize` because it is no larger than the 1 MiB limit.
        let end = header.encoded_len + header.length as usize;
        let Some(payload) = pending.get(header.encoded_len..end) else {
            return Ok(None);
        };
        let frame = ControlMessageFrame::new(header.message_type, payload.to_vec());

        self.start += end;
        if let Some(budget) = &mut self.budget {
            *budget = budget.saturating_sub(end as u64);
        }
        if self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        }
        Ok(Some(frame))
    }

    /// Reads a message header from the front of `pending`, or returns `Ok(None)` if
    /// it is not yet complete.
    fn read_header(pending: &[u8]) -> Result<Option<ControlMessageHeader>, WireError> {
        let mut reader = Reader::new(pending);
        let fields = reader
            .read_varint()
            .and_then(|message_type| Ok((message_type, reader.read_varint()?)));
        match fields {
            Ok((message_type, length)) => Ok(Some(ControlMessageHeader {
                message_type: MessageType(message_type),
                length: length.into_inner(),
                encoded_len: pending.len() - reader.remaining(),
            })),
            Err(WireError::UnexpectedEnd) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Checks a message's length against the 1 MiB limit and any remaining
    /// pre-authentication budget.
    fn check_limits(&self, header: ControlMessageHeader) -> Result<(), WireError> {
        let limit = match self.budget {
            Some(budget) => budget.min(MAX_CONTROL_MESSAGE_LENGTH),
            None => MAX_CONTROL_MESSAGE_LENGTH,
        };
        if header.length > limit {
            return Err(WireError::MessageTooLarge {
                length: header.length,
                limit,
            });
        }
        Ok(())
    }
}

/// A datagram whose payload has not yet been decoded (Section 3.7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatagramFrame<'a> {
    /// The Datagram Type.
    pub datagram_type: DatagramType,
    /// The Datagram Payload, which runs to the end of the QUIC DATAGRAM frame.
    pub payload: &'a [u8],
}

impl<'a> DatagramFrame<'a> {
    /// Creates a datagram frame.
    pub const fn new(datagram_type: DatagramType, payload: &'a [u8]) -> Self {
        Self {
            datagram_type,
            payload,
        }
    }

    /// Splits the contents of a QUIC DATAGRAM frame into its type and payload.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(bytes);
        let datagram_type = DatagramType(reader.read_varint()?);
        Ok(Self::new(datagram_type, reader.read_rest()))
    }

    /// Appends the encoded datagram to `writer`.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.datagram_type.0);
        writer.write_bytes(self.payload);
    }

    /// Returns the number of bytes of the encoded datagram.
    pub const fn encoded_len(&self) -> usize {
        self.datagram_type.0.encoded_len() + self.payload.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `CLIENT_READY` encodes as in the example of Section 3.12.
    #[test]
    fn client_ready_example() {
        let frame = ControlMessageFrame::new(MessageType::CLIENT_READY, Vec::new());
        assert_eq!(frame.to_bytes().unwrap(), [0x06, 0x00]);

        let mut decoder = ControlStreamDecoder::new();
        decoder.push(&[0x06, 0x00]);
        assert_eq!(decoder.next_message().unwrap(), Some(frame));
        assert_eq!(decoder.next_message().unwrap(), None);
    }

    /// `TIME_PING` encodes as in the example of Section 3.12.
    #[test]
    fn time_ping_example() {
        let bytes = [0x07, 0x07, 0x80, 0x16, 0xe3, 0x60];
        let datagram = DatagramFrame::decode(&bytes).unwrap();
        assert_eq!(datagram.datagram_type, DatagramType::TIME_PING);

        let mut payload = Reader::new(datagram.payload);
        assert_eq!(payload.read_varint().unwrap().into_inner(), 7);
        assert_eq!(payload.read_varint().unwrap().into_inner(), 1_500_000);

        let mut writer = Writer::new();
        datagram.encode(&mut writer);
        assert_eq!(writer.as_bytes(), bytes);
        assert_eq!(datagram.encoded_len(), bytes.len());
    }

    /// An empty datagram is shorter than its defined fields.
    #[test]
    fn empty_datagram() {
        assert_eq!(DatagramFrame::decode(&[]), Err(WireError::UnexpectedEnd));
    }

    /// Messages split across chunks at every byte are reassembled.
    #[test]
    fn messages_split_across_chunks() {
        let first = ControlMessageFrame::new(MessageType::AUTH_START, vec![1, 2, 3]);
        let second = ControlMessageFrame::new(MessageType(VarInt::from_u32(0x5f)), vec![9; 300]);
        let mut bytes = first.to_bytes().unwrap();
        bytes.extend(second.to_bytes().unwrap());

        let mut decoder = ControlStreamDecoder::new();
        let mut decoded = Vec::new();
        for byte in &bytes {
            decoder.push(std::slice::from_ref(byte));
            while let Some(frame) = decoder.next_message().unwrap() {
                decoded.push(frame);
            }
        }
        assert_eq!(decoded, [first, second]);
        assert_eq!(decoder.buffered_len(), 0);
    }

    /// Several messages in one chunk are returned one at a time.
    #[test]
    fn several_messages_in_one_chunk() {
        let mut decoder = ControlStreamDecoder::new();
        decoder.push(&[0x06, 0x00, 0x07, 0x01, 0xaa, 0x08]);
        let first = decoder.next_message().unwrap().unwrap();
        let second = decoder.next_message().unwrap().unwrap();
        assert_eq!(first.message_type, MessageType::CLIENT_READY);
        assert_eq!(second.message_type, MessageType::SESSION_START);
        assert_eq!(second.payload, [0xaa]);
        assert_eq!(decoder.next_message().unwrap(), None);
        assert_eq!(decoder.buffered_len(), 1);
    }

    /// A Message Length above 1 MiB is rejected from the header alone.
    #[test]
    fn rejects_oversized_message_before_payload() {
        let mut header = Writer::new();
        header.write_varint(MessageType::CLIPBOARD_OFFER.0);
        header.write_varint(VarInt::new(MAX_CONTROL_MESSAGE_LENGTH + 1).unwrap());

        let mut decoder = ControlStreamDecoder::new();
        decoder.push(header.as_bytes());
        let error = decoder.next_message().unwrap_err();
        assert_eq!(
            error,
            WireError::MessageTooLarge {
                length: MAX_CONTROL_MESSAGE_LENGTH + 1,
                limit: MAX_CONTROL_MESSAGE_LENGTH,
            }
        );
        assert_eq!(
            error.error_code(),
            crate::registry::ErrorCode::MESSAGE_TOO_LARGE
        );
    }

    /// A message of exactly 1 MiB is accepted.
    #[test]
    fn accepts_largest_message() {
        let frame = ControlMessageFrame::new(
            MessageType::CLIPBOARD_OFFER,
            vec![0; MAX_CONTROL_MESSAGE_LENGTH as usize],
        );
        let mut decoder = ControlStreamDecoder::new();
        decoder.push(&frame.to_bytes().unwrap());
        assert_eq!(decoder.next_message().unwrap(), Some(frame));
    }

    /// The sender refuses to encode a payload above 1 MiB.
    #[test]
    fn refuses_to_encode_oversized_message() {
        let frame = ControlMessageFrame::new(
            MessageType::CLIPBOARD_OFFER,
            vec![0; MAX_CONTROL_MESSAGE_LENGTH as usize + 1],
        );
        assert!(matches!(
            frame.to_bytes(),
            Err(WireError::MessageTooLarge { .. })
        ));
    }

    /// The pre-authentication budget shrinks with every message and rejects a
    /// Message Length larger than what remains.
    #[test]
    fn pre_authentication_budget() {
        let mut decoder = ControlStreamDecoder::with_pre_authentication_budget();
        let hello = ControlMessageFrame::new(MessageType::CLIENT_HELLO, vec![0; 60_000]);
        let hello_bytes = hello.to_bytes().unwrap();
        decoder.push(&hello_bytes);
        assert_eq!(decoder.next_message().unwrap(), Some(hello));

        let remaining = PRE_AUTHENTICATION_BUDGET - hello_bytes.len() as u64;
        assert_eq!(decoder.remaining_budget(), Some(remaining));

        let mut header = Writer::new();
        header.write_varint(MessageType::AUTH_START.0);
        header.write_varint(VarInt::new(remaining + 1).unwrap());
        decoder.push(header.as_bytes());
        assert_eq!(
            decoder.next_message(),
            Err(WireError::MessageTooLarge {
                length: remaining + 1,
                limit: remaining,
            })
        );
    }

    /// Once the budget is cleared, only the 1 MiB limit applies.
    #[test]
    fn cleared_budget() {
        let mut decoder = ControlStreamDecoder::with_pre_authentication_budget();
        decoder.clear_budget();
        assert_eq!(decoder.remaining_budget(), None);
        let frame = ControlMessageFrame::new(MessageType::CLIPBOARD_OFFER, vec![0; 100_000]);
        decoder.push(&frame.to_bytes().unwrap());
        assert_eq!(decoder.next_message().unwrap(), Some(frame));
    }
}
