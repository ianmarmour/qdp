//! Audio configuration and packets (Section 7).

use crate::message::{
    AudioCodec, AudioConfigId, DatagramPayload, MessagePayload, StreamId, Timestamp,
};
use crate::registry::{DatagramType, MessageType};
use crate::wire::{Reader, VarInt, WireError, Writer};

/// The largest number of channels of an audio stream (Section 7.3).
pub const MAX_AUDIO_CHANNELS: u64 = 8;

/// The largest number of channels an Opus stream carries without an Opus Mapping
/// (Section 7.3).
const MAX_CHANNELS_WITHOUT_MAPPING: u64 = 2;

/// An `AUDIO_REQUEST` message (Section 7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioRequest {
    /// The output channels the client wants, from 1 to 8, or 0 for no playback.
    pub playback_channels: VarInt,
    /// The microphone channels the client will send, or 0 for no microphone.
    pub capture_channels: VarInt,
}

impl MessagePayload for AudioRequest {
    const MESSAGE_TYPE: MessageType = MessageType::AUDIO_REQUEST;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.playback_channels);
        writer.write_varint(self.capture_channels);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            playback_channels: read_channels(reader, "Playback Channels", 0)?,
            capture_channels: read_channels(reader, "Capture Channels", 0)?,
        })
    }
}

/// Reads a channel count that must lie between `min` and [`MAX_AUDIO_CHANNELS`].
fn read_channels(
    reader: &mut Reader<'_>,
    field: &'static str,
    min: u64,
) -> Result<VarInt, WireError> {
    let channels = reader.read_varint()?;
    if (min..=MAX_AUDIO_CHANNELS).contains(&channels.into_inner()) {
        Ok(channels)
    } else {
        Err(WireError::InvalidValue { field })
    }
}

/// The direction of an audio stream (Section 7.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum AudioDirection {
    /// Host audio output to the client.
    Playback = 0x00,
    /// Client microphone to the host.
    Capture = 0x01,
}

impl TryFrom<u8> for AudioDirection {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Playback),
            0x01 => Ok(Self::Capture),
            _ => Err(WireError::InvalidValue { field: "Direction" }),
        }
    }
}

/// The multistream layout of an Opus stream with more than two channels, as the
/// channel mapping table of RFC 7845, Section 5.1.1 (Section 7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusMapping {
    /// The number of Opus streams.
    pub stream_count: u8,
    /// How many of the streams are coupled stereo streams.
    pub coupled_count: u8,
    /// For each output channel, the index of the decoded channel that feeds it.
    pub channel_mapping: Vec<u8>,
}

/// The description of one audio stream (Section 7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioStreamConfig {
    /// The stream's identifier, shared with video streams.
    pub stream_id: StreamId,
    /// Playback or capture.
    pub direction: AudioDirection,
    /// The codec.
    pub codec: AudioCodec,
    /// The sample rate in hertz.
    pub sample_rate: VarInt,
    /// The number of channels, from 1 to 8, in the order of RFC 7845 mapping
    /// family 1.
    pub channel_count: VarInt,
    /// The audio duration of each packet, in microseconds.
    pub frame_duration: VarInt,
    /// The multistream layout. Present exactly for Opus streams with more than two
    /// channels.
    pub opus_mapping: Option<OpusMapping>,
}

impl AudioStreamConfig {
    /// Returns whether a stream of this codec and channel count carries an Opus
    /// Mapping.
    fn has_opus_mapping(codec: AudioCodec, channel_count: VarInt) -> bool {
        codec == AudioCodec::OPUS && channel_count.into_inner() > MAX_CHANNELS_WITHOUT_MAPPING
    }

    /// Appends the entry, with its Entry Length, to `writer`.
    fn encode(&self, writer: &mut Writer) {
        writer.write_length_prefixed(|entry| {
            entry.write_varint(self.stream_id.0);
            entry.write_u8(self.direction as u8);
            entry.write_varint(self.codec.0);
            entry.write_varint(self.sample_rate);
            entry.write_varint(self.channel_count);
            entry.write_varint(self.frame_duration);
            if let Some(mapping) = &self.opus_mapping {
                entry.write_u8(mapping.stream_count);
                entry.write_u8(mapping.coupled_count);
                entry.write_bytes(&mapping.channel_mapping);
            }
        });
    }

    /// Reads the entry, with its Entry Length, from `reader`.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let mut entry = reader.read_length_prefixed()?;
        let stream_id = StreamId(entry.read_varint()?);
        let direction = AudioDirection::try_from(entry.read_u8()?)?;
        let codec = AudioCodec(entry.read_varint()?);
        let sample_rate = entry.read_varint()?;
        let channel_count = read_channels(&mut entry, "Channel Count", 1)?;
        let frame_duration = entry.read_varint()?;
        let opus_mapping = if Self::has_opus_mapping(codec, channel_count) {
            Some(OpusMapping {
                stream_count: entry.read_u8()?,
                coupled_count: entry.read_u8()?,
                // The channel count was checked to be at most 8 above.
                channel_mapping: entry
                    .read_bytes(channel_count.into_inner() as usize)?
                    .to_vec(),
            })
        } else {
            None
        };
        Ok(Self {
            stream_id,
            direction,
            codec,
            sample_rate,
            channel_count,
            frame_duration,
            opus_mapping,
        })
    }
}

/// An `AUDIO_CONFIG` message (Section 7.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioConfig {
    /// Numbered from 1, increasing with each configuration.
    pub config_id: AudioConfigId,
    /// Every audio stream of the session, in both directions.
    pub streams: Vec<AudioStreamConfig>,
}

impl MessagePayload for AudioConfig {
    const MESSAGE_TYPE: MessageType = MessageType::AUDIO_CONFIG;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.config_id.0);
        writer.write_count(self.streams.len());
        for stream in &self.streams {
            stream.encode(writer);
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let config_id = AudioConfigId(reader.read_varint()?);
        let count = reader.read_count("Stream Count", u64::MAX)?;
        let streams = (0..count)
            .map(|_| AudioStreamConfig::decode(reader))
            .collect::<Result<_, _>>()?;
        Ok(Self { config_id, streams })
    }
}

/// An `AUDIO_PACKET` datagram, borrowing its payload (Section 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPacket<'a> {
    /// The audio stream.
    pub stream_id: StreamId,
    /// Numbered from 1, increasing with each packet of the stream.
    pub sequence_number: VarInt,
    /// The index, per channel, of the packet's first sample since the stream began.
    pub sample_position: VarInt,
    /// When the first sample was captured, on the sender's clock.
    pub capture_timestamp: Timestamp,
    /// One encoded frame.
    pub payload: &'a [u8],
}

impl<'a> DatagramPayload<'a> for AudioPacket<'a> {
    const DATAGRAM_TYPE: DatagramType = DatagramType::AUDIO_PACKET;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.stream_id.0);
        writer.write_varint(self.sequence_number);
        writer.write_varint(self.sample_position);
        self.capture_timestamp.write(writer);
        writer.write_bytes(self.payload);
    }

    fn decode(reader: &mut Reader<'a>) -> Result<Self, WireError> {
        Ok(Self {
            stream_id: StreamId(reader.read_varint()?),
            sequence_number: reader.read_varint()?,
            sample_position: reader.read_varint()?,
            capture_timestamp: Timestamp::read(reader)?,
            payload: reader.read_rest(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes `message` and decodes it again.
    fn round_trip<M: MessagePayload>(message: &M) -> Result<M, WireError> {
        let frame = message.to_frame();
        M::decode(&mut Reader::new(&frame.payload))
    }

    /// Returns a playback stream of `channels` channels of `codec`.
    fn stream(
        codec: AudioCodec,
        channels: u32,
        opus_mapping: Option<OpusMapping>,
    ) -> AudioStreamConfig {
        AudioStreamConfig {
            stream_id: StreamId(VarInt::from_u32(5)),
            direction: AudioDirection::Playback,
            codec,
            sample_rate: VarInt::from_u32(48_000),
            channel_count: VarInt::from_u32(channels),
            frame_duration: VarInt::from_u32(5_000),
            opus_mapping,
        }
    }

    /// Audio requests round-trip, and channel counts above 8 are errors.
    #[test]
    fn audio_request() {
        let request = AudioRequest {
            playback_channels: VarInt::from_u32(2),
            capture_channels: VarInt::from_u32(1),
        };
        assert_eq!(round_trip(&request).unwrap(), request);

        let too_many = AudioRequest {
            playback_channels: VarInt::from_u32(9),
            capture_channels: VarInt::ZERO,
        };
        assert_eq!(
            round_trip(&too_many),
            Err(WireError::InvalidValue {
                field: "Playback Channels"
            })
        );
    }

    /// Configurations with and without an Opus Mapping round-trip.
    #[test]
    fn audio_config_round_trip() {
        let config = AudioConfig {
            config_id: AudioConfigId(VarInt::from_u32(1)),
            streams: vec![
                stream(AudioCodec::OPUS, 2, None),
                stream(
                    AudioCodec::OPUS,
                    6,
                    Some(OpusMapping {
                        stream_count: 4,
                        coupled_count: 2,
                        channel_mapping: vec![0, 4, 1, 2, 3, 5],
                    }),
                ),
                stream(AudioCodec::PCM_L24, 8, None),
                AudioStreamConfig {
                    direction: AudioDirection::Capture,
                    ..stream(AudioCodec::OPUS, 1, None)
                },
            ],
        };
        assert_eq!(round_trip(&config).unwrap(), config);
    }

    /// An Opus Mapping with fewer entries than channels is an error.
    #[test]
    fn short_opus_mapping() {
        let config = AudioConfig {
            config_id: AudioConfigId(VarInt::from_u32(1)),
            streams: vec![stream(
                AudioCodec::OPUS,
                6,
                Some(OpusMapping {
                    stream_count: 4,
                    coupled_count: 2,
                    channel_mapping: vec![0, 4, 1],
                }),
            )],
        };
        assert_eq!(round_trip(&config), Err(WireError::UnexpectedEnd));
    }

    /// An audio packet round-trips and borrows its payload.
    #[test]
    fn audio_packet_round_trip() {
        let packet = AudioPacket {
            stream_id: StreamId(VarInt::from_u32(5)),
            sequence_number: VarInt::from_u32(1),
            sample_position: VarInt::ZERO,
            capture_timestamp: Timestamp(VarInt::from_u32(20_000)),
            payload: &[0xfc, 0xff, 0xfe],
        };
        let bytes = packet.to_bytes();
        assert_eq!(bytes[0], 0x03);
        let mut reader = Reader::new(&bytes[1..]);
        assert_eq!(AudioPacket::decode(&mut reader).unwrap(), packet);
    }
}
