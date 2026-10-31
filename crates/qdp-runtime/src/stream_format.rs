//! The client's check that each video stream is coded as its Video Stream
//! Config says: every key frame begins with a sequence header that matches
//! the config, stays the same for the whole stream and does not exceed the
//! client's Max Level (Sections 6.3 and 6.4.1).

use qdp::message::{Chroma, NO_LEVEL_LIMIT, VideoStreamConfig, VideoStreamFlags};
use qdp_codec::{ColorDescription, SequenceHeader};

use crate::RuntimeError;

/// A stream's expected format, and the sequence header it is coded with.
#[derive(Debug, Clone)]
pub(crate) struct StreamFormat {
    /// The stream's Video Stream Config.
    config: VideoStreamConfig,
    /// The client's Max Level, or [`NO_LEVEL_LIMIT`].
    max_level: u8,
    /// The sequence header of the stream's first key frame.
    header: Option<SequenceHeader>,
}

impl StreamFormat {
    /// Returns the format a stream configured by `config` must have, for a
    /// client whose Max Level is `max_level`.
    pub(crate) fn new(config: VideoStreamConfig, max_level: u8) -> Self {
        Self {
            config,
            max_level,
            header: None,
        }
    }

    /// Checks the temporal unit of a frame flagged `KEY_FRAME`, which begins
    /// with a sequence header (Section 6.4.1, rule 7).
    pub(crate) fn check_key_frame(&mut self, temporal_unit: &[u8]) -> Result<(), RuntimeError> {
        let header = SequenceHeader::find(temporal_unit)
            .map_err(|error| violation(format!("the sequence header is malformed: {error}")))?
            .ok_or_else(|| violation("a key frame has no sequence header".to_owned()))?;
        if let Some(first) = &self.header {
            if first.obu() != header.obu() {
                return Err(violation(
                    "the sequence header changed within the stream".to_owned(),
                ));
            }
            return Ok(());
        }
        if let Some(field) = mismatch(&self.config, &header) {
            return Err(violation(format!(
                "the sequence header's {field} does not match the stream's config"
            )));
        }
        if self.max_level != NO_LEVEL_LIMIT && header.level > self.max_level {
            return Err(violation(format!(
                "the stream's level {} exceeds the client's {}",
                header.level, self.max_level
            )));
        }
        self.header = Some(header);
        Ok(())
    }
}

/// Returns a protocol violation about a stream's format.
fn violation(reason: String) -> RuntimeError {
    RuntimeError::ProtocolViolation(reason)
}

/// Returns the first part of `config` that `header` contradicts, or that
/// `header` uses although no stream may (Section 6.4.1, rules 5 and 6).
fn mismatch(config: &VideoStreamConfig, header: &SequenceHeader) -> Option<&'static str> {
    let chroma = match (
        header.monochrome,
        header.subsampling_x,
        header.subsampling_y,
    ) {
        (false, true, true) => Some(Chroma::Yuv420),
        (false, false, false) => Some(Chroma::Yuv444),
        _ => None,
    };
    let description = ColorDescription {
        primaries: config.color_primaries,
        transfer: config.transfer_characteristics,
        matrix: config.matrix_coefficients,
    };
    let full_range = config.flags.contains(VideoStreamFlags::FULL_RANGE);
    [
        ("profile", header.profile == config.profile),
        ("bit depth", header.bit_depth == config.bit_depth),
        ("chroma subsampling", chroma == Some(config.chroma)),
        ("color range", header.full_range == full_range),
        (
            "color description",
            header.color_description == Some(description),
        ),
        ("super-resolution", !header.superres),
        ("film grain", !header.film_grain),
    ]
    .into_iter()
    .find(|(_, matches)| !matches)
    .map(|(field, _)| field)
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::{MonitorId, StreamId, VideoCodec, VideoLayer};
    use qdp::wire::VarInt;

    /// A 1080p sequence header: profile 0, 8 bits, 4:2:0, BT.709 primaries,
    /// sRGB transfer, BT.709 matrix and limited range.
    const SEQUENCE_HEADER: [u8; 16] = [
        0x0a, 0x0e, 0x00, 0x00, 0x00, 0x4a, 0xab, 0xbf, 0xc3, 0x70, 0x08, 0x74, 0x40, 0x43, 0x40,
        0x41,
    ];

    /// A frame OBU to follow the sequence header.
    const FRAME: [u8; 3] = [0x32, 0x01, 0x00];

    /// Returns the config that describes [`SEQUENCE_HEADER`].
    fn config() -> VideoStreamConfig {
        VideoStreamConfig {
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
        }
    }

    /// Returns a key frame's temporal unit.
    fn key_frame() -> Vec<u8> {
        [SEQUENCE_HEADER.as_slice(), &FRAME].concat()
    }

    /// A stream coded as configured passes, again and again.
    #[test]
    fn accepts_a_matching_stream() {
        let mut format = StreamFormat::new(config(), NO_LEVEL_LIMIT);
        format.check_key_frame(&key_frame()).unwrap();
        format.check_key_frame(&key_frame()).unwrap();
    }

    /// Every field of the config must match the sequence header.
    #[test]
    fn rejects_mismatched_fields() {
        let changes: [fn(&mut VideoStreamConfig); 5] = [
            |config| config.bit_depth = 10,
            |config| config.chroma = Chroma::Yuv444,
            |config| config.flags = VideoStreamFlags::FULL_RANGE,
            |config| config.transfer_characteristics = 16,
            |config| config.matrix_coefficients = 9,
        ];
        for change in changes {
            let mut config = config();
            change(&mut config);
            let mut format = StreamFormat::new(config, NO_LEVEL_LIMIT);
            assert!(matches!(
                format.check_key_frame(&key_frame()),
                Err(RuntimeError::ProtocolViolation(_))
            ));
        }
    }

    /// Key frames must carry the stream's one sequence header, at a level
    /// the client supports.
    #[test]
    fn rejects_missing_changed_and_excessive_headers() {
        let mut format = StreamFormat::new(config(), NO_LEVEL_LIMIT);
        assert!(format.check_key_frame(&FRAME).is_err());
        format.check_key_frame(&key_frame()).unwrap();
        let mut changed = key_frame();
        changed[5] ^= 0x01;
        assert!(format.check_key_frame(&changed).is_err());
        // The header signals level 9 (4.1).
        let mut limited = StreamFormat::new(config(), 8);
        assert!(limited.check_key_frame(&key_frame()).is_err());
        let mut enough = StreamFormat::new(config(), 9);
        enough.check_key_frame(&key_frame()).unwrap();
    }
}
