//! Display configuration, video fragments and frame payloads (Section 6).

use crate::message::{
    BlockGrid, BlockSet, ClientMonitorId, DatagramPayload, DisplayConfigId, DisplayRequestId,
    FrameNumber, MessagePayload, MonitorId, StreamId, Timestamp, VideoCodec,
};
use crate::registry::{DatagramType, MessageType};
use crate::wire::{Reader, SignedVarInt, VarInt, WireError, Writer};

/// The largest frame payload, 64 MiB (Section 6.5).
pub const MAX_FRAME_PAYLOAD_LENGTH: usize = 64 << 20;

/// The largest number of fragments in one frame (Section 6.6.2).
pub const MAX_FRAGMENTS_PER_FRAME: u64 = 65535;

/// The flags of a client display area (Section 6.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ClientMonitorFlags(pub u8);

impl ClientMonitorFlags {
    /// The client's primary display.
    pub const PRIMARY: Self = Self(0x01);
    /// The display supports variable refresh rates.
    pub const VRR_CAPABLE: Self = Self(0x02);
    /// The display can currently present BT.2020 with PQ.
    pub const HDR_PQ: Self = Self(0x04);
    /// The display can currently present BT.2020 with HLG.
    pub const HDR_HLG: Self = Self(0x08);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A display area of the client (Section 6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientMonitor {
    /// The client's identifier for this display area.
    pub id: ClientMonitorId,
    /// Primary, variable refresh and HDR flags.
    pub flags: ClientMonitorFlags,
    /// Horizontal position in the client's desktop, in physical pixels.
    pub x: SignedVarInt,
    /// Vertical position in the client's desktop, in physical pixels.
    pub y: SignedVarInt,
    /// Width of the area, in physical pixels.
    pub width: VarInt,
    /// Height of the area, in physical pixels.
    pub height: VarInt,
    /// Refresh rate in millihertz, or 0 if unknown.
    pub refresh_rate: VarInt,
    /// Interface scale in percent.
    pub scale: VarInt,
    /// Physical width in millimetres, or 0 if unknown.
    pub physical_width: VarInt,
    /// Physical height in millimetres, or 0 if unknown.
    pub physical_height: VarInt,
    /// Peak luminance in candela per square metre, or 0 if unknown.
    pub peak_luminance: VarInt,
}

impl ClientMonitor {
    /// Appends the entry, with its Entry Length, to `writer`.
    fn encode(&self, writer: &mut Writer) {
        writer.write_length_prefixed(|entry| {
            entry.write_varint(self.id.0);
            entry.write_u8(self.flags.0);
            entry.write_signed_varint(self.x);
            entry.write_signed_varint(self.y);
            entry.write_varint(self.width);
            entry.write_varint(self.height);
            entry.write_varint(self.refresh_rate);
            entry.write_varint(self.scale);
            entry.write_varint(self.physical_width);
            entry.write_varint(self.physical_height);
            entry.write_varint(self.peak_luminance);
        });
    }

    /// Reads the entry, with its Entry Length, from `reader`.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let mut entry = reader.read_length_prefixed()?;
        Ok(Self {
            id: ClientMonitorId(entry.read_varint()?),
            flags: ClientMonitorFlags(entry.read_u8()?),
            x: entry.read_signed_varint()?,
            y: entry.read_signed_varint()?,
            width: entry.read_varint()?,
            height: entry.read_varint()?,
            refresh_rate: entry.read_varint()?,
            scale: entry.read_varint()?,
            physical_width: entry.read_varint()?,
            physical_height: entry.read_varint()?,
            peak_luminance: entry.read_varint()?,
        })
    }
}

/// A `DISPLAY_REQUEST` message (Section 6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayRequest {
    /// Numbered from 1, increasing with each request.
    pub request_id: DisplayRequestId,
    /// The client's display areas, at least one.
    pub monitors: Vec<ClientMonitor>,
}

impl MessagePayload for DisplayRequest {
    const MESSAGE_TYPE: MessageType = MessageType::DISPLAY_REQUEST;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.request_id.0);
        writer.write_count(self.monitors.len());
        for monitor in &self.monitors {
            monitor.encode(writer);
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let request_id = DisplayRequestId(reader.read_varint()?);
        let count = reader.read_count("Monitor Count", u64::MAX)?;
        if count == 0 {
            return Err(WireError::InvalidValue {
                field: "Monitor Count",
            });
        }
        let monitors = (0..count)
            .map(|_| ClientMonitor::decode(reader))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            request_id,
            monitors,
        })
    }
}

/// The flags of a host monitor (Section 6.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct HostMonitorFlags(pub u8);

impl HostMonitorFlags {
    /// The host's primary monitor.
    pub const PRIMARY: Self = Self(0x01);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A monitor the host streams, placed in the host desktop (Sections 1.5.6 and 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostMonitor {
    /// The host's identifier for this monitor.
    pub id: MonitorId,
    /// The client display area this monitor is meant for, or
    /// [`ClientMonitorId::NONE`].
    pub client_monitor_id: ClientMonitorId,
    /// Primary flag.
    pub flags: HostMonitorFlags,
    /// Left edge in the host desktop, in pixels of the monitor's video.
    pub x: SignedVarInt,
    /// Top edge in the host desktop, in pixels of the monitor's video.
    pub y: SignedVarInt,
    /// Width in pixels, from 16 to 16384.
    pub width: VarInt,
    /// Height in pixels, from 16 to 16384.
    pub height: VarInt,
    /// Refresh rate in millihertz.
    pub refresh_rate: VarInt,
    /// Interface scale in percent.
    pub scale: VarInt,
}

impl HostMonitor {
    /// Appends the entry, with its Entry Length, to `writer`.
    fn encode(&self, writer: &mut Writer) {
        writer.write_length_prefixed(|entry| {
            entry.write_varint(self.id.0);
            entry.write_varint(self.client_monitor_id.0);
            entry.write_u8(self.flags.0);
            entry.write_signed_varint(self.x);
            entry.write_signed_varint(self.y);
            entry.write_varint(self.width);
            entry.write_varint(self.height);
            entry.write_varint(self.refresh_rate);
            entry.write_varint(self.scale);
        });
    }

    /// Reads the entry, with its Entry Length, from `reader`.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let mut entry = reader.read_length_prefixed()?;
        Ok(Self {
            id: MonitorId(entry.read_varint()?),
            client_monitor_id: ClientMonitorId(entry.read_varint()?),
            flags: HostMonitorFlags(entry.read_u8()?),
            x: entry.read_signed_varint()?,
            y: entry.read_signed_varint()?,
            width: entry.read_varint()?,
            height: entry.read_varint()?,
            refresh_rate: entry.read_varint()?,
            scale: entry.read_varint()?,
        })
    }

    /// Returns the grid of composition blocks that divides this monitor.
    ///
    /// Sizes above `u32::MAX` are clamped; valid monitors are at most 16384 pixels
    /// on each side.
    pub fn block_grid(&self) -> BlockGrid {
        let clamp = |value: VarInt| u32::try_from(value.into_inner()).unwrap_or(u32::MAX);
        BlockGrid::for_monitor(clamp(self.width), clamp(self.height))
    }
}

/// The layer a video stream carries (Section 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum VideoLayer {
    /// Every change, with the lowest latency.
    Motion = 0x00,
    /// Blocks that stopped changing, at higher fidelity.
    Refinement = 0x01,
}

impl TryFrom<u8> for VideoLayer {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Motion),
            0x01 => Ok(Self::Refinement),
            _ => Err(WireError::InvalidValue { field: "Layer" }),
        }
    }
}

/// The chroma subsampling of a video stream (Section 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Chroma {
    /// 4:2:0 subsampling.
    Yuv420 = 0x00,
    /// No subsampling.
    Yuv444 = 0x01,
}

impl TryFrom<u8> for Chroma {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Yuv420),
            0x01 => Ok(Self::Yuv444),
            _ => Err(WireError::InvalidValue { field: "Chroma" }),
        }
    }
}

/// The flags of a video stream (Section 6.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct VideoStreamFlags(pub u8);

impl VideoStreamFlags {
    /// Full color range.
    pub const FULL_RANGE: Self = Self(0x01);
    /// Every frame is coded losslessly. Refinement layer only.
    pub const LOSSLESS: Self = Self(0x02);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The description of one video stream (Section 6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoStreamConfig {
    /// The stream's identifier, shared with audio streams.
    pub stream_id: StreamId,
    /// The monitor whose content the stream carries.
    pub monitor_id: MonitorId,
    /// Motion or refinement.
    pub layer: VideoLayer,
    /// The codec, AV1.
    pub codec: VideoCodec,
    /// The AV1 `seq_profile`.
    pub profile: u8,
    /// The bit depth, 8 or 10.
    pub bit_depth: u8,
    /// The chroma subsampling.
    pub chroma: Chroma,
    /// Range and lossless flags.
    pub flags: VideoStreamFlags,
    /// The H.273 colour primaries code point.
    pub color_primaries: u8,
    /// The H.273 transfer characteristics code point.
    pub transfer_characteristics: u8,
    /// The H.273 matrix coefficients code point.
    pub matrix_coefficients: u8,
}

impl VideoStreamConfig {
    /// Appends the entry, with its Entry Length, to `writer`.
    fn encode(&self, writer: &mut Writer) {
        writer.write_length_prefixed(|entry| {
            entry.write_varint(self.stream_id.0);
            entry.write_varint(self.monitor_id.0);
            entry.write_u8(self.layer as u8);
            entry.write_varint(self.codec.0);
            entry.write_u8(self.profile);
            entry.write_u8(self.bit_depth);
            entry.write_u8(self.chroma as u8);
            entry.write_u8(self.flags.0);
            entry.write_u8(self.color_primaries);
            entry.write_u8(self.transfer_characteristics);
            entry.write_u8(self.matrix_coefficients);
        });
    }

    /// Reads the entry, with its Entry Length, from `reader`.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let mut entry = reader.read_length_prefixed()?;
        Ok(Self {
            stream_id: StreamId(entry.read_varint()?),
            monitor_id: MonitorId(entry.read_varint()?),
            layer: VideoLayer::try_from(entry.read_u8()?)?,
            codec: VideoCodec(entry.read_varint()?),
            profile: entry.read_u8()?,
            bit_depth: entry.read_u8()?,
            chroma: Chroma::try_from(entry.read_u8()?)?,
            flags: VideoStreamFlags(entry.read_u8()?),
            color_primaries: entry.read_u8()?,
            transfer_characteristics: entry.read_u8()?,
            matrix_coefficients: entry.read_u8()?,
        })
    }
}

/// A `DISPLAY_CONFIG` message (Section 6.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayConfig {
    /// Numbered from 1, increasing with each configuration.
    pub config_id: DisplayConfigId,
    /// The highest request this configuration answers, or
    /// [`DisplayRequestId::NONE`].
    pub request_id: DisplayRequestId,
    /// The monitors the host streams.
    pub monitors: Vec<HostMonitor>,
    /// The video streams of those monitors.
    pub streams: Vec<VideoStreamConfig>,
}

impl MessagePayload for DisplayConfig {
    const MESSAGE_TYPE: MessageType = MessageType::DISPLAY_CONFIG;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.config_id.0);
        writer.write_varint(self.request_id.0);
        writer.write_count(self.monitors.len());
        for monitor in &self.monitors {
            monitor.encode(writer);
        }
        writer.write_count(self.streams.len());
        for stream in &self.streams {
            stream.encode(writer);
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let config_id = DisplayConfigId(reader.read_varint()?);
        let request_id = DisplayRequestId(reader.read_varint()?);
        let monitor_count = reader.read_count("Monitor Count", u64::MAX)?;
        let monitors = (0..monitor_count)
            .map(|_| HostMonitor::decode(reader))
            .collect::<Result<_, _>>()?;
        let stream_count = reader.read_count("Stream Count", u64::MAX)?;
        let streams = (0..stream_count)
            .map(|_| VideoStreamConfig::decode(reader))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            config_id,
            request_id,
            monitors,
            streams,
        })
    }
}

/// A `KEYFRAME_REQUEST` message (Section 6.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyframeRequest {
    /// The stream whose decoder failed or was reset.
    pub stream_id: StreamId,
}

impl MessagePayload for KeyframeRequest {
    const MESSAGE_TYPE: MessageType = MessageType::KEYFRAME_REQUEST;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.stream_id.0);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            stream_id: StreamId(reader.read_varint()?),
        })
    }
}

/// The flags of a video fragment (Section 6.6.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct FragmentFlags(pub u8);

impl FragmentFlags {
    /// The temporal unit begins with a sequence header and a key frame.
    pub const KEY_FRAME: Self = Self(0x01);
    /// The frame ends a loss state.
    pub const RECOVERY: Self = Self(0x02);
    /// This datagram repeats an earlier one.
    pub const RETRANSMISSION: Self = Self(0x04);
    /// This is the frame's last fragment.
    pub const LAST: Self = Self(0x08);

    /// The flags that must be identical on every fragment of a frame.
    pub const FRAME_FLAGS: Self = Self(Self::KEY_FRAME.0 | Self::RECOVERY.0);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the flags in both `self` and `other`.
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    /// Returns the flags in either `self` or `other`.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Returns whether a frame with these flags carries a Recovery Base
    /// (Section 6.5).
    pub const fn has_recovery_base(self) -> bool {
        self.contains(Self::RECOVERY) && !self.contains(Self::KEY_FRAME)
    }
}

/// A `VIDEO_FRAGMENT` datagram, borrowing its data (Section 6.6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFragment<'a> {
    /// The video stream.
    pub stream_id: StreamId,
    /// The frame this fragment belongs to.
    pub frame_number: FrameNumber,
    /// The position of this fragment within the frame, from 0.
    pub fragment_index: VarInt,
    /// Key frame, recovery, retransmission and last flags.
    pub flags: FragmentFlags,
    /// The fragment's piece of the frame payload.
    pub data: &'a [u8],
}

impl<'a> DatagramPayload<'a> for VideoFragment<'a> {
    const DATAGRAM_TYPE: DatagramType = DatagramType::VIDEO_FRAGMENT;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.stream_id.0);
        writer.write_varint(self.frame_number.0);
        writer.write_varint(self.fragment_index);
        writer.write_u8(self.flags.0);
        writer.write_bytes(self.data);
    }

    fn decode(reader: &mut Reader<'a>) -> Result<Self, WireError> {
        Ok(Self {
            stream_id: StreamId(reader.read_varint()?),
            frame_number: FrameNumber(reader.read_varint()?),
            fragment_index: reader.read_varint()?,
            flags: FragmentFlags(reader.read_u8()?),
            data: reader.read_rest(),
        })
    }
}

/// The header that precedes the temporal unit in a frame payload (Section 6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameHeader {
    /// When the source image was captured, on the host's clock.
    pub capture_timestamp: Timestamp,
    /// The last frame the client is assumed to have decoded. Present exactly when
    /// the frame is flagged `RECOVERY` and not `KEY_FRAME`.
    pub recovery_base: Option<FrameNumber>,
    /// The motion frame whose source this refinement frame reproduces. Present
    /// exactly in refinement streams.
    pub base_frame_number: Option<FrameNumber>,
    /// The changed blocks of a motion frame or the covered blocks of a refinement
    /// frame.
    pub blocks: BlockSet,
}

impl FrameHeader {
    /// Appends the header, with its Header Length, to `writer`.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_length_prefixed(|header| {
            self.capture_timestamp.write(header);
            if let Some(recovery_base) = self.recovery_base {
                header.write_varint(recovery_base.0);
            }
            if let Some(base_frame_number) = self.base_frame_number {
                header.write_varint(base_frame_number.0);
            }
            self.blocks.encode(header);
        });
    }

    /// Reads the header, with its Header Length, of a frame of a stream of `layer`
    /// whose fragments carry `flags`, for a monitor divided into `grid`.
    pub fn decode(
        reader: &mut Reader<'_>,
        layer: VideoLayer,
        flags: FragmentFlags,
        grid: BlockGrid,
    ) -> Result<Self, WireError> {
        let mut header = reader.read_length_prefixed()?;
        let capture_timestamp = Timestamp::read(&mut header)?;
        let recovery_base = if flags.has_recovery_base() {
            Some(FrameNumber(header.read_varint()?))
        } else {
            None
        };
        let base_frame_number = match layer {
            VideoLayer::Refinement => Some(FrameNumber(header.read_varint()?)),
            VideoLayer::Motion => None,
        };
        Ok(Self {
            capture_timestamp,
            recovery_base,
            base_frame_number,
            blocks: BlockSet::decode(&mut header, grid.len())?,
        })
    }
}

/// A reassembled frame: its header and its AV1 temporal unit (Section 6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePayload<'a> {
    /// The frame header.
    pub header: FrameHeader,
    /// The AV1 temporal unit, in the low overhead bitstream format.
    pub temporal_unit: &'a [u8],
}

impl<'a> FramePayload<'a> {
    /// Returns the encoded frame payload, which the host splits into fragments.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(self.temporal_unit.len() + 64);
        self.header.encode(&mut writer);
        writer.write_bytes(self.temporal_unit);
        writer.into_bytes()
    }

    /// Decodes a reassembled frame payload; the arguments are as for
    /// [`FrameHeader::decode`].
    pub fn decode(
        bytes: &'a [u8],
        layer: VideoLayer,
        flags: FragmentFlags,
        grid: BlockGrid,
    ) -> Result<Self, WireError> {
        let mut reader = Reader::new(bytes);
        let header = FrameHeader::decode(&mut reader, layer, flags, grid)?;
        Ok(Self {
            header,
            temporal_unit: reader.read_rest(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes `message` and decodes it again.
    fn round_trip<M: MessagePayload>(message: &M) -> M {
        let frame = message.to_frame();
        M::decode(&mut Reader::new(&frame.payload)).unwrap()
    }

    /// Returns a 1920 × 1080 host monitor at the origin.
    fn host_monitor() -> HostMonitor {
        HostMonitor {
            id: MonitorId(VarInt::from_u32(1)),
            client_monitor_id: ClientMonitorId(VarInt::from_u32(1)),
            flags: HostMonitorFlags::PRIMARY,
            x: SignedVarInt::ZERO,
            y: SignedVarInt::from(-1080),
            width: VarInt::from_u32(1920),
            height: VarInt::from_u32(1080),
            refresh_rate: VarInt::from_u32(120_000),
            scale: VarInt::from_u32(100),
        }
    }

    /// Display messages round-trip.
    #[test]
    fn display_messages_round_trip() {
        let request = DisplayRequest {
            request_id: DisplayRequestId(VarInt::from_u32(1)),
            monitors: vec![ClientMonitor {
                id: ClientMonitorId(VarInt::from_u32(1)),
                flags: ClientMonitorFlags::PRIMARY,
                x: SignedVarInt::from(-2560),
                y: SignedVarInt::ZERO,
                width: VarInt::from_u32(2560),
                height: VarInt::from_u32(1440),
                refresh_rate: VarInt::from_u32(59_940),
                scale: VarInt::from_u32(125),
                physical_width: VarInt::from_u32(597),
                physical_height: VarInt::from_u32(336),
                peak_luminance: VarInt::ZERO,
            }],
        };
        assert_eq!(round_trip(&request), request);

        let config = DisplayConfig {
            config_id: DisplayConfigId(VarInt::from_u32(1)),
            request_id: request.request_id,
            monitors: vec![host_monitor()],
            streams: vec![VideoStreamConfig {
                stream_id: StreamId(VarInt::from_u32(1)),
                monitor_id: MonitorId(VarInt::from_u32(1)),
                layer: VideoLayer::Motion,
                codec: VideoCodec::AV1,
                profile: 0,
                bit_depth: 8,
                chroma: Chroma::Yuv420,
                flags: VideoStreamFlags::default(),
                color_primaries: 1,
                transfer_characteristics: 13,
                matrix_coefficients: 1,
            }],
        };
        assert_eq!(round_trip(&config), config);

        let keyframe = KeyframeRequest {
            stream_id: StreamId(VarInt::from_u32(3)),
        };
        assert_eq!(round_trip(&keyframe), keyframe);
    }

    /// Bytes added to an entry by a later version are skipped.
    #[test]
    fn entry_trailing_bytes_skipped() {
        let mut original = Writer::new();
        host_monitor().encode(&mut original);
        let fields = Reader::new(original.as_bytes()).read_octets().unwrap();

        let mut extended = Writer::new();
        extended.write_length_prefixed(|entry| {
            entry.write_bytes(fields);
            entry.write_bytes(&[0xaa, 0xbb]);
        });
        let mut reader = Reader::new(extended.as_bytes());
        assert_eq!(HostMonitor::decode(&mut reader).unwrap(), host_monitor());
        assert!(reader.is_empty());
    }

    /// A `DISPLAY_REQUEST` without monitors is an error.
    #[test]
    fn display_request_without_monitors() {
        let request = DisplayRequest {
            request_id: DisplayRequestId(VarInt::from_u32(1)),
            monitors: Vec::new(),
        };
        let frame = request.to_frame();
        assert!(matches!(
            DisplayRequest::decode(&mut Reader::new(&frame.payload)),
            Err(WireError::InvalidValue { .. })
        ));
    }

    /// A video fragment round-trips and borrows its data.
    #[test]
    fn video_fragment_round_trip() {
        let fragment = VideoFragment {
            stream_id: StreamId(VarInt::from_u32(1)),
            frame_number: FrameNumber(VarInt::from_u32(42)),
            fragment_index: VarInt::from_u32(3),
            flags: FragmentFlags::KEY_FRAME.union(FragmentFlags::LAST),
            data: &[1, 2, 3, 4],
        };
        let bytes = fragment.to_bytes();
        assert_eq!(bytes, [0x01, 0x01, 0x2a, 0x03, 0x09, 1, 2, 3, 4]);
        let mut reader = Reader::new(&bytes[1..]);
        assert_eq!(VideoFragment::decode(&mut reader).unwrap(), fragment);
    }

    /// The optional header fields appear according to the flags and the layer.
    #[test]
    fn frame_payload_optional_fields() {
        let grid = host_monitor().block_grid();
        let mut blocks = BlockSet::empty(grid.len());
        blocks.insert(grid.index(3, 2));

        let cases = [
            (VideoLayer::Motion, FragmentFlags::KEY_FRAME, None, None),
            (
                VideoLayer::Motion,
                FragmentFlags::RECOVERY,
                Some(FrameNumber(VarInt::from_u32(9))),
                None,
            ),
            (
                VideoLayer::Refinement,
                FragmentFlags::RECOVERY.union(FragmentFlags::KEY_FRAME),
                None,
                Some(FrameNumber(VarInt::from_u32(12))),
            ),
        ];
        for (layer, flags, recovery_base, base_frame_number) in cases {
            let payload = FramePayload {
                header: FrameHeader {
                    capture_timestamp: Timestamp(VarInt::from_u32(16_667)),
                    recovery_base,
                    base_frame_number,
                    blocks: blocks.clone(),
                },
                temporal_unit: &[0x12, 0x00, 0x0a, 0x0b],
            };
            let bytes = payload.to_bytes();
            assert_eq!(
                FramePayload::decode(&bytes, layer, flags, grid).unwrap(),
                payload
            );
        }
    }

    /// The Recovery Base is present only for recovery frames that are not key
    /// frames.
    #[test]
    fn recovery_base_presence() {
        assert!(FragmentFlags::RECOVERY.has_recovery_base());
        assert!(!FragmentFlags::KEY_FRAME.has_recovery_base());
        assert!(
            !FragmentFlags::RECOVERY
                .union(FragmentFlags::KEY_FRAME)
                .has_recovery_base()
        );
    }
}
