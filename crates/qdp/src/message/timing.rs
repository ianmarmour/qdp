//! Clock synchronization datagrams (Section 5.2).

use crate::message::DatagramPayload;
use crate::registry::DatagramType;
use crate::wire::{Reader, VarInt, WireError, Writer};

/// A time on the sender's session clock, in microseconds since the epoch the sender
/// chose when the session started (Sections 1.5.5 and 5.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub VarInt);

impl Timestamp {
    /// Returns the timestamp in microseconds.
    pub const fn as_micros(self) -> u64 {
        self.0.into_inner()
    }

    /// Reads a Timestamp field.
    pub(crate) fn read(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        reader.read_varint().map(Self)
    }

    /// Writes a Timestamp field.
    pub(crate) fn write(self, writer: &mut Writer) {
        writer.write_varint(self.0);
    }
}

/// The identifier that matches a `TIME_PONG` to the `TIME_PING` it answers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PingId(pub VarInt);

/// A `TIME_PING` datagram (Section 5.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimePing {
    /// The identifier the answering `TIME_PONG` echoes.
    pub ping_id: PingId,
    /// When the ping was sent, on the sender's clock (T1).
    pub origin_timestamp: Timestamp,
}

impl DatagramPayload for TimePing {
    const DATAGRAM_TYPE: DatagramType = DatagramType::TIME_PING;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.ping_id.0);
        self.origin_timestamp.write(writer);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            ping_id: PingId(reader.read_varint()?),
            origin_timestamp: Timestamp::read(reader)?,
        })
    }
}

/// A `TIME_PONG` datagram (Section 5.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimePong {
    /// The Ping ID of the `TIME_PING` being answered.
    pub ping_id: PingId,
    /// The Origin Timestamp of the `TIME_PING` being answered (T1).
    pub origin_timestamp: Timestamp,
    /// When the ping arrived, on the answering endpoint's clock (T2).
    pub receive_timestamp: Timestamp,
    /// When the pong was sent, on the answering endpoint's clock (T3).
    pub transmit_timestamp: Timestamp,
}

impl DatagramPayload for TimePong {
    const DATAGRAM_TYPE: DatagramType = DatagramType::TIME_PONG;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.ping_id.0);
        self.origin_timestamp.write(writer);
        self.receive_timestamp.write(writer);
        self.transmit_timestamp.write(writer);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            ping_id: PingId(reader.read_varint()?),
            origin_timestamp: Timestamp::read(reader)?,
            receive_timestamp: Timestamp::read(reader)?,
            transmit_timestamp: Timestamp::read(reader)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a timestamp of `micros` microseconds.
    fn timestamp(micros: u32) -> Timestamp {
        Timestamp(VarInt::from_u32(micros))
    }

    /// A `TIME_PONG` round-trips and ignores fields added by later versions.
    #[test]
    fn time_pong_round_trip() {
        let pong = TimePong {
            ping_id: PingId(VarInt::from_u32(7)),
            origin_timestamp: timestamp(1_500_000),
            receive_timestamp: timestamp(9_000_000),
            transmit_timestamp: timestamp(9_000_050),
        };
        let mut bytes = pong.to_bytes();
        assert_eq!(bytes[0], 0x08);
        bytes.push(0x2a);

        let mut reader = Reader::new(&bytes[1..]);
        assert_eq!(TimePong::decode(&mut reader).unwrap(), pong);
    }

    /// A truncated `TIME_PONG` is an error.
    #[test]
    fn truncated_time_pong() {
        let mut reader = Reader::new(&[0x07, 0x01, 0x02]);
        assert_eq!(TimePong::decode(&mut reader), Err(WireError::UnexpectedEnd));
    }
}
