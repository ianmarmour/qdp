//! The `VIDEO_FEEDBACK` datagram and its records (Sections 5.3 and 6.7).

use crate::message::{DatagramPayload, FrameNumber, MonitorId, StreamId, Timestamp};
use crate::registry::{DatagramType, RecordType};
use crate::wire::{Reader, VarInt, WireError, Writer};

/// A run of fragments of one frame that the client is missing (Section 6.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissingRange {
    /// The index of the first missing fragment.
    pub first_index: VarInt,
    /// The number of missing fragments, or 0 for the first and every later one.
    pub count: VarInt,
}

/// A `DECODED` record: the highest frame of a stream the client has decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedRecord {
    /// The video stream.
    pub stream_id: StreamId,
    /// The highest Frame Number decoded.
    pub last_decoded_frame: FrameNumber,
}

/// A `NACK` record: fragments of one frame that the client is missing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NackRecord {
    /// The video stream.
    pub stream_id: StreamId,
    /// The incomplete frame.
    pub frame_number: FrameNumber,
    /// The missing fragments.
    pub missing: Vec<MissingRange>,
}

/// A `LOSS` record: the client stopped decoding a stream after Last Decoded Frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossRecord {
    /// The video stream.
    pub stream_id: StreamId,
    /// The last frame decoded, or [`FrameNumber::NONE`] in the `KEY_ONLY` state.
    pub last_decoded_frame: FrameNumber,
    /// The highest frame of which the client has received any fragment.
    pub highest_frame_seen: FrameNumber,
}

/// A `FRAME_TIMING` record: when one frame arrived, decoded and was presented, on
/// the client's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTimingRecord {
    /// The video stream.
    pub stream_id: StreamId,
    /// The frame.
    pub frame_number: FrameNumber,
    /// When the frame's first fragment arrived.
    pub first_fragment_time: Timestamp,
    /// When the frame's last fragment arrived.
    pub last_fragment_time: Timestamp,
    /// When the frame finished decoding.
    pub decoded_time: Timestamp,
    /// When the frame was presented, or 0 if a newer frame replaced it first.
    pub presented_time: Timestamp,
}

/// The flags of a `DISPLAY_TIMING` record (Section 5.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DisplayTimingFlags(pub u8);

impl DisplayTimingFlags {
    /// The display is in variable refresh mode.
    pub const VRR_ACTIVE: Self = Self(0x01);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A `DISPLAY_TIMING` record: the refresh timing of the display that shows one
/// host monitor (Section 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplayTimingRecord {
    /// The host monitor shown on the display.
    pub monitor_id: MonitorId,
    /// The refresh period in nanoseconds, or 0 if unknown.
    pub refresh_period_nanos: VarInt,
    /// A recent start of vertical blanking, on the client's clock.
    pub vsync_timestamp: Timestamp,
    /// Variable refresh flag.
    pub flags: DisplayTimingFlags,
}

/// One record of a `VIDEO_FEEDBACK` datagram (Section 6.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedbackRecord {
    /// `DECODED`.
    Decoded(DecodedRecord),
    /// `NACK`.
    Nack(NackRecord),
    /// `LOSS`.
    Loss(LossRecord),
    /// `FRAME_TIMING`.
    FrameTiming(FrameTimingRecord),
    /// `DISPLAY_TIMING`.
    DisplayTiming(DisplayTimingRecord),
    /// A record of a type this implementation does not know, which the receiver
    /// ignores.
    Unknown {
        /// The Record Type.
        record_type: RecordType,
        /// The Record Value.
        value: Vec<u8>,
    },
}

impl FeedbackRecord {
    /// Returns the Record Type of this record.
    pub fn record_type(&self) -> RecordType {
        match self {
            Self::Decoded(_) => RecordType::DECODED,
            Self::Nack(_) => RecordType::NACK,
            Self::Loss(_) => RecordType::LOSS,
            Self::FrameTiming(_) => RecordType::FRAME_TIMING,
            Self::DisplayTiming(_) => RecordType::DISPLAY_TIMING,
            Self::Unknown { record_type, .. } => *record_type,
        }
    }

    /// Appends the record, with its type and length, to `writer`.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.record_type().0);
        writer.write_length_prefixed(|value| match self {
            Self::Decoded(record) => {
                value.write_varint(record.stream_id.0);
                value.write_varint(record.last_decoded_frame.0);
            }
            Self::Nack(record) => {
                value.write_varint(record.stream_id.0);
                value.write_varint(record.frame_number.0);
                value.write_count(record.missing.len());
                for range in &record.missing {
                    value.write_varint(range.first_index);
                    value.write_varint(range.count);
                }
            }
            Self::Loss(record) => {
                value.write_varint(record.stream_id.0);
                value.write_varint(record.last_decoded_frame.0);
                value.write_varint(record.highest_frame_seen.0);
            }
            Self::FrameTiming(record) => {
                value.write_varint(record.stream_id.0);
                value.write_varint(record.frame_number.0);
                record.first_fragment_time.write(value);
                record.last_fragment_time.write(value);
                record.decoded_time.write(value);
                record.presented_time.write(value);
            }
            Self::DisplayTiming(record) => {
                value.write_varint(record.monitor_id.0);
                value.write_varint(record.refresh_period_nanos);
                record.vsync_timestamp.write(value);
                value.write_u8(record.flags.0);
            }
            Self::Unknown { value: bytes, .. } => value.write_bytes(bytes),
        });
    }

    /// Reads a record, with its type and length, from `reader`.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let record_type = RecordType(reader.read_varint()?);
        let bytes = reader.read_octets()?;
        let mut value = Reader::new(bytes);
        let record = match record_type {
            RecordType::DECODED => Self::Decoded(DecodedRecord {
                stream_id: StreamId(value.read_varint()?),
                last_decoded_frame: FrameNumber(value.read_varint()?),
            }),
            RecordType::NACK => {
                let stream_id = StreamId(value.read_varint()?);
                let frame_number = FrameNumber(value.read_varint()?);
                let range_count = value.read_count("Range Count", u64::MAX)?;
                let missing = (0..range_count)
                    .map(|_| {
                        Ok(MissingRange {
                            first_index: value.read_varint()?,
                            count: value.read_varint()?,
                        })
                    })
                    .collect::<Result<_, WireError>>()?;
                Self::Nack(NackRecord {
                    stream_id,
                    frame_number,
                    missing,
                })
            }
            RecordType::LOSS => Self::Loss(LossRecord {
                stream_id: StreamId(value.read_varint()?),
                last_decoded_frame: FrameNumber(value.read_varint()?),
                highest_frame_seen: FrameNumber(value.read_varint()?),
            }),
            RecordType::FRAME_TIMING => Self::FrameTiming(FrameTimingRecord {
                stream_id: StreamId(value.read_varint()?),
                frame_number: FrameNumber(value.read_varint()?),
                first_fragment_time: Timestamp::read(&mut value)?,
                last_fragment_time: Timestamp::read(&mut value)?,
                decoded_time: Timestamp::read(&mut value)?,
                presented_time: Timestamp::read(&mut value)?,
            }),
            RecordType::DISPLAY_TIMING => Self::DisplayTiming(DisplayTimingRecord {
                monitor_id: MonitorId(value.read_varint()?),
                refresh_period_nanos: value.read_varint()?,
                vsync_timestamp: Timestamp::read(&mut value)?,
                flags: DisplayTimingFlags(value.read_u8()?),
            }),
            _ => Self::Unknown {
                record_type,
                value: bytes.to_vec(),
            },
        };
        Ok(record)
    }
}

/// A `VIDEO_FEEDBACK` datagram (Section 6.7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VideoFeedback {
    /// The records, for any streams.
    pub records: Vec<FeedbackRecord>,
}

impl DatagramPayload<'_> for VideoFeedback {
    const DATAGRAM_TYPE: DatagramType = DatagramType::VIDEO_FEEDBACK;

    fn encode(&self, writer: &mut Writer) {
        for record in &self.records {
            record.encode(writer);
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let mut records = Vec::new();
        while !reader.is_empty() {
            records.push(FeedbackRecord::decode(reader)?);
        }
        Ok(Self { records })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a stream ID.
    fn stream() -> StreamId {
        StreamId(VarInt::from_u32(1))
    }

    /// Returns a frame number.
    fn frame(number: u32) -> FrameNumber {
        FrameNumber(VarInt::from_u32(number))
    }

    /// Every record kind round-trips in one datagram.
    #[test]
    fn records_round_trip() {
        let feedback = VideoFeedback {
            records: vec![
                FeedbackRecord::Decoded(DecodedRecord {
                    stream_id: stream(),
                    last_decoded_frame: frame(41),
                }),
                FeedbackRecord::Nack(NackRecord {
                    stream_id: stream(),
                    frame_number: frame(42),
                    missing: vec![
                        MissingRange {
                            first_index: VarInt::from_u32(3),
                            count: VarInt::from_u32(2),
                        },
                        MissingRange {
                            first_index: VarInt::from_u32(9),
                            count: VarInt::ZERO,
                        },
                    ],
                }),
                FeedbackRecord::Loss(LossRecord {
                    stream_id: stream(),
                    last_decoded_frame: frame(41),
                    highest_frame_seen: frame(44),
                }),
                FeedbackRecord::FrameTiming(FrameTimingRecord {
                    stream_id: stream(),
                    frame_number: frame(41),
                    first_fragment_time: Timestamp(VarInt::from_u32(1000)),
                    last_fragment_time: Timestamp(VarInt::from_u32(1400)),
                    decoded_time: Timestamp(VarInt::from_u32(2600)),
                    presented_time: Timestamp(VarInt::ZERO),
                }),
                FeedbackRecord::DisplayTiming(DisplayTimingRecord {
                    monitor_id: MonitorId(VarInt::from_u32(1)),
                    refresh_period_nanos: VarInt::from_u32(8_333_333),
                    vsync_timestamp: Timestamp(VarInt::from_u32(5000)),
                    flags: DisplayTimingFlags::VRR_ACTIVE,
                }),
                FeedbackRecord::Unknown {
                    record_type: RecordType(VarInt::from_u32(0x40)),
                    value: vec![0xff],
                },
            ],
        };
        let bytes = feedback.to_bytes();
        assert_eq!(bytes[0], 0x02);
        let mut reader = Reader::new(&bytes[1..]);
        assert_eq!(VideoFeedback::decode(&mut reader).unwrap(), feedback);
    }

    /// Bytes after the known fields of a record are skipped.
    #[test]
    fn record_trailing_bytes_skipped() {
        let bytes = [0x01, 0x03, 0x01, 0x05, 0xee, 0x01, 0x02, 0x01, 0x06];
        let mut reader = Reader::new(&bytes);
        let feedback = VideoFeedback::decode(&mut reader).unwrap();
        assert_eq!(
            feedback.records,
            [
                FeedbackRecord::Decoded(DecodedRecord {
                    stream_id: stream(),
                    last_decoded_frame: frame(5),
                }),
                FeedbackRecord::Decoded(DecodedRecord {
                    stream_id: stream(),
                    last_decoded_frame: frame(6),
                }),
            ]
        );
    }

    /// A record whose value is shorter than its fields is an error.
    #[test]
    fn short_record() {
        let mut reader = Reader::new(&[0x03, 0x02, 0x01, 0x05]);
        assert_eq!(
            VideoFeedback::decode(&mut reader),
            Err(WireError::UnexpectedEnd)
        );
    }
}
