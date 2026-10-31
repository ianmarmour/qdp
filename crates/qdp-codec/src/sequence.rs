//! Reading AV1 sequence headers (AV1 specification, Section 5.5), for decoders
//! that must be told a stream's format before its first frame, and building
//! the AV1 codec configuration record (`av1C`) such decoders take.

use crate::{CodecError, ObuType, parse_obus};

/// The first byte of an `av1C` record: `marker` 1 and `version` 1.
const AV1C_MARKER_AND_VERSION: u8 = 0x81;

/// The `seq_level_idx` above which a tier bit follows.
const MAX_LEVEL_WITHOUT_TIER: u64 = 7;

/// `seq_force_screen_content_tools` meaning "chosen per frame".
const SELECT_SCREEN_CONTENT_TOOLS: u64 = 2;

/// `color_primaries` of BT.709.
const PRIMARIES_BT709: u64 = 1;

/// `transfer_characteristics` of sRGB.
const TRANSFER_SRGB: u64 = 13;

/// `matrix_coefficients` of identity (RGB).
const MATRIX_IDENTITY: u64 = 0;

/// The values used when no colour description is present: unspecified.
const COLOR_UNSPECIFIED: u64 = 2;

/// Reads values most significant bit first, as AV1's `f(n)` descriptor does.
struct BitReader<'a> {
    /// The bytes being read.
    bytes: &'a [u8],
    /// The number of bits read.
    position: usize,
}

impl<'a> BitReader<'a> {
    /// Starts reading at the first bit of `bytes`.
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    /// Reads one bit.
    fn bit(&mut self) -> Result<bool, CodecError> {
        let byte = self
            .bytes
            .get(self.position / 8)
            .ok_or(CodecError::MalformedBitstream("truncated sequence header"))?;
        let set = byte & (0x80 >> (self.position % 8)) != 0;
        self.position += 1;
        Ok(set)
    }

    /// Reads `count` bits: AV1's `f(count)`.
    fn bits(&mut self, count: u32) -> Result<u64, CodecError> {
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | u64::from(self.bit()?);
        }
        Ok(value)
    }

    /// Reads a variable-length unsigned value: AV1's `uvlc()`.
    fn uvlc(&mut self) -> Result<u64, CodecError> {
        let mut leading_zeros = 0;
        while !self.bit()? {
            leading_zeros += 1;
            if leading_zeros >= 32 {
                return Ok(u64::from(u32::MAX));
            }
        }
        Ok(self.bits(leading_zeros)? + (1 << leading_zeros) - 1)
    }
}

/// The parts of an AV1 sequence header that describe a stream's format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequenceHeader {
    /// `seq_profile`.
    pub profile: u8,
    /// `seq_level_idx` of the first operating point.
    pub level: u8,
    /// `seq_tier` of the first operating point.
    pub tier: u8,
    /// The bit depth: 8, 10 or 12.
    pub bit_depth: u8,
    /// Whether the stream has no chroma.
    pub monochrome: bool,
    /// Whether chroma is subsampled horizontally.
    pub subsampling_x: bool,
    /// Whether chroma is subsampled vertically.
    pub subsampling_y: bool,
    /// `chroma_sample_position`.
    pub chroma_sample_position: u8,
    /// The widest frame, in luma samples.
    pub max_width: u32,
    /// The tallest frame, in luma samples.
    pub max_height: u32,
    /// The whole sequence header OBU.
    obu: Vec<u8>,
}

impl SequenceHeader {
    /// Returns the first sequence header in a temporal unit, if it has one.
    pub fn find(temporal_unit: &[u8]) -> Result<Option<Self>, CodecError> {
        parse_obus(temporal_unit)?
            .into_iter()
            .find(|obu| obu.obu_type == ObuType::SequenceHeader)
            .map(|obu| Self::parse(obu.bytes))
            .transpose()
    }

    /// Parses a sequence header OBU, header and size field included.
    pub fn parse(obu: &[u8]) -> Result<Self, CodecError> {
        let parsed = parse_obus(obu)?;
        let [only] = parsed.as_slice() else {
            return Err(CodecError::MalformedBitstream(
                "expected one sequence header OBU",
            ));
        };
        if only.obu_type != ObuType::SequenceHeader {
            return Err(CodecError::MalformedBitstream("not a sequence header OBU"));
        }
        // The payload follows the header byte and the LEB128 size.
        let size_length = obu[1..]
            .iter()
            .position(|byte| byte & 0x80 == 0)
            .map_or(1, |index| index + 1);
        Self::parse_payload(&obu[1 + size_length..], obu.to_vec())
    }

    /// Parses `sequence_header_obu()` from its payload.
    fn parse_payload(payload: &[u8], obu: Vec<u8>) -> Result<Self, CodecError> {
        let mut reader = BitReader::new(payload);
        let profile = reader.bits(3)?;
        reader.bit()?; // still_picture
        let reduced_still_picture_header = reader.bit()?;
        let (level, tier) = if reduced_still_picture_header {
            (reader.bits(5)?, 0)
        } else {
            Self::skip_to_first_operating_point(&mut reader)?
        };
        let width_bits = reader.bits(4)? as u32 + 1;
        let height_bits = reader.bits(4)? as u32 + 1;
        let max_width = reader.bits(width_bits)? + 1;
        let max_height = reader.bits(height_bits)? + 1;
        let frame_id_numbers_present = !reduced_still_picture_header && reader.bit()?;
        if frame_id_numbers_present {
            reader.bits(4)?; // delta_frame_id_length_minus_2
            reader.bits(3)?; // additional_frame_id_length_minus_1
        }
        reader.bit()?; // use_128x128_superblock
        reader.bit()?; // enable_filter_intra
        reader.bit()?; // enable_intra_edge_filter
        if !reduced_still_picture_header {
            reader.bit()?; // enable_interintra_compound
            reader.bit()?; // enable_masked_compound
            reader.bit()?; // enable_warped_motion
            reader.bit()?; // enable_dual_filter
            let enable_order_hint = reader.bit()?;
            if enable_order_hint {
                reader.bit()?; // enable_jnt_comp
                reader.bit()?; // enable_ref_frame_mvs
            }
            let force_screen_content_tools = if reader.bit()? {
                SELECT_SCREEN_CONTENT_TOOLS
            } else {
                reader.bits(1)?
            };
            if force_screen_content_tools > 0 && !reader.bit()? {
                reader.bit()?; // seq_force_integer_mv
            }
            if enable_order_hint {
                reader.bits(3)?; // order_hint_bits_minus_1
            }
        }
        reader.bit()?; // enable_superres
        reader.bit()?; // enable_cdef
        reader.bit()?; // enable_restoration
        let color = ColorConfig::parse(&mut reader, profile)?;
        Ok(Self {
            profile: profile as u8,
            level: level as u8,
            tier: tier as u8,
            bit_depth: color.bit_depth,
            monochrome: color.monochrome,
            subsampling_x: color.subsampling_x,
            subsampling_y: color.subsampling_y,
            chroma_sample_position: color.chroma_sample_position,
            max_width: max_width as u32,
            max_height: max_height as u32,
            obu,
        })
    }

    /// Reads the timing, decoder model and operating point fields, returning
    /// the first operating point's level and tier.
    fn skip_to_first_operating_point(reader: &mut BitReader<'_>) -> Result<(u64, u64), CodecError> {
        let timing_info_present = reader.bit()?;
        let mut decoder_model_info_present = false;
        let mut buffer_delay_length = 0;
        if timing_info_present {
            reader.bits(32)?; // num_units_in_display_tick
            reader.bits(32)?; // time_scale
            if reader.bit()? {
                reader.uvlc()?; // num_ticks_per_picture_minus_1
            }
            decoder_model_info_present = reader.bit()?;
            if decoder_model_info_present {
                buffer_delay_length = reader.bits(5)? as u32 + 1;
                reader.bits(32)?; // num_units_in_decoding_tick
                reader.bits(5)?; // buffer_removal_time_length_minus_1
                reader.bits(5)?; // frame_presentation_time_length_minus_1
            }
        }
        let initial_display_delay_present = reader.bit()?;
        let operating_points = reader.bits(5)? + 1;
        let mut first = None;
        for _ in 0..operating_points {
            reader.bits(12)?; // operating_point_idc
            let level = reader.bits(5)?;
            let tier = if level > MAX_LEVEL_WITHOUT_TIER {
                reader.bits(1)?
            } else {
                0
            };
            if decoder_model_info_present && reader.bit()? {
                reader.bits(buffer_delay_length)?; // decoder_buffer_delay
                reader.bits(buffer_delay_length)?; // encoder_buffer_delay
                reader.bit()?; // low_delay_mode_flag
            }
            if initial_display_delay_present && reader.bit()? {
                reader.bits(4)?; // initial_display_delay_minus_1
            }
            first.get_or_insert((level, tier));
        }
        first.ok_or(CodecError::MalformedBitstream("no operating points"))
    }

    /// Returns the sequence header OBU.
    pub fn obu(&self) -> &[u8] {
        &self.obu
    }

    /// Returns the AV1 codec configuration record (`av1C`, AV1 ISO-BMFF
    /// binding Section 2.3) of the stream, carrying the sequence header OBU.
    pub fn codec_configuration_record(&self) -> Vec<u8> {
        let high_bitdepth = self.bit_depth > 8;
        let twelve_bit = self.bit_depth == 12;
        let mut record = vec![
            AV1C_MARKER_AND_VERSION,
            (self.profile << 5) | self.level,
            (self.tier << 7)
                | (u8::from(high_bitdepth) << 6)
                | (u8::from(twelve_bit) << 5)
                | (u8::from(self.monochrome) << 4)
                | (u8::from(self.subsampling_x) << 3)
                | (u8::from(self.subsampling_y) << 2)
                | self.chroma_sample_position,
            // initial_presentation_delay_present is 0.
            0,
        ];
        record.extend_from_slice(&self.obu);
        record
    }
}

/// The fields of `color_config()` that describe the sample format.
struct ColorConfig {
    /// The bit depth.
    bit_depth: u8,
    /// Whether the stream has no chroma.
    monochrome: bool,
    /// Whether chroma is subsampled horizontally.
    subsampling_x: bool,
    /// Whether chroma is subsampled vertically.
    subsampling_y: bool,
    /// `chroma_sample_position`.
    chroma_sample_position: u8,
}

impl ColorConfig {
    /// Parses `color_config()` for a stream of `profile`.
    fn parse(reader: &mut BitReader<'_>, profile: u64) -> Result<Self, CodecError> {
        let high_bitdepth = reader.bit()?;
        let bit_depth = if profile == 2 && high_bitdepth {
            if reader.bit()? { 12 } else { 10 }
        } else if high_bitdepth {
            10
        } else {
            8
        };
        let monochrome = profile != 1 && reader.bit()?;
        let (primaries, transfer, matrix) = if reader.bit()? {
            (reader.bits(8)?, reader.bits(8)?, reader.bits(8)?)
        } else {
            (COLOR_UNSPECIFIED, COLOR_UNSPECIFIED, COLOR_UNSPECIFIED)
        };
        let mut config = Self {
            bit_depth,
            monochrome,
            subsampling_x: true,
            subsampling_y: true,
            chroma_sample_position: 0,
        };
        if monochrome {
            reader.bit()?; // color_range
            return Ok(config);
        }
        if primaries == PRIMARIES_BT709 && transfer == TRANSFER_SRGB && matrix == MATRIX_IDENTITY {
            config.subsampling_x = false;
            config.subsampling_y = false;
        } else {
            reader.bit()?; // color_range
            match profile {
                0 => {}
                1 => {
                    config.subsampling_x = false;
                    config.subsampling_y = false;
                }
                _ if bit_depth == 12 => {
                    config.subsampling_x = reader.bit()?;
                    config.subsampling_y = config.subsampling_x && reader.bit()?;
                }
                _ => config.subsampling_y = false,
            }
            if config.subsampling_x && config.subsampling_y {
                config.chroma_sample_position = reader.bits(2)? as u8;
            }
        }
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 1080p sequence header the VA-API encoder writes, which FFmpeg's
    /// parser read back as intended.
    const SEQUENCE_HEADER_1080P: [u8; 16] = [
        0x0a, 0x0e, 0x00, 0x00, 0x00, 0x4a, 0xab, 0xbf, 0xc3, 0x70, 0x08, 0x74, 0x40, 0x43, 0x40,
        0x41,
    ];

    /// A known sequence header parses to its fields.
    #[test]
    fn parses_a_sequence_header() {
        let header = SequenceHeader::parse(&SEQUENCE_HEADER_1080P).unwrap();
        assert_eq!((header.profile, header.level, header.tier), (0, 9, 0));
        assert_eq!((header.max_width, header.max_height), (1920, 1080));
        assert_eq!(header.bit_depth, 8);
        assert!(!header.monochrome && header.subsampling_x && header.subsampling_y);
        assert_eq!(header.obu(), SEQUENCE_HEADER_1080P);
    }

    /// The configuration record is the four header bytes and the OBU.
    #[test]
    fn builds_the_configuration_record() {
        let header = SequenceHeader::parse(&SEQUENCE_HEADER_1080P).unwrap();
        let record = header.codec_configuration_record();
        assert_eq!(record[..4], [0x81, 0x09, 0x0c, 0x00]);
        assert_eq!(record[4..], SEQUENCE_HEADER_1080P);
    }

    /// Temporal units without a sequence header have none to find, and other
    /// OBUs are rejected.
    #[test]
    fn finds_sequence_headers() {
        let frame = [0x32, 0x01, 0x00];
        assert_eq!(SequenceHeader::find(&frame).unwrap(), None);
        let unit = [SEQUENCE_HEADER_1080P.as_slice(), &frame].concat();
        assert_eq!(
            SequenceHeader::find(&unit).unwrap().unwrap().max_width,
            1920
        );
        assert!(SequenceHeader::parse(&frame).is_err());
    }
}
