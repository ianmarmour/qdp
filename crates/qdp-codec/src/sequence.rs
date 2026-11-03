//! Reading AV1 sequence headers (AV1 specification, Section 5.5), for decoders
//! that must be told a stream's format before its first frame, and building
//! the AV1 codec configuration record (`av1C`) such decoders take.

use crate::bits::BitReader;
use crate::{CodecError, ObuType, parse_obus};

/// The first byte of an `av1C` record: `marker` 1 and `version` 1.
const AV1C_MARKER_AND_VERSION: u8 = 0x81;

/// The `seq_level_idx` above which a tier bit follows.
const MAX_LEVEL_WITHOUT_TIER: u64 = 7;

/// `color_primaries` of BT.709.
const PRIMARIES_BT709: u64 = 1;

/// `transfer_characteristics` of sRGB.
const TRANSFER_SRGB: u64 = 13;

/// `matrix_coefficients` of identity (RGB).
const MATRIX_IDENTITY: u64 = 0;

/// The values used when no colour description is present: unspecified.
const COLOR_UNSPECIFIED: u64 = 2;

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
    /// The colour primaries, transfer characteristics and matrix, if the
    /// header describes them (`color_description_present_flag`).
    pub color_description: Option<ColorDescription>,
    /// Whether samples use the full range rather than the limited one.
    pub full_range: bool,
    /// Whether super-resolution may be used (`enable_superres`).
    pub superres: bool,
    /// Whether frames may carry film grain (`film_grain_params_present`).
    pub film_grain: bool,
    /// The coding tools frames may use, which their headers depend on.
    #[cfg_attr(not(any(windows, target_os = "linux", test)), allow(dead_code))]
    pub(crate) coding: SequenceCoding,
    /// The whole sequence header OBU.
    obu: Vec<u8>,
}

/// Whether a coding tool is off, on, or chosen by each frame's header.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum ToolChoice {
    /// Off for every frame.
    Off,
    /// On for every frame.
    On,
    /// Chosen by each frame's header.
    #[default]
    PerFrame,
}

/// The fields of a sequence header that frame headers depend on (AV1
/// specification, Section 5.5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SequenceCoding {
    /// `still_picture`.
    pub(crate) still_picture: bool,
    /// `reduced_still_picture_header`.
    pub(crate) reduced_still_picture_header: bool,
    /// The decoder model, if `decoder_model_info_present_flag` is set.
    pub(crate) decoder_model: Option<DecoderModel>,
    /// The bits of `frame_width_minus_1`.
    pub(crate) frame_width_bits: u32,
    /// The bits of `frame_height_minus_1`.
    pub(crate) frame_height_bits: u32,
    /// The lengths of frame IDs, if `frame_id_numbers_present_flag` is set.
    pub(crate) frame_ids: Option<FrameIdLengths>,
    /// `use_128x128_superblock`.
    pub(crate) use_128x128_superblock: bool,
    /// `enable_filter_intra`.
    pub(crate) enable_filter_intra: bool,
    /// `enable_intra_edge_filter`.
    pub(crate) enable_intra_edge_filter: bool,
    /// `enable_interintra_compound`.
    pub(crate) enable_interintra_compound: bool,
    /// `enable_masked_compound`.
    pub(crate) enable_masked_compound: bool,
    /// `enable_warped_motion`.
    pub(crate) enable_warped_motion: bool,
    /// `enable_dual_filter`.
    pub(crate) enable_dual_filter: bool,
    /// `enable_jnt_comp`.
    pub(crate) enable_jnt_comp: bool,
    /// `enable_ref_frame_mvs`.
    pub(crate) enable_ref_frame_mvs: bool,
    /// `seq_force_screen_content_tools`.
    pub(crate) screen_content_tools: ToolChoice,
    /// `seq_force_integer_mv`.
    pub(crate) integer_mv: ToolChoice,
    /// `OrderHintBits`: the bits of `order_hint`, 0 if `enable_order_hint` is
    /// not set.
    pub(crate) order_hint_bits: u32,
    /// `enable_cdef`.
    pub(crate) enable_cdef: bool,
    /// `enable_restoration`.
    pub(crate) enable_restoration: bool,
    /// `separate_uv_delta_q`.
    pub(crate) separate_uv_delta_q: bool,
}

/// The decoder model's fields that frame headers depend on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DecoderModel {
    /// `equal_picture_interval` of the timing info.
    pub(crate) equal_picture_interval: bool,
    /// The bits of `buffer_removal_time`.
    pub(crate) buffer_removal_time_bits: u32,
    /// The bits of `frame_presentation_time`.
    pub(crate) frame_presentation_time_bits: u32,
    /// `operating_point_idc` of the operating points that have decoder model
    /// parameters (`decoder_model_present_for_this_op`).
    pub(crate) modelled_operating_points: Vec<u16>,
}

/// The lengths of frame IDs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrameIdLengths {
    /// `idLen`: the bits of `current_frame_id`.
    pub(crate) frame_id_bits: u32,
    /// The bits of `delta_frame_id_minus_1`.
    pub(crate) delta_frame_id_bits: u32,
}

/// The first operating point's level and tier, and the decoder model.
struct OperatingPoints {
    /// `seq_level_idx` of the first operating point.
    level: u64,
    /// `seq_tier` of the first operating point.
    tier: u64,
    /// The decoder model, if present.
    decoder_model: Option<DecoderModel>,
}

/// The colour code points of a stream, from H.273.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ColorDescription {
    /// `color_primaries`.
    pub primaries: u8,
    /// `transfer_characteristics`.
    pub transfer: u8,
    /// `matrix_coefficients`.
    pub matrix: u8,
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
        let still_picture = reader.bit()?;
        let reduced_still_picture_header = reader.bit()?;
        let points = if reduced_still_picture_header {
            OperatingPoints {
                level: reader.bits(5)?,
                tier: 0,
                decoder_model: None,
            }
        } else {
            Self::parse_operating_points(&mut reader)?
        };
        let frame_width_bits = reader.bits32(4)? + 1;
        let frame_height_bits = reader.bits32(4)? + 1;
        let max_width = reader.bits(frame_width_bits)? + 1;
        let max_height = reader.bits(frame_height_bits)? + 1;
        let frame_ids = if !reduced_still_picture_header && reader.bit()? {
            let delta_frame_id_bits = reader.bits32(4)? + 2;
            let additional_bits = reader.bits32(3)? + 1;
            Some(FrameIdLengths {
                frame_id_bits: delta_frame_id_bits + additional_bits,
                delta_frame_id_bits,
            })
        } else {
            None
        };
        let mut coding = SequenceCoding {
            still_picture,
            reduced_still_picture_header,
            decoder_model: points.decoder_model,
            frame_width_bits,
            frame_height_bits,
            frame_ids,
            use_128x128_superblock: reader.bit()?,
            enable_filter_intra: reader.bit()?,
            enable_intra_edge_filter: reader.bit()?,
            ..SequenceCoding::default()
        };
        if !reduced_still_picture_header {
            coding.enable_interintra_compound = reader.bit()?;
            coding.enable_masked_compound = reader.bit()?;
            coding.enable_warped_motion = reader.bit()?;
            coding.enable_dual_filter = reader.bit()?;
            let enable_order_hint = reader.bit()?;
            if enable_order_hint {
                coding.enable_jnt_comp = reader.bit()?;
                coding.enable_ref_frame_mvs = reader.bit()?;
            }
            coding.screen_content_tools = Self::parse_choice(&mut reader)?;
            coding.integer_mv = if coding.screen_content_tools == ToolChoice::Off {
                ToolChoice::PerFrame
            } else {
                Self::parse_choice(&mut reader)?
            };
            if enable_order_hint {
                coding.order_hint_bits = reader.bits32(3)? + 1;
            }
        }
        let superres = reader.bit()?;
        coding.enable_cdef = reader.bit()?;
        coding.enable_restoration = reader.bit()?;
        let color = ColorConfig::parse(&mut reader, profile)?;
        coding.separate_uv_delta_q = color.separate_uv_delta_q;
        let film_grain = reader.bit()?;
        Ok(Self {
            profile: profile as u8,
            level: points.level as u8,
            tier: points.tier as u8,
            bit_depth: color.bit_depth,
            monochrome: color.monochrome,
            subsampling_x: color.subsampling_x,
            subsampling_y: color.subsampling_y,
            chroma_sample_position: color.chroma_sample_position,
            max_width: max_width as u32,
            max_height: max_height as u32,
            color_description: color.description,
            full_range: color.full_range,
            superres,
            film_grain,
            coding,
            obu,
        })
    }

    /// Reads a `seq_choose_*` flag and, if it is not set, the `seq_force_*`
    /// value.
    fn parse_choice(reader: &mut BitReader<'_>) -> Result<ToolChoice, CodecError> {
        Ok(if reader.bit()? {
            ToolChoice::PerFrame
        } else if reader.bit()? {
            ToolChoice::On
        } else {
            ToolChoice::Off
        })
    }

    /// Reads the timing, decoder model and operating point fields.
    fn parse_operating_points(reader: &mut BitReader<'_>) -> Result<OperatingPoints, CodecError> {
        let timing_info_present = reader.bit()?;
        let mut decoder_model = None;
        let mut buffer_delay_length = 0;
        if timing_info_present {
            reader.bits(32)?; // num_units_in_display_tick
            reader.bits(32)?; // time_scale
            let equal_picture_interval = reader.bit()?;
            if equal_picture_interval {
                reader.uvlc()?; // num_ticks_per_picture_minus_1
            }
            if reader.bit()? {
                buffer_delay_length = reader.bits32(5)? + 1;
                reader.bits(32)?; // num_units_in_decoding_tick
                decoder_model = Some(DecoderModel {
                    equal_picture_interval,
                    buffer_removal_time_bits: reader.bits32(5)? + 1,
                    frame_presentation_time_bits: reader.bits32(5)? + 1,
                    modelled_operating_points: Vec::new(),
                });
            }
        }
        let initial_display_delay_present = reader.bit()?;
        let operating_points = reader.bits(5)? + 1;
        let mut first = None;
        for _ in 0..operating_points {
            let idc = reader.bits(12)? as u16;
            let level = reader.bits(5)?;
            let tier = if level > MAX_LEVEL_WITHOUT_TIER {
                reader.bits(1)?
            } else {
                0
            };
            if let Some(model) = decoder_model.as_mut() {
                if reader.bit()? {
                    model.modelled_operating_points.push(idc);
                    reader.bits(buffer_delay_length)?; // decoder_buffer_delay
                    reader.bits(buffer_delay_length)?; // encoder_buffer_delay
                    reader.bit()?; // low_delay_mode_flag
                }
            }
            if initial_display_delay_present && reader.bit()? {
                reader.bits(4)?; // initial_display_delay_minus_1
            }
            first.get_or_insert((level, tier));
        }
        let (level, tier) = first.ok_or(CodecError::MalformedBitstream("no operating points"))?;
        Ok(OperatingPoints {
            level,
            tier,
            decoder_model,
        })
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
    /// The colour code points, if described.
    description: Option<ColorDescription>,
    /// Whether samples use the full range.
    full_range: bool,
    /// `separate_uv_delta_q`.
    separate_uv_delta_q: bool,
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
        let description = if reader.bit()? {
            Some(ColorDescription {
                primaries: reader.bits(8)? as u8,
                transfer: reader.bits(8)? as u8,
                matrix: reader.bits(8)? as u8,
            })
        } else {
            None
        };
        let (primaries, transfer, matrix) = description.map_or(
            (COLOR_UNSPECIFIED, COLOR_UNSPECIFIED, COLOR_UNSPECIFIED),
            |description| {
                (
                    u64::from(description.primaries),
                    u64::from(description.transfer),
                    u64::from(description.matrix),
                )
            },
        );
        let mut config = Self {
            bit_depth,
            monochrome,
            subsampling_x: true,
            subsampling_y: true,
            chroma_sample_position: 0,
            description,
            full_range: false,
            separate_uv_delta_q: false,
        };
        if monochrome {
            config.full_range = reader.bit()?; // color_range
            return Ok(config);
        }
        if primaries == PRIMARIES_BT709 && transfer == TRANSFER_SRGB && matrix == MATRIX_IDENTITY {
            // RGB is always full range.
            config.subsampling_x = false;
            config.subsampling_y = false;
            config.full_range = true;
        } else {
            config.full_range = reader.bit()?; // color_range
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
        config.separate_uv_delta_q = reader.bit()?;
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
        assert_eq!(
            header.color_description,
            Some(ColorDescription {
                primaries: 1,
                transfer: 13,
                matrix: 1,
            })
        );
        assert!(!header.full_range && !header.superres && !header.film_grain);
        assert_eq!(header.obu(), SEQUENCE_HEADER_1080P);
    }

    /// The fields frame headers depend on are read too: the VA-API
    /// encoder's stream has 8-bit order hints and CDEF, and no other tool.
    #[test]
    fn reads_the_coding_tools() {
        let coding = SequenceHeader::parse(&SEQUENCE_HEADER_1080P)
            .unwrap()
            .coding;
        assert_eq!(coding.order_hint_bits, 8);
        assert_eq!(
            (coding.frame_width_bits, coding.frame_height_bits),
            (11, 11)
        );
        assert!(coding.enable_cdef && !coding.enable_restoration);
        assert_eq!(coding.screen_content_tools, ToolChoice::Off);
        assert_eq!(coding.integer_mv, ToolChoice::PerFrame);
        assert!(!coding.use_128x128_superblock && !coding.reduced_still_picture_header);
        assert!(!coding.enable_ref_frame_mvs && !coding.separate_uv_delta_q);
        assert_eq!((coding.decoder_model, coding.frame_ids), (None, None));
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
