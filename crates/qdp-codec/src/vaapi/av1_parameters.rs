//! The structures a VA-API AV1 decoder is given for each frame, as
//! `va_dec_av1.h` declares them (`VADecPictureParameterBufferAV1` and
//! `VASliceParameterBufferAV1`), filled from a parsed frame header as FFmpeg's
//! VA-API AV1 decoder fills them.
//!
//! The C structures' bit fields are kept as the integers that hold them,
//! with the bits set by the shifts below; the layouts are checked against the
//! offsets a C compiler gives `va_dec_av1.h`.

use std::ffi::c_int;
use std::ops::Range;

use super::ffi::{VA_INVALID_ID, VaId, Zeroed};
use crate::frame_parser::{FrameType, NUM_REF_FRAMES, ParsedFrame, REFS_PER_FRAME};
use crate::sequence::SequenceHeader;

/// The `matrix_coefficients` of a stream without a colour description:
/// unspecified.
const MATRIX_UNSPECIFIED: u8 = 2;

/// The width and height arrays' entries: the last tile's is implied.
const TILE_SIZES: usize = 63;

/// The `log2` of the restoration unit size `lr_unit_shift` 0 stands for.
const RESTORATION_UNIT_LOG2_BASE: u8 = 6;

/// `VA_SLICE_DATA_FLAG_ALL`: a tile's data is all in one buffer.
const TILE_DATA_WHOLE: u32 = 0;

/// The bits of `seq_info_fields`.
mod sequence_bits {
    /// `still_picture`.
    pub(super) const STILL_PICTURE: u32 = 1 << 0;
    /// `use_128x128_superblock`.
    pub(super) const USE_128X128_SUPERBLOCK: u32 = 1 << 1;
    /// `enable_filter_intra`.
    pub(super) const ENABLE_FILTER_INTRA: u32 = 1 << 2;
    /// `enable_intra_edge_filter`.
    pub(super) const ENABLE_INTRA_EDGE_FILTER: u32 = 1 << 3;
    /// `enable_interintra_compound`.
    pub(super) const ENABLE_INTERINTRA_COMPOUND: u32 = 1 << 4;
    /// `enable_masked_compound`.
    pub(super) const ENABLE_MASKED_COMPOUND: u32 = 1 << 5;
    /// `enable_dual_filter`.
    pub(super) const ENABLE_DUAL_FILTER: u32 = 1 << 6;
    /// `enable_order_hint`.
    pub(super) const ENABLE_ORDER_HINT: u32 = 1 << 7;
    /// `enable_jnt_comp`.
    pub(super) const ENABLE_JNT_COMP: u32 = 1 << 8;
    /// `enable_cdef`.
    pub(super) const ENABLE_CDEF: u32 = 1 << 9;
    /// `mono_chrome`.
    pub(super) const MONO_CHROME: u32 = 1 << 10;
    /// `color_range`.
    pub(super) const COLOR_RANGE: u32 = 1 << 11;
    /// `subsampling_x`.
    pub(super) const SUBSAMPLING_X: u32 = 1 << 12;
    /// `subsampling_y`.
    pub(super) const SUBSAMPLING_Y: u32 = 1 << 13;
    /// `chroma_sample_position`.
    pub(super) const CHROMA_SAMPLE_POSITION: u32 = 1 << 14;
    /// `film_grain_params_present`.
    pub(super) const FILM_GRAIN_PARAMS_PRESENT: u32 = 1 << 15;
}

/// The bits of `pic_info_fields`.
mod picture_bits {
    /// The shift of `frame_type`, two bits.
    pub(super) const FRAME_TYPE_SHIFT: u32 = 0;
    /// `show_frame`.
    pub(super) const SHOW_FRAME: u32 = 1 << 2;
    /// `showable_frame`.
    pub(super) const SHOWABLE_FRAME: u32 = 1 << 3;
    /// `error_resilient_mode`.
    pub(super) const ERROR_RESILIENT_MODE: u32 = 1 << 4;
    /// `disable_cdf_update`.
    pub(super) const DISABLE_CDF_UPDATE: u32 = 1 << 5;
    /// `allow_screen_content_tools`.
    pub(super) const ALLOW_SCREEN_CONTENT_TOOLS: u32 = 1 << 6;
    /// `force_integer_mv`.
    pub(super) const FORCE_INTEGER_MV: u32 = 1 << 7;
    /// `allow_intrabc`.
    pub(super) const ALLOW_INTRABC: u32 = 1 << 8;
    /// `use_superres`.
    pub(super) const USE_SUPERRES: u32 = 1 << 9;
    /// `allow_high_precision_mv`.
    pub(super) const ALLOW_HIGH_PRECISION_MV: u32 = 1 << 10;
    /// `is_motion_mode_switchable`.
    pub(super) const IS_MOTION_MODE_SWITCHABLE: u32 = 1 << 11;
    /// `use_ref_frame_mvs`.
    pub(super) const USE_REF_FRAME_MVS: u32 = 1 << 12;
    /// `disable_frame_end_update_cdf`.
    pub(super) const DISABLE_FRAME_END_UPDATE_CDF: u32 = 1 << 13;
    /// `uniform_tile_spacing_flag`.
    pub(super) const UNIFORM_TILE_SPACING: u32 = 1 << 14;
    /// `allow_warped_motion`.
    pub(super) const ALLOW_WARPED_MOTION: u32 = 1 << 15;
}

/// The bits of `loop_filter_info_fields`.
mod loop_filter_bits {
    /// The shift of `sharpness_level`, three bits.
    pub(super) const SHARPNESS_SHIFT: u8 = 0;
    /// `mode_ref_delta_enabled`.
    pub(super) const MODE_REF_DELTA_ENABLED: u8 = 1 << 3;
    /// `mode_ref_delta_update`.
    pub(super) const MODE_REF_DELTA_UPDATE: u8 = 1 << 4;
}

/// The bits of `qmatrix_fields`.
mod quantizer_matrix_bits {
    /// `using_qmatrix`.
    pub(super) const USING_QMATRIX: u16 = 1 << 0;
    /// The shift of `qm_y`, four bits.
    pub(super) const QM_Y_SHIFT: u16 = 1;
    /// The shift of `qm_u`, four bits.
    pub(super) const QM_U_SHIFT: u16 = 5;
    /// The shift of `qm_v`, four bits.
    pub(super) const QM_V_SHIFT: u16 = 9;
}

/// The bits of `mode_control_fields`.
mod mode_bits {
    /// `delta_q_present_flag`.
    pub(super) const DELTA_Q_PRESENT: u32 = 1 << 0;
    /// The shift of `log2_delta_q_res`, two bits.
    pub(super) const DELTA_Q_RES_SHIFT: u32 = 1;
    /// `delta_lf_present_flag`.
    pub(super) const DELTA_LF_PRESENT: u32 = 1 << 3;
    /// The shift of `log2_delta_lf_res`, two bits.
    pub(super) const DELTA_LF_RES_SHIFT: u32 = 4;
    /// `delta_lf_multi`.
    pub(super) const DELTA_LF_MULTI: u32 = 1 << 6;
    /// The shift of `tx_mode`, two bits.
    pub(super) const TX_MODE_SHIFT: u32 = 7;
    /// `reference_select`.
    pub(super) const REFERENCE_SELECT: u32 = 1 << 9;
    /// `reduced_tx_set_used`.
    pub(super) const REDUCED_TX_SET_USED: u32 = 1 << 10;
    /// `skip_mode_present`.
    pub(super) const SKIP_MODE_PRESENT: u32 = 1 << 11;
}

/// The bits of `loop_restoration_fields`.
mod restoration_bits {
    /// The shift of `yframe_restoration_type`, two bits; the Cb and Cr
    /// planes' follow, two bits each.
    pub(super) const TYPE_SHIFT: u16 = 0;
    /// The bits each plane's restoration type takes.
    pub(super) const TYPE_BITS: u16 = 2;
    /// The shift of `lr_unit_shift`, two bits.
    pub(super) const UNIT_SHIFT_SHIFT: u16 = 6;
    /// `lr_uv_shift`.
    pub(super) const UV_SHIFT: u16 = 1 << 8;
}

/// The bits of `segment_info_fields`.
mod segmentation_bits {
    /// `enabled`.
    pub(super) const ENABLED: u32 = 1 << 0;
    /// `update_map`.
    pub(super) const UPDATE_MAP: u32 = 1 << 1;
    /// `temporal_update`.
    pub(super) const TEMPORAL_UPDATE: u32 = 1 << 2;
    /// `update_data`.
    pub(super) const UPDATE_DATA: u32 = 1 << 3;
}

/// `VASegmentationStructAV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct Segmentation {
    /// `segment_info_fields`.
    pub(super) info: u32,
    /// `feature_data`.
    pub(super) feature_data: [[i16; 8]; 8],
    /// `feature_mask`.
    pub(super) feature_mask: [u8; 8],
    /// `va_reserved`.
    va_reserved: [u32; 4],
}

/// `VAWarpedMotionParamsAV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct WarpedMotion {
    /// `wmtype`.
    pub(super) wmtype: c_int,
    /// `wmmat`: the six parameters, then two unused.
    pub(super) wmmat: [i32; 8],
    /// `invalid`.
    pub(super) invalid: u8,
    /// `va_reserved`.
    va_reserved: [u32; 4],
}

/// `VADecPictureParameterBufferAV1`: everything about a frame but its tiles'
/// data.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct PictureParameters {
    /// `profile`.
    pub(super) profile: u8,
    /// `order_hint_bits_minus_1`.
    pub(super) order_hint_bits_minus_1: u8,
    /// `bit_depth_idx`: 0 for 8 bits, 1 for 10, 2 for 12.
    pub(super) bit_depth_idx: u8,
    /// `matrix_coefficients`.
    pub(super) matrix_coefficients: u8,
    /// `seq_info_fields`.
    pub(super) seq_info_fields: u32,
    /// `current_frame`: the surface the frame is decoded into.
    pub(super) current_frame: VaId,
    /// `current_display_picture`: the surface shown, which differs only for
    /// frames with film grain.
    pub(super) current_display_picture: VaId,
    /// `anchor_frames_num`, for large-scale tiles.
    pub(super) anchor_frames_num: u8,
    /// `anchor_frames_list`, for large-scale tiles.
    pub(super) anchor_frames_list: *mut VaId,
    /// `frame_width_minus1`: the width after super-resolution.
    pub(super) frame_width_minus1: u16,
    /// `frame_height_minus1`.
    pub(super) frame_height_minus1: u16,
    /// `output_frame_width_in_tiles_minus_1`, for large-scale tiles.
    pub(super) output_frame_width_in_tiles_minus_1: u16,
    /// `output_frame_height_in_tiles_minus_1`, for large-scale tiles.
    pub(super) output_frame_height_in_tiles_minus_1: u16,
    /// `ref_frame_map`: the surface each reference slot holds.
    pub(super) ref_frame_map: [VaId; NUM_REF_FRAMES],
    /// `ref_frame_idx`.
    pub(super) ref_frame_idx: [u8; REFS_PER_FRAME],
    /// `primary_ref_frame`.
    pub(super) primary_ref_frame: u8,
    /// `order_hint`.
    pub(super) order_hint: u8,
    /// `seg_info`.
    pub(super) seg_info: Segmentation,
    /// `film_grain_info`: all zero, as QDP streams have no film grain
    /// (Section 6.4).
    film_grain_info: [u32; 44],
    /// `tile_cols`.
    pub(super) tile_cols: u8,
    /// `tile_rows`.
    pub(super) tile_rows: u8,
    /// `width_in_sbs_minus_1`.
    pub(super) width_in_sbs_minus_1: [u16; TILE_SIZES],
    /// `height_in_sbs_minus_1`.
    pub(super) height_in_sbs_minus_1: [u16; TILE_SIZES],
    /// `tile_count_minus_1`, for large-scale tiles.
    pub(super) tile_count_minus_1: u16,
    /// `context_update_tile_id`.
    pub(super) context_update_tile_id: u16,
    /// `pic_info_fields`.
    pub(super) pic_info_fields: u32,
    /// `superres_scale_denominator`.
    pub(super) superres_scale_denominator: u8,
    /// `interp_filter`.
    pub(super) interp_filter: u8,
    /// `filter_level`: the luma levels, vertical and horizontal.
    pub(super) filter_level: [u8; 2],
    /// `filter_level_u`.
    pub(super) filter_level_u: u8,
    /// `filter_level_v`.
    pub(super) filter_level_v: u8,
    /// `loop_filter_info_fields`.
    pub(super) loop_filter_info_fields: u8,
    /// `ref_deltas`.
    pub(super) ref_deltas: [i8; NUM_REF_FRAMES],
    /// `mode_deltas`.
    pub(super) mode_deltas: [i8; 2],
    /// `base_qindex`.
    pub(super) base_qindex: u8,
    /// `y_dc_delta_q`.
    pub(super) y_dc_delta_q: i8,
    /// `u_dc_delta_q`.
    pub(super) u_dc_delta_q: i8,
    /// `u_ac_delta_q`.
    pub(super) u_ac_delta_q: i8,
    /// `v_dc_delta_q`.
    pub(super) v_dc_delta_q: i8,
    /// `v_ac_delta_q`.
    pub(super) v_ac_delta_q: i8,
    /// `qmatrix_fields`.
    pub(super) qmatrix_fields: u16,
    /// `mode_control_fields`.
    pub(super) mode_control_fields: u32,
    /// `cdef_damping_minus_3`.
    pub(super) cdef_damping_minus_3: u8,
    /// `cdef_bits`.
    pub(super) cdef_bits: u8,
    /// `cdef_y_strengths`: each primary strength shifted by two, with the
    /// secondary as coded below it.
    pub(super) cdef_y_strengths: [u8; 8],
    /// `cdef_uv_strengths`, as the luma ones.
    pub(super) cdef_uv_strengths: [u8; 8],
    /// `loop_restoration_fields`.
    pub(super) loop_restoration_fields: u16,
    /// `wm`: the global motion of `LAST_FRAME` … `ALTREF_FRAME`.
    pub(super) wm: [WarpedMotion; REFS_PER_FRAME],
    /// `va_reserved`.
    va_reserved: [u32; 8],
}

/// `VASliceParameterBufferAV1`: where a tile's data lies in its tile
/// group's data.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TileParameters {
    /// `slice_data_size`.
    pub(super) slice_data_size: u32,
    /// `slice_data_offset`.
    pub(super) slice_data_offset: u32,
    /// `slice_data_flag`.
    pub(super) slice_data_flag: u32,
    /// `tile_row`.
    pub(super) tile_row: u16,
    /// `tile_column`.
    pub(super) tile_column: u16,
    /// `tg_start`: the tile group's first tile.
    pub(super) tg_start: u16,
    /// `tg_end`: the tile group's last tile.
    pub(super) tg_end: u16,
    /// `anchor_frame_idx`, for large-scale tiles.
    pub(super) anchor_frame_idx: u8,
    /// `tile_idx_in_tile_list`, for large-scale tiles.
    pub(super) tile_idx_in_tile_list: u16,
    /// `va_reserved`.
    va_reserved: [u32; 4],
}

impl Zeroed for PictureParameters {}
impl Zeroed for TileParameters {}

/// One tile group of a frame: where its data lies in the temporal unit, and
/// where each of its tiles lies in that data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct TileGroup {
    /// The group's data in the temporal unit.
    pub(super) data: Range<usize>,
    /// Its tiles.
    pub(super) tiles: Vec<TileParameters>,
}

/// Returns `bit` if `set`, or 0.
fn flag<T: Default>(set: bool, bit: T) -> T {
    if set { bit } else { T::default() }
}

/// Returns the sequence's bits of `seq_info_fields`.
fn sequence_fields(sequence: &SequenceHeader) -> u32 {
    use sequence_bits::*;
    let coding = &sequence.coding;
    flag(coding.still_picture, STILL_PICTURE)
        | flag(coding.use_128x128_superblock, USE_128X128_SUPERBLOCK)
        | flag(coding.enable_filter_intra, ENABLE_FILTER_INTRA)
        | flag(coding.enable_intra_edge_filter, ENABLE_INTRA_EDGE_FILTER)
        | flag(coding.enable_interintra_compound, ENABLE_INTERINTRA_COMPOUND)
        | flag(coding.enable_masked_compound, ENABLE_MASKED_COMPOUND)
        | flag(coding.enable_dual_filter, ENABLE_DUAL_FILTER)
        | flag(coding.order_hint_bits > 0, ENABLE_ORDER_HINT)
        | flag(coding.enable_jnt_comp, ENABLE_JNT_COMP)
        | flag(coding.enable_cdef, ENABLE_CDEF)
        | flag(sequence.monochrome, MONO_CHROME)
        | flag(sequence.full_range, COLOR_RANGE)
        | flag(sequence.subsampling_x, SUBSAMPLING_X)
        | flag(sequence.subsampling_y, SUBSAMPLING_Y)
        // A one-bit field, which takes the low bit of the two coded, as C
        // assigns it.
        | flag(sequence.chroma_sample_position & 1 != 0, CHROMA_SAMPLE_POSITION)
        | flag(sequence.film_grain, FILM_GRAIN_PARAMS_PRESENT)
}

impl PictureParameters {
    /// Returns the parameters of `frame`, a frame of `sequence` that is not
    /// an existing frame shown again, decoded into surface `current`, when
    /// each reference slot holds the frame of the surface `slots` names, or
    /// [`VA_INVALID_ID`].
    pub(super) fn new(
        sequence: &SequenceHeader,
        frame: &ParsedFrame,
        current: VaId,
        slots: &[VaId; NUM_REF_FRAMES],
    ) -> Self {
        let header = &frame.header;
        let size = &header.size;
        let mut params = Self::zeroed();
        params.profile = sequence.profile;
        params.order_hint_bits_minus_1 = sequence.coding.order_hint_bits.saturating_sub(1) as u8;
        params.bit_depth_idx = sequence.bit_depth.saturating_sub(8) / 2;
        params.matrix_coefficients = sequence
            .color_description
            .map_or(MATRIX_UNSPECIFIED, |color| color.matrix);
        params.seq_info_fields = sequence_fields(sequence);
        params.current_frame = current;
        params.current_display_picture = current;
        params.frame_width_minus1 = size.upscaled_width.saturating_sub(1) as u16;
        params.frame_height_minus1 = size.height.saturating_sub(1) as u16;
        // A key frame shown resets every slot, so it refers to none.
        params.ref_frame_map = if header.frame_type == FrameType::Key && header.show_frame {
            [VA_INVALID_ID; NUM_REF_FRAMES]
        } else {
            *slots
        };
        params.ref_frame_idx = header.ref_frame_idx;
        params.primary_ref_frame = header.primary_ref_frame;
        params.order_hint = header.order_hint as u8;
        params.seg_info = segmentation(frame);
        let tiles = &header.tiles;
        params.tile_cols = tiles.cols() as u8;
        params.tile_rows = tiles.rows() as u8;
        for (target, &width) in params
            .width_in_sbs_minus_1
            .iter_mut()
            .zip(&tiles.col_widths)
        {
            *target = width.saturating_sub(1) as u16;
        }
        for (target, &height) in params
            .height_in_sbs_minus_1
            .iter_mut()
            .zip(&tiles.row_heights)
        {
            *target = height.saturating_sub(1) as u16;
        }
        params.context_update_tile_id = tiles.context_update_tile_id as u16;
        params.pic_info_fields = picture_fields(frame);
        params.superres_scale_denominator = size.superres_denom;
        params.interp_filter = header.interpolation_filter;
        let filter = &header.loop_filter;
        params.filter_level = [filter.levels[0], filter.levels[1]];
        params.filter_level_u = filter.levels[2];
        params.filter_level_v = filter.levels[3];
        params.loop_filter_info_fields = filter.sharpness << loop_filter_bits::SHARPNESS_SHIFT
            | flag(
                filter.delta_enabled,
                loop_filter_bits::MODE_REF_DELTA_ENABLED,
            )
            | flag(filter.delta_update, loop_filter_bits::MODE_REF_DELTA_UPDATE);
        params.ref_deltas = filter.ref_deltas;
        params.mode_deltas = filter.mode_deltas;
        let quantization = &header.quantization;
        params.base_qindex = quantization.base_q_idx;
        params.y_dc_delta_q = quantization.delta_q_y_dc;
        params.u_dc_delta_q = quantization.delta_q_u_dc;
        params.u_ac_delta_q = quantization.delta_q_u_ac;
        params.v_dc_delta_q = quantization.delta_q_v_dc;
        params.v_ac_delta_q = quantization.delta_q_v_ac;
        params.qmatrix_fields = quantization.matrices.map_or(0, |matrices| {
            use quantizer_matrix_bits::*;
            USING_QMATRIX
                | u16::from(matrices.y) << QM_Y_SHIFT
                | u16::from(matrices.u) << QM_U_SHIFT
                | u16::from(matrices.v) << QM_V_SHIFT
        });
        params.mode_control_fields = mode_fields(frame);
        let cdef = &header.cdef;
        params.cdef_damping_minus_3 = cdef.damping_minus_3;
        params.cdef_bits = cdef.bits;
        for index in 0..1usize << cdef.bits {
            params.cdef_y_strengths[index] = cdef.y_primary[index] << 2 | cdef.y_secondary[index];
            params.cdef_uv_strengths[index] =
                cdef.uv_primary[index] << 2 | cdef.uv_secondary[index];
        }
        params.loop_restoration_fields = restoration_fields(frame);
        params.wm = std::array::from_fn(|index| {
            let motion = &header.global_motion[index + 1];
            let mut wmmat = [0; 8];
            wmmat[..6].copy_from_slice(&motion.params);
            WarpedMotion {
                wmtype: motion.model as c_int,
                wmmat,
                invalid: u8::from(motion.invalid),
                va_reserved: [0; 4],
            }
        });
        params
    }
}

/// Returns the frame's bits of `pic_info_fields`.
fn picture_fields(frame: &ParsedFrame) -> u32 {
    use picture_bits::*;
    let header = &frame.header;
    (header.frame_type as u32) << FRAME_TYPE_SHIFT
        | flag(header.show_frame, SHOW_FRAME)
        | flag(header.showable_frame, SHOWABLE_FRAME)
        | flag(header.error_resilient_mode, ERROR_RESILIENT_MODE)
        | flag(header.disable_cdf_update, DISABLE_CDF_UPDATE)
        | flag(
            header.allow_screen_content_tools,
            ALLOW_SCREEN_CONTENT_TOOLS,
        )
        | flag(header.force_integer_mv, FORCE_INTEGER_MV)
        | flag(header.allow_intrabc, ALLOW_INTRABC)
        | flag(header.size.uses_superres(), USE_SUPERRES)
        | flag(header.allow_high_precision_mv, ALLOW_HIGH_PRECISION_MV)
        | flag(header.is_motion_mode_switchable, IS_MOTION_MODE_SWITCHABLE)
        | flag(header.use_ref_frame_mvs, USE_REF_FRAME_MVS)
        | flag(
            header.disable_frame_end_update_cdf,
            DISABLE_FRAME_END_UPDATE_CDF,
        )
        | flag(header.tiles.uniform, UNIFORM_TILE_SPACING)
        | flag(header.allow_warped_motion, ALLOW_WARPED_MOTION)
}

/// Returns the frame's bits of `mode_control_fields`.
fn mode_fields(frame: &ParsedFrame) -> u32 {
    use mode_bits::*;
    let header = &frame.header;
    let delta = &header.delta;
    flag(delta.q_present, DELTA_Q_PRESENT)
        | u32::from(delta.q_res) << DELTA_Q_RES_SHIFT
        | flag(delta.lf_present, DELTA_LF_PRESENT)
        | u32::from(delta.lf_res) << DELTA_LF_RES_SHIFT
        | flag(delta.lf_multi, DELTA_LF_MULTI)
        | (header.tx_mode as u32) << TX_MODE_SHIFT
        | flag(header.reference_select, REFERENCE_SELECT)
        | flag(header.reduced_tx_set, REDUCED_TX_SET_USED)
        | flag(header.skip_mode_present, SKIP_MODE_PRESENT)
}

/// Returns the frame's `loop_restoration_fields`: each plane's type, and
/// the unit sizes as the shifts that code them.
fn restoration_fields(frame: &ParsedFrame) -> u16 {
    use restoration_bits::*;
    let restoration = &frame.header.restoration;
    let types = restoration
        .types
        .iter()
        .enumerate()
        .fold(0, |fields, (plane, &kind)| {
            fields | (kind as u16) << (TYPE_SHIFT + TYPE_BITS * plane as u16)
        });
    if !restoration.is_used() {
        return types;
    }
    let [luma, chroma, _] = restoration.unit_size_log2;
    let unit_shift = u16::from(luma.saturating_sub(RESTORATION_UNIT_LOG2_BASE));
    types | unit_shift << UNIT_SHIFT_SHIFT | flag(chroma < luma, UV_SHIFT)
}

/// Returns the frame's `seg_info`.
fn segmentation(frame: &ParsedFrame) -> Segmentation {
    use segmentation_bits::*;
    let segmentation = &frame.header.segmentation;
    Segmentation {
        info: flag(segmentation.enabled, ENABLED)
            | flag(segmentation.update_map, UPDATE_MAP)
            | flag(segmentation.temporal_update, TEMPORAL_UPDATE)
            | flag(segmentation.update_data, UPDATE_DATA),
        feature_data: segmentation.features.data,
        feature_mask: segmentation.features.enabled,
        va_reserved: [0; 4],
    }
}

/// Returns the frame's tile groups, each with its tiles' places in its data.
pub(super) fn tile_groups(frame: &ParsedFrame) -> Vec<TileGroup> {
    let mut first = 0;
    frame
        .tile_data
        .iter()
        .map(|data| {
            let count = frame.tiles[first..]
                .iter()
                .take_while(|tile| data.contains(&tile.offset))
                .count();
            let (start, end) = (first, first + count.saturating_sub(1));
            let tiles = frame.tiles[first..first + count]
                .iter()
                .map(|tile| TileParameters {
                    slice_data_size: tile.size as u32,
                    slice_data_offset: (tile.offset - data.start) as u32,
                    slice_data_flag: TILE_DATA_WHOLE,
                    tile_row: tile.row,
                    tile_column: tile.column,
                    tg_start: start as u16,
                    tg_end: end as u16,
                    ..TileParameters::zeroed()
                })
                .collect();
            first += count;
            TileGroup {
                data: data.clone(),
                tiles,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_parser::FrameParser;
    use crate::{Av1Encoder, EncoderSettings, FASTEST_SPEED, KeyFrameRequest, Yuv420Frame};
    use std::mem::{offset_of, size_of};

    /// The structures match the C header's layouts, as `sizeof` and
    /// `offsetof` report them for libva 2.24 on x86-64 Linux.
    #[test]
    fn layouts_match_the_c_header() {
        assert_eq!(size_of::<Segmentation>(), 156);
        assert_eq!(offset_of!(Segmentation, feature_data), 4);
        assert_eq!(offset_of!(Segmentation, feature_mask), 132);
        assert_eq!(size_of::<WarpedMotion>(), 56);
        assert_eq!(offset_of!(WarpedMotion, invalid), 36);
        assert_eq!(size_of::<PictureParameters>(), 1160);
        assert_eq!(offset_of!(PictureParameters, seq_info_fields), 4);
        assert_eq!(offset_of!(PictureParameters, current_frame), 8);
        assert_eq!(offset_of!(PictureParameters, anchor_frames_list), 24);
        assert_eq!(offset_of!(PictureParameters, frame_width_minus1), 32);
        assert_eq!(offset_of!(PictureParameters, ref_frame_map), 40);
        assert_eq!(offset_of!(PictureParameters, ref_frame_idx), 72);
        assert_eq!(offset_of!(PictureParameters, primary_ref_frame), 79);
        assert_eq!(offset_of!(PictureParameters, order_hint), 80);
        assert_eq!(offset_of!(PictureParameters, seg_info), 84);
        assert_eq!(offset_of!(PictureParameters, film_grain_info), 240);
        assert_eq!(offset_of!(PictureParameters, tile_cols), 416);
        assert_eq!(offset_of!(PictureParameters, width_in_sbs_minus_1), 418);
        assert_eq!(offset_of!(PictureParameters, height_in_sbs_minus_1), 544);
        assert_eq!(offset_of!(PictureParameters, tile_count_minus_1), 670);
        assert_eq!(offset_of!(PictureParameters, context_update_tile_id), 672);
        assert_eq!(offset_of!(PictureParameters, pic_info_fields), 676);
        assert_eq!(
            offset_of!(PictureParameters, superres_scale_denominator),
            680
        );
        assert_eq!(offset_of!(PictureParameters, filter_level), 682);
        assert_eq!(offset_of!(PictureParameters, loop_filter_info_fields), 686);
        assert_eq!(offset_of!(PictureParameters, ref_deltas), 687);
        assert_eq!(offset_of!(PictureParameters, mode_deltas), 695);
        assert_eq!(offset_of!(PictureParameters, base_qindex), 697);
        assert_eq!(offset_of!(PictureParameters, v_ac_delta_q), 702);
        assert_eq!(offset_of!(PictureParameters, qmatrix_fields), 704);
        assert_eq!(offset_of!(PictureParameters, mode_control_fields), 708);
        assert_eq!(offset_of!(PictureParameters, cdef_damping_minus_3), 712);
        assert_eq!(offset_of!(PictureParameters, cdef_y_strengths), 714);
        assert_eq!(offset_of!(PictureParameters, cdef_uv_strengths), 722);
        assert_eq!(offset_of!(PictureParameters, loop_restoration_fields), 730);
        assert_eq!(offset_of!(PictureParameters, wm), 732);
        assert_eq!(offset_of!(PictureParameters, va_reserved), 1124);
        assert_eq!(size_of::<TileParameters>(), 40);
        assert_eq!(offset_of!(TileParameters, tile_row), 12);
        assert_eq!(offset_of!(TileParameters, tg_start), 16);
        assert_eq!(offset_of!(TileParameters, anchor_frame_idx), 20);
        assert_eq!(offset_of!(TileParameters, tile_idx_in_tile_list), 22);
        assert_eq!(offset_of!(TileParameters, va_reserved), 24);
    }

    /// The bit positions match the C bit fields, as a C program printed them
    /// after setting each field alone.
    #[test]
    fn bit_positions_match_the_c_header() {
        assert_eq!(sequence_bits::ENABLE_ORDER_HINT, 128);
        assert_eq!(sequence_bits::FILM_GRAIN_PARAMS_PRESENT, 32_768);
        assert_eq!(picture_bits::UNIFORM_TILE_SPACING, 16_384);
        assert_eq!(picture_bits::ALLOW_WARPED_MOTION, 32_768);
        assert_eq!(mode_bits::SKIP_MODE_PRESENT, 2048);
        assert_eq!(2 << mode_bits::TX_MODE_SHIFT, 256);
        assert_eq!(15 << quantizer_matrix_bits::QM_V_SHIFT, 7680);
        assert_eq!(restoration_bits::UV_SHIFT, 256);
        assert_eq!(loop_filter_bits::MODE_REF_DELTA_UPDATE, 16);
    }

    /// Returns the parsed frames of a short two-tile stream from the software
    /// encoder, with the sequence header.
    fn encoded_frames() -> (SequenceHeader, Vec<ParsedFrame>) {
        let (width, height) = (1024, 256);
        let mut encoder = Av1Encoder::new(EncoderSettings {
            width,
            height,
            frame_rate: 60,
            speed: FASTEST_SPEED,
            quantizer: 100,
            threads: 1,
        })
        .unwrap();
        let mut parser = FrameParser::new();
        let mut frames = Vec::new();
        for index in 0..3 {
            let frame = Yuv420Frame::from_planes(
                width,
                height,
                (0..width * height)
                    .map(|i| ((i + index * 7) % 220) as u8)
                    .collect(),
                vec![128; width * height / 4],
                vec![128; width * height / 4],
            )
            .unwrap();
            let request = if index == 0 {
                KeyFrameRequest::Force
            } else {
                KeyFrameRequest::Auto
            };
            for unit in encoder.encode(&frame, request).unwrap() {
                frames.extend(parser.parse(&unit.temporal_unit).unwrap());
            }
        }
        (parser.sequence().unwrap().clone(), frames)
    }

    /// A key frame refers to no surface, and an inter frame to the surfaces
    /// of its slots; the sizes, quantizer and tiles come from its header.
    #[test]
    fn fills_parameters_from_the_header() {
        let (sequence, frames) = encoded_frames();
        let slots = [1, 2, 3, 4, 5, 6, 7, 8];
        let key = PictureParameters::new(&sequence, &frames[0], 9, &slots);
        assert_eq!(key.ref_frame_map, [VA_INVALID_ID; NUM_REF_FRAMES]);
        assert_eq!(key.current_frame, 9);
        assert_eq!(key.current_display_picture, 9);
        assert_eq!(key.frame_width_minus1, 1023);
        assert_eq!(key.frame_height_minus1, 255);
        assert_eq!(key.bit_depth_idx, 0);
        assert_eq!(key.pic_info_fields & 0b11, FrameType::Key as u32);
        assert_eq!(key.base_qindex, frames[0].header.quantization.base_q_idx);
        let header = &frames[0].header;
        assert_eq!(usize::from(key.tile_cols), header.tiles.cols());
        assert_eq!(
            u32::from(key.width_in_sbs_minus_1[0]) + 1,
            header.tiles.col_widths[0]
        );
        let inter = PictureParameters::new(&sequence, &frames[1], 10, &slots);
        assert_eq!(inter.ref_frame_map, slots);
        assert_eq!(inter.pic_info_fields & 0b11, FrameType::Inter as u32);
        assert_eq!(inter.ref_frame_idx, frames[1].header.ref_frame_idx);
    }

    /// Each tile group's tiles lie within its data, where the frame's tiles
    /// lie in the temporal unit.
    #[test]
    fn places_tiles_in_their_group() {
        let (_, frames) = encoded_frames();
        for frame in &frames {
            let groups = tile_groups(frame);
            let tiles: Vec<_> = groups.iter().flat_map(|group| &group.tiles).collect();
            assert_eq!(tiles.len(), frame.tiles.len());
            for group in &groups {
                for tile in &group.tiles {
                    let start = group.data.start + tile.slice_data_offset as usize;
                    let end = start + tile.slice_data_size as usize;
                    assert!(end <= group.data.end);
                    assert!(frame.tiles.iter().any(|parsed| parsed.offset == start
                        && parsed.size == tile.slice_data_size as usize));
                }
            }
        }
        assert!(frames[0].tiles.len() > 1, "the stream has several tiles");
    }
}
