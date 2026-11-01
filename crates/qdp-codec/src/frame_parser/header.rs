//! `uncompressed_header()`: the header of each AV1 frame (AV1 specification,
//! Section 5.9), with the values the decoding process derives from it that a
//! hardware decoder must be given (Section 7).

use crate::CodecError;
use crate::bits::BitReader;
use crate::sequence::{SequenceHeader, ToolChoice};

use super::grain::FilmGrain;
use super::motion::{self, GlobalMotion};
use super::{NUM_REF_FRAMES, REFS_PER_FRAME, ReferenceSlot};

/// `refresh_frame_flags` that replaces every reference slot.
pub(crate) const ALL_FRAMES: u8 = 0xff;

/// `primary_ref_frame` when no reference provides the frame's starting
/// state.
pub(crate) const PRIMARY_REF_NONE: u8 = 7;

/// `SUPERRES_NUM`: the numerator of the super-resolution scale, and the
/// denominator when it is not used.
pub(crate) const SUPERRES_NUM: u8 = 8;

/// `SUPERRES_DENOM_MIN`: the denominator `coded_denom` 0 stands for.
const SUPERRES_DENOM_MIN: u8 = 9;

/// The bits of `coded_denom`.
const SUPERRES_DENOM_BITS: u32 = 3;

/// `interpolation_filter` of a frame that chooses the filter per block.
pub(crate) const INTERPOLATION_SWITCHABLE: u8 = 4;

/// `MAX_TILE_WIDTH`: the widest tile, in luma samples.
const MAX_TILE_WIDTH: u32 = 4096;

/// `MAX_TILE_AREA`: the largest tile, in luma samples.
const MAX_TILE_AREA: u32 = 4096 * 2304;

/// `MAX_TILE_COLS`: the most tile columns.
pub(crate) const MAX_TILE_COLS: usize = 64;

/// `MAX_TILE_ROWS`: the most tile rows.
pub(crate) const MAX_TILE_ROWS: usize = 64;

/// `MAX_SEGMENTS`.
pub(crate) const MAX_SEGMENTS: usize = 8;

/// `SEG_LVL_MAX`: the number of segment features.
pub(crate) const SEG_LVL_MAX: usize = 8;

/// `SEG_LVL_ALT_Q`: the segment feature that changes the quantizer.
const SEG_LVL_ALT_Q: usize = 0;

/// `Segmentation_Feature_Bits`.
const SEGMENT_FEATURE_BITS: [u32; SEG_LVL_MAX] = [8, 6, 6, 6, 6, 3, 0, 0];

/// `Segmentation_Feature_Signed`.
const SEGMENT_FEATURE_SIGNED: [bool; SEG_LVL_MAX] =
    [true, true, true, true, true, false, false, false];

/// `Segmentation_Feature_Max`: 255 for the quantizer, `MAX_LOOP_FILTER` for
/// the loop filter levels, and 7 for the reference frame.
const SEGMENT_FEATURE_MAX: [i32; SEG_LVL_MAX] = [255, 63, 63, 63, 63, 7, 0, 0];

/// The loop filter's reference deltas when no reference provides them, for
/// `INTRA_FRAME`, `LAST_FRAME` … `ALTREF_FRAME`.
pub(crate) const DEFAULT_LOOP_FILTER_REF_DELTAS: [i8; NUM_REF_FRAMES] = [1, 0, 0, 0, -1, 0, -1, -1];

/// The bits of a loop filter level.
const LOOP_FILTER_LEVEL_BITS: u32 = 6;

/// The bits of a loop filter delta, sign included.
const LOOP_FILTER_DELTA_BITS: u32 = 7;

/// The bits of a quantizer delta, sign included.
const DELTA_Q_BITS: u32 = 7;

/// `Remap_Lr_Type`: the restoration type each coded `lr_type` stands for.
const REMAP_LR_TYPE: [RestorationType; 4] = [
    RestorationType::None,
    RestorationType::Switchable,
    RestorationType::Wiener,
    RestorationType::SelfGuided,
];

/// The log2 of the restoration unit size `lr_unit_shift` 0 stands for: 64
/// luma samples.
const RESTORATION_UNIT_LOG2_BASE: u8 = 6;

/// `frame_type`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum FrameType {
    /// `KEY_FRAME`.
    #[default]
    Key = 0,
    /// `INTER_FRAME`.
    Inter = 1,
    /// `INTRA_ONLY_FRAME`.
    IntraOnly = 2,
    /// `SWITCH_FRAME`.
    Switch = 3,
}

impl FrameType {
    /// Returns the type `frame_type` codes.
    fn from_bits(bits: u64) -> Self {
        match bits {
            0 => Self::Key,
            1 => Self::Inter,
            2 => Self::IntraOnly,
            _ => Self::Switch,
        }
    }

    /// Returns `FrameIsIntra`.
    pub(crate) fn is_intra(self) -> bool {
        matches!(self, Self::Key | Self::IntraOnly)
    }
}

/// A frame's sizes (Sections 5.9.5 to 5.9.8).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrameSize {
    /// `UpscaledWidth`: the width after super-resolution, which the decoded
    /// picture has.
    pub(crate) upscaled_width: u32,
    /// `FrameWidth`: the width frames are coded at.
    pub(crate) width: u32,
    /// `FrameHeight`.
    pub(crate) height: u32,
    /// `RenderWidth`.
    pub(crate) render_width: u32,
    /// `RenderHeight`.
    pub(crate) render_height: u32,
    /// `SuperresDenom`: [`SUPERRES_NUM`] when super-resolution is not used.
    pub(crate) superres_denom: u8,
}

impl FrameSize {
    /// Returns `use_superres`.
    pub(crate) fn uses_superres(&self) -> bool {
        self.superres_denom != SUPERRES_NUM
    }

    /// Returns `MiCols`: the width in 4 × 4 blocks, rounded up to 8 samples.
    fn mi_cols(&self) -> u32 {
        2 * self.width.div_ceil(8)
    }

    /// Returns `MiRows`.
    fn mi_rows(&self) -> u32 {
        2 * self.height.div_ceil(8)
    }
}

/// How a frame is split into tiles (Section 5.9.15).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TileInfo {
    /// `TileColsLog2`.
    pub(crate) cols_log2: u32,
    /// `TileRowsLog2`.
    pub(crate) rows_log2: u32,
    /// The width of each tile column, in superblocks.
    pub(crate) col_widths: Vec<u32>,
    /// The height of each tile row, in superblocks.
    pub(crate) row_heights: Vec<u32>,
    /// `context_update_tile_id`.
    pub(crate) context_update_tile_id: u32,
    /// `TileSizeBytes`: the bytes of each tile's size field.
    pub(crate) tile_size_bytes: u32,
}

impl TileInfo {
    /// Returns `TileCols`.
    pub(crate) fn cols(&self) -> usize {
        self.col_widths.len()
    }

    /// Returns `TileRows`.
    pub(crate) fn rows(&self) -> usize {
        self.row_heights.len()
    }

    /// Returns `NumTiles`.
    pub(crate) fn count(&self) -> usize {
        self.cols() * self.rows()
    }
}

/// The quantizer matrices of a frame that uses them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct QuantizerMatrices {
    /// `qm_y`.
    pub(crate) y: u8,
    /// `qm_u`.
    pub(crate) u: u8,
    /// `qm_v`.
    pub(crate) v: u8,
}

/// `quantization_params()` (Section 5.9.12).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Quantization {
    /// `base_q_idx`.
    pub(crate) base_q_idx: u8,
    /// `DeltaQYDc`.
    pub(crate) delta_q_y_dc: i8,
    /// `DeltaQUDc`.
    pub(crate) delta_q_u_dc: i8,
    /// `DeltaQUAc`.
    pub(crate) delta_q_u_ac: i8,
    /// `DeltaQVDc`.
    pub(crate) delta_q_v_dc: i8,
    /// `DeltaQVAc`.
    pub(crate) delta_q_v_ac: i8,
    /// The quantizer matrices, if `using_qmatrix` is set.
    pub(crate) matrices: Option<QuantizerMatrices>,
}

/// The features of each segment: `FeatureEnabled` and `FeatureData`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SegmentFeatures {
    /// The features each segment enables, one bit per feature.
    pub(crate) enabled: [u8; MAX_SEGMENTS],
    /// Each segment's feature values, clipped to their ranges.
    pub(crate) data: [[i16; SEG_LVL_MAX]; MAX_SEGMENTS],
}

impl SegmentFeatures {
    /// Returns whether `segment` enables `feature`.
    pub(crate) fn is_enabled(&self, segment: usize, feature: usize) -> bool {
        self.enabled[segment] & (1 << feature) != 0
    }
}

/// `segmentation_params()` (Section 5.9.14).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Segmentation {
    /// `segmentation_enabled`.
    pub(crate) enabled: bool,
    /// `segmentation_update_map`.
    pub(crate) update_map: bool,
    /// `segmentation_temporal_update`.
    pub(crate) temporal_update: bool,
    /// `segmentation_update_data`.
    pub(crate) update_data: bool,
    /// The features of each segment, read or loaded from the primary
    /// reference.
    pub(crate) features: SegmentFeatures,
}

/// `delta_q_params()` and `delta_lf_params()` (Sections 5.9.17 and 5.9.18).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DeltaParams {
    /// `delta_q_present`.
    pub(crate) q_present: bool,
    /// `delta_q_res`.
    pub(crate) q_res: u8,
    /// `delta_lf_present`.
    pub(crate) lf_present: bool,
    /// `delta_lf_res`.
    pub(crate) lf_res: u8,
    /// `delta_lf_multi`.
    pub(crate) lf_multi: bool,
}

/// `loop_filter_params()` (Section 5.9.11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoopFilter {
    /// `loop_filter_level`: vertical and horizontal luma edges, then U and V.
    pub(crate) levels: [u8; 4],
    /// `loop_filter_sharpness`.
    pub(crate) sharpness: u8,
    /// `loop_filter_delta_enabled`.
    pub(crate) delta_enabled: bool,
    /// `loop_filter_delta_update`.
    pub(crate) delta_update: bool,
    /// `loop_filter_ref_deltas`, for `INTRA_FRAME` … `ALTREF_FRAME`.
    pub(crate) ref_deltas: [i8; NUM_REF_FRAMES],
    /// `loop_filter_mode_deltas`.
    pub(crate) mode_deltas: [i8; 2],
}

impl Default for LoopFilter {
    fn default() -> Self {
        Self {
            levels: [0; 4],
            sharpness: 0,
            delta_enabled: false,
            delta_update: false,
            ref_deltas: DEFAULT_LOOP_FILTER_REF_DELTAS,
            mode_deltas: [0; 2],
        }
    }
}

/// `cdef_params()` (Section 5.9.19), with the strengths as coded: a
/// secondary strength of 3 stands for 4.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Cdef {
    /// `cdef_damping_minus_3`.
    pub(crate) damping_minus_3: u8,
    /// `cdef_bits`.
    pub(crate) bits: u8,
    /// `cdef_y_pri_strength`.
    pub(crate) y_primary: [u8; 8],
    /// `cdef_y_sec_strength`, as coded.
    pub(crate) y_secondary: [u8; 8],
    /// `cdef_uv_pri_strength`.
    pub(crate) uv_primary: [u8; 8],
    /// `cdef_uv_sec_strength`, as coded.
    pub(crate) uv_secondary: [u8; 8],
}

/// `FrameRestorationType`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum RestorationType {
    /// `RESTORE_NONE`.
    #[default]
    None = 0,
    /// `RESTORE_WIENER`.
    Wiener = 1,
    /// `RESTORE_SGRPROJ`.
    SelfGuided = 2,
    /// `RESTORE_SWITCHABLE`.
    Switchable = 3,
}

/// `lr_params()` (Section 5.9.20).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Restoration {
    /// The restoration type of each plane.
    pub(crate) types: [RestorationType; 3],
    /// The log2 of `LoopRestorationSize` of each plane, when restoration is
    /// used.
    pub(crate) unit_size_log2: [u8; 3],
}

impl Restoration {
    /// Returns `UsesLr`.
    pub(crate) fn is_used(&self) -> bool {
        self.types.iter().any(|&kind| kind != RestorationType::None)
    }
}

/// `TxMode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) enum TxMode {
    /// `ONLY_4X4`.
    #[default]
    Only4x4 = 0,
    /// `TX_MODE_LARGEST`.
    Largest = 1,
    /// `TX_MODE_SELECT`.
    Select = 2,
}

/// A frame's header, and the values derived from it and from the reference
/// slots that decoding it needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FrameHeader {
    /// `show_existing_frame`: the frame shows a reference slot's frame
    /// instead of coding one, and no other field but those below that say
    /// otherwise is set.
    pub(crate) show_existing_frame: bool,
    /// `frame_to_show_map_idx`, for a frame that shows an existing one.
    pub(crate) frame_to_show_map_idx: u8,
    /// `frame_type`; for an existing frame shown, the shown frame's.
    pub(crate) frame_type: FrameType,
    /// `show_frame`.
    pub(crate) show_frame: bool,
    /// `showable_frame`.
    pub(crate) showable_frame: bool,
    /// `error_resilient_mode`.
    pub(crate) error_resilient_mode: bool,
    /// `disable_cdf_update`.
    pub(crate) disable_cdf_update: bool,
    /// `allow_screen_content_tools`.
    pub(crate) allow_screen_content_tools: bool,
    /// `force_integer_mv`, which intra frames always set.
    pub(crate) force_integer_mv: bool,
    /// `current_frame_id`.
    pub(crate) current_frame_id: u32,
    /// `OrderHint`.
    pub(crate) order_hint: u32,
    /// `primary_ref_frame`.
    pub(crate) primary_ref_frame: u8,
    /// `refresh_frame_flags`: the reference slots the frame replaces.
    pub(crate) refresh_frame_flags: u8,
    /// The frame's sizes; for an existing frame shown, the shown frame's.
    pub(crate) size: FrameSize,
    /// `allow_intrabc`.
    pub(crate) allow_intrabc: bool,
    /// `ref_frame_idx`: the slot of each of `LAST_FRAME` … `ALTREF_FRAME`.
    pub(crate) ref_frame_idx: [u8; REFS_PER_FRAME],
    /// `allow_high_precision_mv`.
    pub(crate) allow_high_precision_mv: bool,
    /// `interpolation_filter`, [`INTERPOLATION_SWITCHABLE`] when chosen per
    /// block.
    pub(crate) interpolation_filter: u8,
    /// `is_motion_mode_switchable`.
    pub(crate) is_motion_mode_switchable: bool,
    /// `use_ref_frame_mvs`.
    pub(crate) use_ref_frame_mvs: bool,
    /// `disable_frame_end_update_cdf`.
    pub(crate) disable_frame_end_update_cdf: bool,
    /// The tiles.
    pub(crate) tiles: TileInfo,
    /// The quantizers.
    pub(crate) quantization: Quantization,
    /// The segmentation.
    pub(crate) segmentation: Segmentation,
    /// The quantizer and loop filter deltas blocks may code.
    pub(crate) delta: DeltaParams,
    /// `CodedLossless`.
    pub(crate) coded_lossless: bool,
    /// `AllLossless`.
    pub(crate) all_lossless: bool,
    /// The loop filter.
    pub(crate) loop_filter: LoopFilter,
    /// The constrained directional enhancement filter.
    pub(crate) cdef: Cdef,
    /// Loop restoration.
    pub(crate) restoration: Restoration,
    /// `TxMode`.
    pub(crate) tx_mode: TxMode,
    /// `reference_select`.
    pub(crate) reference_select: bool,
    /// `skip_mode_present`.
    pub(crate) skip_mode_present: bool,
    /// `allow_warped_motion`.
    pub(crate) allow_warped_motion: bool,
    /// `reduced_tx_set`.
    pub(crate) reduced_tx_set: bool,
    /// The global motion of each reference, indexed by reference frame:
    /// `LAST_FRAME` (1) … `ALTREF_FRAME` (7); index 0 is unused.
    pub(crate) global_motion: [GlobalMotion; NUM_REF_FRAMES],
    /// The film grain; for an existing frame shown, the shown frame's.
    pub(crate) film_grain: FilmGrain,
}

/// Returns `get_relative_dist(a, b)`: how far order hint `a` is after `b`,
/// with the hints wrapping at `order_hint_bits` bits.
pub(crate) fn relative_distance(order_hint_bits: u32, a: u32, b: u32) -> i32 {
    if order_hint_bits == 0 {
        return 0;
    }
    let difference = a.wrapping_sub(b) as i32;
    let half = 1i32 << (order_hint_bits - 1);
    (difference & (half - 1)) - (difference & half)
}

/// Reads `uncompressed_header()` for a stream of `sequence`, whose
/// reference slots are `refs`. The header updates the slots' validity as
/// the specification's does; the caller stores the frame in them once it is
/// complete.
pub(crate) fn parse(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    refs: &mut [ReferenceSlot; NUM_REF_FRAMES],
) -> Result<FrameHeader, CodecError> {
    let coding = &sequence.coding;
    let mut header = FrameHeader {
        primary_ref_frame: PRIMARY_REF_NONE,
        ..FrameHeader::default()
    };
    if coding.reduced_still_picture_header {
        header.frame_type = FrameType::Key;
        header.show_frame = true;
        header.error_resilient_mode = true;
    } else {
        header.show_existing_frame = reader.bit()?;
        if header.show_existing_frame {
            return show_existing_frame(reader, sequence, refs, header);
        }
        header.frame_type = FrameType::from_bits(reader.bits(2)?);
        header.show_frame = reader.bit()?;
        if header.show_frame {
            temporal_point_info(reader, sequence)?;
            header.showable_frame = header.frame_type != FrameType::Key;
        } else {
            header.showable_frame = reader.bit()?;
        }
        header.error_resilient_mode = if header.frame_type == FrameType::Switch
            || (header.frame_type == FrameType::Key && header.show_frame)
        {
            true
        } else {
            reader.bit()?
        };
    }
    let frame_is_intra = header.frame_type.is_intra();
    if header.frame_type == FrameType::Key && header.show_frame {
        for slot in refs.iter_mut() {
            slot.valid = false;
            slot.order_hint = 0;
        }
    }
    header.disable_cdf_update = reader.bit()?;
    header.allow_screen_content_tools = choose(reader, coding.screen_content_tools)?;
    header.force_integer_mv = if header.allow_screen_content_tools {
        choose(reader, coding.integer_mv)?
    } else {
        false
    };
    if frame_is_intra {
        header.force_integer_mv = true;
    }
    if let Some(lengths) = coding.frame_ids {
        header.current_frame_id = reader.bits32(lengths.frame_id_bits)?;
        mark_ref_frames(
            refs,
            header.current_frame_id,
            lengths.frame_id_bits,
            lengths.delta_frame_id_bits,
        );
    }
    let frame_size_override = if header.frame_type == FrameType::Switch {
        true
    } else if coding.reduced_still_picture_header {
        false
    } else {
        reader.bit()?
    };
    header.order_hint = reader.bits32(coding.order_hint_bits)?;
    if !frame_is_intra && !header.error_resilient_mode {
        header.primary_ref_frame = reader.bits(3)? as u8;
    }
    buffer_removal_times(reader, sequence)?;
    header.refresh_frame_flags = if header.frame_type == FrameType::Switch
        || (header.frame_type == FrameType::Key && header.show_frame)
    {
        ALL_FRAMES
    } else {
        reader.bits(8)? as u8
    };
    if (!frame_is_intra || header.refresh_frame_flags != ALL_FRAMES)
        && header.error_resilient_mode
        && coding.order_hint_bits > 0
    {
        for slot in refs.iter_mut() {
            let ref_order_hint = reader.bits32(coding.order_hint_bits)?;
            if ref_order_hint != slot.order_hint {
                slot.valid = false;
                slot.order_hint = ref_order_hint;
            }
        }
    }
    if frame_is_intra {
        header.size = frame_size(reader, sequence, frame_size_override)?;
        render_size(reader, &mut header.size)?;
        if header.allow_screen_content_tools && !header.size.uses_superres() {
            header.allow_intrabc = reader.bit()?;
        }
    } else {
        inter_frame_references(reader, sequence, refs, &mut header)?;
        header.size = if frame_size_override && !header.error_resilient_mode {
            frame_size_with_refs(reader, sequence, refs, &header.ref_frame_idx)?
        } else {
            let mut size = frame_size(reader, sequence, frame_size_override)?;
            render_size(reader, &mut size)?;
            size
        };
        header.allow_high_precision_mv = !header.force_integer_mv && reader.bit()?;
        header.interpolation_filter = if reader.bit()? {
            INTERPOLATION_SWITCHABLE
        } else {
            reader.bits(2)? as u8
        };
        header.is_motion_mode_switchable = reader.bit()?;
        header.use_ref_frame_mvs =
            !header.error_resilient_mode && coding.enable_ref_frame_mvs && reader.bit()?;
    }
    header.disable_frame_end_update_cdf =
        coding.reduced_still_picture_header || header.disable_cdf_update || reader.bit()?;

    // setup_past_independence() or load_previous(): the state the frame
    // starts from.
    let previous = (header.primary_ref_frame != PRIMARY_REF_NONE).then(|| {
        let slot = header.ref_frame_idx[usize::from(header.primary_ref_frame)];
        &refs[usize::from(slot)]
    });
    let num_planes = if sequence.monochrome { 1 } else { 3 };
    header.tiles = tile_info(reader, sequence, &header.size)?;
    header.quantization = quantization_params(reader, sequence, num_planes)?;
    header.segmentation = segmentation_params(
        reader,
        header.primary_ref_frame,
        previous.map(|slot| slot.segment_features),
    )?;
    header.delta = delta_params(reader, &header)?;
    header.coded_lossless = coded_lossless(&header);
    header.all_lossless = header.coded_lossless && !header.size.uses_superres();
    header.loop_filter = loop_filter_params(
        reader,
        &header,
        num_planes,
        previous.map_or(LoopFilter::default(), |slot| LoopFilter {
            ref_deltas: slot.loop_filter_ref_deltas,
            mode_deltas: slot.loop_filter_mode_deltas,
            ..LoopFilter::default()
        }),
    )?;
    header.cdef = cdef_params(reader, sequence, &header, num_planes)?;
    header.restoration = lr_params(reader, sequence, &header, num_planes)?;
    header.tx_mode = if header.coded_lossless {
        TxMode::Only4x4
    } else if reader.bit()? {
        TxMode::Select
    } else {
        TxMode::Largest
    };
    header.reference_select = !frame_is_intra && reader.bit()?;
    header.skip_mode_present = skip_mode_allowed(sequence, refs, &header) && reader.bit()?;
    header.allow_warped_motion = !frame_is_intra
        && !header.error_resilient_mode
        && coding.enable_warped_motion
        && reader.bit()?;
    header.reduced_tx_set = reader.bit()?;
    let previous_motion = previous.map_or(motion::DEFAULT_PARAMS, |slot| slot.global_motion);
    header.global_motion = motion::global_motion_params(
        reader,
        frame_is_intra,
        header.allow_high_precision_mv,
        &previous_motion,
    )?;
    header.film_grain = super::grain::film_grain_params(reader, sequence, &header, refs)?;
    Ok(header)
}

/// Reads the rest of the header of a frame that shows the frame in a
/// reference slot.
fn show_existing_frame(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    refs: &[ReferenceSlot; NUM_REF_FRAMES],
    mut header: FrameHeader,
) -> Result<FrameHeader, CodecError> {
    header.frame_to_show_map_idx = reader.bits(3)? as u8;
    temporal_point_info(reader, sequence)?;
    if let Some(lengths) = sequence.coding.frame_ids {
        reader.bits(lengths.frame_id_bits)?; // display_frame_id
    }
    let shown = &refs[usize::from(header.frame_to_show_map_idx)];
    if !shown.valid {
        return Err(CodecError::MalformedBitstream(
            "show_existing_frame names an empty reference slot",
        ));
    }
    header.frame_type = shown.frame_type;
    header.show_frame = true;
    header.size = shown.size;
    header.order_hint = shown.order_hint;
    header.current_frame_id = shown.frame_id;
    header.film_grain = shown.film_grain.clone();
    // Showing a key frame again refreshes every slot with it (Section 7.21).
    header.refresh_frame_flags = if shown.frame_type == FrameType::Key {
        ALL_FRAMES
    } else {
        0
    };
    Ok(header)
}

/// Reads a choice the sequence header leaves to frames, or returns the one
/// it made.
fn choose(reader: &mut BitReader<'_>, choice: ToolChoice) -> Result<bool, CodecError> {
    Ok(match choice {
        ToolChoice::Off => false,
        ToolChoice::On => true,
        ToolChoice::PerFrame => reader.bit()?,
    })
}

/// Reads `temporal_point_info()` of a shown frame, if the stream has one.
fn temporal_point_info(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
) -> Result<(), CodecError> {
    if let Some(model) = &sequence.coding.decoder_model {
        if !model.equal_picture_interval {
            reader.bits(model.frame_presentation_time_bits)?; // frame_presentation_time
        }
    }
    Ok(())
}

/// Reads the `buffer_removal_time` of each operating point that has one. QDP
/// streams have no OBU extensions, so every frame is in temporal and spatial
/// layer 0.
fn buffer_removal_times(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
) -> Result<(), CodecError> {
    let Some(model) = &sequence.coding.decoder_model else {
        return Ok(());
    };
    if reader.bit()? {
        for &idc in &model.modelled_operating_points {
            let in_temporal_layer = idc & 1 != 0;
            let in_spatial_layer = (idc >> 8) & 1 != 0;
            if idc == 0 || (in_temporal_layer && in_spatial_layer) {
                reader.bits(model.buffer_removal_time_bits)?;
            }
        }
    }
    Ok(())
}

/// `mark_ref_frames()`: invalidates the slots whose frame IDs are too far
/// from the current one.
fn mark_ref_frames(
    refs: &mut [ReferenceSlot; NUM_REF_FRAMES],
    current: u32,
    id_bits: u32,
    delta_bits: u32,
) {
    let window = 1u32 << delta_bits;
    for slot in refs.iter_mut() {
        let id = slot.frame_id;
        let stale = if current > window {
            id > current || id < current - window
        } else {
            id > current && id < (1u32 << id_bits) + current - window
        };
        if stale {
            slot.valid = false;
        }
    }
}

/// Reads `frame_size()` and `superres_params()`.
fn frame_size(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    frame_size_override: bool,
) -> Result<FrameSize, CodecError> {
    let (width, height) = if frame_size_override {
        (
            reader.bits32(sequence.coding.frame_width_bits)? + 1,
            reader.bits32(sequence.coding.frame_height_bits)? + 1,
        )
    } else {
        (sequence.max_width, sequence.max_height)
    };
    let mut size = FrameSize {
        upscaled_width: width,
        width,
        height,
        render_width: width,
        render_height: height,
        superres_denom: SUPERRES_NUM,
    };
    superres_params(reader, sequence, &mut size)?;
    Ok(size)
}

/// Reads `superres_params()`, scaling `size.width` down from
/// `size.upscaled_width`.
fn superres_params(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    size: &mut FrameSize,
) -> Result<(), CodecError> {
    size.superres_denom = if sequence.superres && reader.bit()? {
        reader.bits(SUPERRES_DENOM_BITS)? as u8 + SUPERRES_DENOM_MIN
    } else {
        SUPERRES_NUM
    };
    let denom = u32::from(size.superres_denom);
    size.width = (size.upscaled_width * u32::from(SUPERRES_NUM) + denom / 2) / denom;
    Ok(())
}

/// Reads `render_size()`.
fn render_size(reader: &mut BitReader<'_>, size: &mut FrameSize) -> Result<(), CodecError> {
    if reader.bit()? {
        size.render_width = reader.bits32(16)? + 1;
        size.render_height = reader.bits32(16)? + 1;
    } else {
        size.render_width = size.upscaled_width;
        size.render_height = size.height;
    }
    Ok(())
}

/// Reads `frame_size_with_refs()`: the size of a reference, or a new one.
fn frame_size_with_refs(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    refs: &[ReferenceSlot; NUM_REF_FRAMES],
    ref_frame_idx: &[u8; REFS_PER_FRAME],
) -> Result<FrameSize, CodecError> {
    for &slot in ref_frame_idx {
        if reader.bit()? {
            let reference = &refs[usize::from(slot)];
            let mut size = FrameSize {
                upscaled_width: reference.size.upscaled_width,
                width: reference.size.upscaled_width,
                height: reference.size.height,
                render_width: reference.size.render_width,
                render_height: reference.size.render_height,
                superres_denom: SUPERRES_NUM,
            };
            superres_params(reader, sequence, &mut size)?;
            return Ok(size);
        }
    }
    let mut size = frame_size(reader, sequence, true)?;
    render_size(reader, &mut size)?;
    Ok(size)
}

/// Reads which slots an inter frame refers to, signalled in full or, with
/// `frame_refs_short_signaling`, derived from two of them (Section 7.8).
fn inter_frame_references(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    refs: &[ReferenceSlot; NUM_REF_FRAMES],
    header: &mut FrameHeader,
) -> Result<(), CodecError> {
    let coding = &sequence.coding;
    let short_signaling = coding.order_hint_bits > 0 && reader.bit()?;
    if short_signaling {
        let last = reader.bits(3)? as u8;
        let golden = reader.bits(3)? as u8;
        header.ref_frame_idx = set_frame_refs(
            coding.order_hint_bits,
            header.order_hint,
            refs,
            last,
            golden,
        );
    }
    for index in 0..REFS_PER_FRAME {
        if !short_signaling {
            header.ref_frame_idx[index] = reader.bits(3)? as u8;
        }
        if let Some(lengths) = coding.frame_ids {
            reader.bits(lengths.delta_frame_id_bits)?; // delta_frame_id_minus_1
        }
    }
    Ok(())
}

/// `set_frame_refs()`: the slots of every reference, from those of
/// `LAST_FRAME` and `GOLDEN_FRAME` and the order of the frames in the slots
/// (Section 7.8).
fn set_frame_refs(
    order_hint_bits: u32,
    order_hint: u32,
    refs: &[ReferenceSlot; NUM_REF_FRAMES],
    last: u8,
    golden: u8,
) -> [u8; REFS_PER_FRAME] {
    /// `LAST2_FRAME` … `ALTREF_FRAME` less `LAST_FRAME`, in the order
    /// `Ref_Frame_List` fills them.
    const REF_FRAME_LIST: [usize; REFS_PER_FRAME - 2] = [1, 2, 4, 5, 6];
    /// `GOLDEN_FRAME` less `LAST_FRAME`.
    const GOLDEN: usize = 3;
    /// `BWDREF_FRAME` less `LAST_FRAME`.
    const BWDREF: usize = 4;
    /// `ALTREF2_FRAME` less `LAST_FRAME`.
    const ALTREF2: usize = 5;
    /// `ALTREF_FRAME` less `LAST_FRAME`.
    const ALTREF: usize = 6;

    let mut chosen: [Option<u8>; REFS_PER_FRAME] = [None; REFS_PER_FRAME];
    chosen[0] = Some(last);
    chosen[GOLDEN] = Some(golden);
    let mut used = [false; NUM_REF_FRAMES];
    used[usize::from(last)] = true;
    used[usize::from(golden)] = true;
    let current = 1i32 << (order_hint_bits - 1);
    let shifted: [i32; NUM_REF_FRAMES] = std::array::from_fn(|slot| {
        current + relative_distance(order_hint_bits, refs[slot].order_hint, order_hint)
    });
    // Finds the unused slot whose hint passes `accept` and is the best by
    // `better`, the later of equals winning when `later_wins`.
    let find = |used: &[bool; NUM_REF_FRAMES],
                accept: &dyn Fn(i32) -> bool,
                better: &dyn Fn(i32, i32) -> bool|
     -> Option<usize> {
        let mut best: Option<(usize, i32)> = None;
        for slot in 0..NUM_REF_FRAMES {
            let hint = shifted[slot];
            if !used[slot] && accept(hint) && best.is_none_or(|(_, kept)| better(hint, kept)) {
                best = Some((slot, hint));
            }
        }
        best.map(|(slot, _)| slot)
    };
    let backward = |hint: i32| hint >= current;
    let forward = |hint: i32| hint < current;
    let latest = |hint: i32, kept: i32| hint >= kept;
    let earliest = |hint: i32, kept: i32| hint < kept;
    if let Some(slot) = find(&used, &backward, &latest) {
        chosen[ALTREF] = Some(slot as u8);
        used[slot] = true;
    }
    if let Some(slot) = find(&used, &backward, &earliest) {
        chosen[BWDREF] = Some(slot as u8);
        used[slot] = true;
    }
    if let Some(slot) = find(&used, &backward, &earliest) {
        chosen[ALTREF2] = Some(slot as u8);
        used[slot] = true;
    }
    for index in REF_FRAME_LIST {
        if chosen[index].is_none() {
            if let Some(slot) = find(&used, &forward, &latest) {
                chosen[index] = Some(slot as u8);
                used[slot] = true;
            }
        }
    }
    // Whatever is left refers to the earliest frame of all.
    let mut earliest_slot = 0;
    for slot in 1..NUM_REF_FRAMES {
        if shifted[slot] < shifted[earliest_slot] {
            earliest_slot = slot;
        }
    }
    chosen.map(|slot| slot.unwrap_or(earliest_slot as u8))
}

/// Returns `tile_log2(blkSize, target)`: the least `k` with `blkSize << k`
/// at least `target`.
fn tile_log2(block_size: u32, target: u32) -> u32 {
    let mut k = 0;
    while (block_size << k) < target {
        k += 1;
    }
    k
}

/// Reads `tile_info()`.
fn tile_info(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    size: &FrameSize,
) -> Result<TileInfo, CodecError> {
    let (sb_shift, sb_size) = if sequence.coding.use_128x128_superblock {
        (5, 7)
    } else {
        (4, 6)
    };
    let round = (1u32 << sb_shift) - 1;
    let sb_cols = (size.mi_cols() + round) >> sb_shift;
    let sb_rows = (size.mi_rows() + round) >> sb_shift;
    let max_tile_width_sb = MAX_TILE_WIDTH >> sb_size;
    let mut max_tile_area_sb = MAX_TILE_AREA >> (2 * sb_size);
    let min_log2_tile_cols = tile_log2(max_tile_width_sb, sb_cols);
    let max_log2_tile_cols = tile_log2(1, sb_cols.min(MAX_TILE_COLS as u32));
    let max_log2_tile_rows = tile_log2(1, sb_rows.min(MAX_TILE_ROWS as u32));
    let min_log2_tiles = min_log2_tile_cols.max(tile_log2(max_tile_area_sb, sb_rows * sb_cols));
    let mut tiles = TileInfo::default();
    if reader.bit()? {
        // uniform_tile_spacing_flag
        tiles.cols_log2 = increment(reader, min_log2_tile_cols, max_log2_tile_cols)?;
        let width_sb = (sb_cols + (1 << tiles.cols_log2) - 1) >> tiles.cols_log2;
        tiles.col_widths = split(sb_cols, width_sb);
        let min_log2_tile_rows = min_log2_tiles.saturating_sub(tiles.cols_log2);
        tiles.rows_log2 = increment(reader, min_log2_tile_rows, max_log2_tile_rows)?;
        let height_sb = (sb_rows + (1 << tiles.rows_log2) - 1) >> tiles.rows_log2;
        tiles.row_heights = split(sb_rows, height_sb);
    } else {
        let mut widest_tile_sb = 0;
        let mut start = 0;
        while start < sb_cols {
            if tiles.col_widths.len() == MAX_TILE_COLS {
                return Err(CodecError::MalformedBitstream("too many tile columns"));
            }
            let max_width = (sb_cols - start).min(max_tile_width_sb);
            let width = reader.non_symmetric(max_width)? + 1;
            widest_tile_sb = widest_tile_sb.max(width);
            tiles.col_widths.push(width);
            start += width;
        }
        tiles.cols_log2 = tile_log2(1, tiles.col_widths.len() as u32);
        max_tile_area_sb = if min_log2_tiles > 0 {
            (sb_rows * sb_cols) >> (min_log2_tiles + 1)
        } else {
            sb_rows * sb_cols
        };
        let max_tile_height_sb = (max_tile_area_sb / widest_tile_sb).max(1);
        let mut start = 0;
        while start < sb_rows {
            if tiles.row_heights.len() == MAX_TILE_ROWS {
                return Err(CodecError::MalformedBitstream("too many tile rows"));
            }
            let max_height = (sb_rows - start).min(max_tile_height_sb);
            let height = reader.non_symmetric(max_height)? + 1;
            tiles.row_heights.push(height);
            start += height;
        }
        tiles.rows_log2 = tile_log2(1, tiles.row_heights.len() as u32);
    }
    if tiles.cols_log2 > 0 || tiles.rows_log2 > 0 {
        tiles.context_update_tile_id = reader.bits32(tiles.cols_log2 + tiles.rows_log2)?;
        tiles.tile_size_bytes = reader.bits32(2)? + 1;
        if tiles.context_update_tile_id as usize >= tiles.count() {
            return Err(CodecError::MalformedBitstream(
                "context_update_tile_id names no tile",
            ));
        }
    }
    Ok(tiles)
}

/// Reads ones while the value is below `max`, starting from `min`, as
/// `increment_tile_cols_log2` and the like do.
fn increment(reader: &mut BitReader<'_>, min: u32, max: u32) -> Result<u32, CodecError> {
    let mut value = min;
    while value < max && reader.bit()? {
        value += 1;
    }
    Ok(value)
}

/// Splits `total` superblocks into tiles of `size`, the last taking what is
/// left.
fn split(total: u32, size: u32) -> Vec<u32> {
    (0..total)
        .step_by(size as usize)
        .map(|start| size.min(total - start))
        .collect()
}

/// Reads `read_delta_q()`: a quantizer delta, if coded.
fn read_delta_q(reader: &mut BitReader<'_>) -> Result<i8, CodecError> {
    Ok(if reader.bit()? {
        reader.signed(DELTA_Q_BITS)? as i8
    } else {
        0
    })
}

/// Reads `quantization_params()`.
fn quantization_params(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    num_planes: usize,
) -> Result<Quantization, CodecError> {
    let separate_uv_delta_q = sequence.coding.separate_uv_delta_q;
    let mut quantization = Quantization {
        base_q_idx: reader.bits(8)? as u8,
        delta_q_y_dc: read_delta_q(reader)?,
        ..Quantization::default()
    };
    if num_planes > 1 {
        let diff_uv_delta = separate_uv_delta_q && reader.bit()?;
        quantization.delta_q_u_dc = read_delta_q(reader)?;
        quantization.delta_q_u_ac = read_delta_q(reader)?;
        if diff_uv_delta {
            quantization.delta_q_v_dc = read_delta_q(reader)?;
            quantization.delta_q_v_ac = read_delta_q(reader)?;
        } else {
            quantization.delta_q_v_dc = quantization.delta_q_u_dc;
            quantization.delta_q_v_ac = quantization.delta_q_u_ac;
        }
    }
    if reader.bit()? {
        let y = reader.bits(4)? as u8;
        let u = reader.bits(4)? as u8;
        let v = if separate_uv_delta_q {
            reader.bits(4)? as u8
        } else {
            u
        };
        quantization.matrices = Some(QuantizerMatrices { y, u, v });
    }
    Ok(quantization)
}

/// Reads `segmentation_params()`, starting from the primary reference's
/// features, `previous`, if the frame has one.
fn segmentation_params(
    reader: &mut BitReader<'_>,
    primary_ref_frame: u8,
    previous: Option<SegmentFeatures>,
) -> Result<Segmentation, CodecError> {
    let mut segmentation = Segmentation {
        enabled: reader.bit()?,
        ..Segmentation::default()
    };
    if !segmentation.enabled {
        return Ok(segmentation);
    }
    if primary_ref_frame == PRIMARY_REF_NONE {
        segmentation.update_map = true;
        segmentation.update_data = true;
    } else {
        segmentation.update_map = reader.bit()?;
        segmentation.temporal_update = segmentation.update_map && reader.bit()?;
        segmentation.update_data = reader.bit()?;
    }
    if !segmentation.update_data {
        segmentation.features = previous.unwrap_or_default();
        return Ok(segmentation);
    }
    for segment in 0..MAX_SEGMENTS {
        for feature in 0..SEG_LVL_MAX {
            if !reader.bit()? {
                continue;
            }
            segmentation.features.enabled[segment] |= 1 << feature;
            let bits = SEGMENT_FEATURE_BITS[feature];
            let limit = SEGMENT_FEATURE_MAX[feature];
            let value = if SEGMENT_FEATURE_SIGNED[feature] {
                reader.signed(1 + bits)?.clamp(-limit, limit)
            } else {
                (reader.bits32(bits)? as i32).clamp(0, limit)
            };
            segmentation.features.data[segment][feature] = value as i16;
        }
    }
    Ok(segmentation)
}

/// Reads `delta_q_params()` and `delta_lf_params()`.
fn delta_params(
    reader: &mut BitReader<'_>,
    header: &FrameHeader,
) -> Result<DeltaParams, CodecError> {
    let mut delta = DeltaParams::default();
    if header.quantization.base_q_idx > 0 {
        delta.q_present = reader.bit()?;
    }
    if delta.q_present {
        delta.q_res = reader.bits(2)? as u8;
        if !header.allow_intrabc {
            delta.lf_present = reader.bit()?;
        }
        if delta.lf_present {
            delta.lf_res = reader.bits(2)? as u8;
            delta.lf_multi = reader.bit()?;
        }
    }
    Ok(delta)
}

/// Returns `CodedLossless`: whether every segment's quantizer is 0, with no
/// deltas.
fn coded_lossless(header: &FrameHeader) -> bool {
    let quantization = &header.quantization;
    let deltas = [
        quantization.delta_q_y_dc,
        quantization.delta_q_u_dc,
        quantization.delta_q_u_ac,
        quantization.delta_q_v_dc,
        quantization.delta_q_v_ac,
    ];
    if deltas.iter().any(|&delta| delta != 0) {
        return false;
    }
    let segmentation = &header.segmentation;
    (0..MAX_SEGMENTS).all(|segment| {
        let base = i32::from(quantization.base_q_idx);
        let qindex =
            if segmentation.enabled && segmentation.features.is_enabled(segment, SEG_LVL_ALT_Q) {
                (base + i32::from(segmentation.features.data[segment][SEG_LVL_ALT_Q])).clamp(0, 255)
            } else {
                base
            };
        qindex == 0
    })
}

/// Reads `loop_filter_params()`, starting from the primary reference's
/// deltas, or the defaults, in `start`.
fn loop_filter_params(
    reader: &mut BitReader<'_>,
    header: &FrameHeader,
    num_planes: usize,
    start: LoopFilter,
) -> Result<LoopFilter, CodecError> {
    if header.coded_lossless || header.allow_intrabc {
        return Ok(LoopFilter::default());
    }
    let mut filter = start;
    filter.levels[0] = reader.bits(LOOP_FILTER_LEVEL_BITS)? as u8;
    filter.levels[1] = reader.bits(LOOP_FILTER_LEVEL_BITS)? as u8;
    if num_planes > 1 && (filter.levels[0] != 0 || filter.levels[1] != 0) {
        filter.levels[2] = reader.bits(LOOP_FILTER_LEVEL_BITS)? as u8;
        filter.levels[3] = reader.bits(LOOP_FILTER_LEVEL_BITS)? as u8;
    }
    filter.sharpness = reader.bits(3)? as u8;
    filter.delta_enabled = reader.bit()?;
    if filter.delta_enabled {
        filter.delta_update = reader.bit()?;
        if filter.delta_update {
            for delta in &mut filter.ref_deltas {
                if reader.bit()? {
                    *delta = reader.signed(LOOP_FILTER_DELTA_BITS)? as i8;
                }
            }
            for delta in &mut filter.mode_deltas {
                if reader.bit()? {
                    *delta = reader.signed(LOOP_FILTER_DELTA_BITS)? as i8;
                }
            }
        }
    }
    Ok(filter)
}

/// Reads `cdef_params()`.
fn cdef_params(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    header: &FrameHeader,
    num_planes: usize,
) -> Result<Cdef, CodecError> {
    let mut cdef = Cdef::default();
    if header.coded_lossless || header.allow_intrabc || !sequence.coding.enable_cdef {
        return Ok(cdef);
    }
    cdef.damping_minus_3 = reader.bits(2)? as u8;
    cdef.bits = reader.bits(2)? as u8;
    for index in 0..1 << cdef.bits {
        cdef.y_primary[index] = reader.bits(4)? as u8;
        cdef.y_secondary[index] = reader.bits(2)? as u8;
        if num_planes > 1 {
            cdef.uv_primary[index] = reader.bits(4)? as u8;
            cdef.uv_secondary[index] = reader.bits(2)? as u8;
        }
    }
    Ok(cdef)
}

/// Reads `lr_params()`.
fn lr_params(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    header: &FrameHeader,
    num_planes: usize,
) -> Result<Restoration, CodecError> {
    let mut restoration = Restoration::default();
    if header.all_lossless || header.allow_intrabc || !sequence.coding.enable_restoration {
        return Ok(restoration);
    }
    for plane in 0..num_planes {
        restoration.types[plane] = REMAP_LR_TYPE[reader.bits(2)? as usize];
    }
    if !restoration.is_used() {
        return Ok(restoration);
    }
    let shift = if sequence.coding.use_128x128_superblock {
        reader.bits(1)? as u8 + 1
    } else {
        let shift = reader.bits(1)? as u8;
        if shift == 1 {
            shift + reader.bits(1)? as u8
        } else {
            shift
        }
    };
    let uses_chroma = restoration.types[1..]
        .iter()
        .any(|&kind| kind != RestorationType::None);
    let uv_shift = if sequence.subsampling_x && sequence.subsampling_y && uses_chroma {
        reader.bits(1)? as u8
    } else {
        0
    };
    let luma = RESTORATION_UNIT_LOG2_BASE + shift;
    restoration.unit_size_log2 = [luma, luma - uv_shift, luma - uv_shift];
    Ok(restoration)
}

/// Returns `skipModeAllowed`: whether the frame has references before it,
/// and either after it or before those, for skip mode to predict from.
fn skip_mode_allowed(
    sequence: &SequenceHeader,
    refs: &[ReferenceSlot; NUM_REF_FRAMES],
    header: &FrameHeader,
) -> bool {
    let bits = sequence.coding.order_hint_bits;
    if header.frame_type.is_intra() || !header.reference_select || bits == 0 {
        return false;
    }
    let hints = header
        .ref_frame_idx
        .map(|slot| refs[usize::from(slot)].order_hint);
    let distance = |a: u32, b: u32| relative_distance(bits, a, b);
    let mut forward: Option<u32> = None;
    let mut backward: Option<u32> = None;
    for &hint in &hints {
        let from_current = distance(hint, header.order_hint);
        if from_current < 0 {
            if forward.is_none_or(|kept| distance(hint, kept) > 0) {
                forward = Some(hint);
            }
        } else if from_current > 0 && backward.is_none_or(|kept| distance(hint, kept) < 0) {
            backward = Some(hint);
        }
    }
    let Some(forward) = forward else {
        return false;
    };
    if backward.is_some() {
        return true;
    }
    // A second reference before the first one also allows skip mode.
    hints.iter().any(|&hint| distance(hint, forward) < 0)
}
