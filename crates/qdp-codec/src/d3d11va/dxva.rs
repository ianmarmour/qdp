//! The structures a DXVA AV1 decoder is given for each frame, as `dxva.h`
//! declares them (`DXVA_PicParams_AV1` and `DXVA_Tile_AV1`), filled from a
//! parsed frame header as Microsoft's "DirectX Video Acceleration
//! Specification for AV1" describes.
//!
//! The C structures' bit fields are kept as the integers that hold them,
//! with the bits set by name below; the layouts are checked against the
//! offsets a C compiler gives `dxva.h`.

use std::mem::{offset_of, size_of};
use std::ops::Range;

use crate::CodecError;
use crate::frame_parser::{
    FrameHeader, NUM_REF_FRAMES, ParsedFrame, REFS_PER_FRAME, RestorationType,
};
use crate::sequence::SequenceHeader;

/// The texture index of a reference slot that holds no frame.
pub(crate) const NO_TEXTURE: u8 = 0xff;

/// The log2 of the restoration unit size given when restoration is not
/// used.
const UNUSED_RESTORATION_UNIT_LOG2: u16 = 8;

/// The quantizer matrix level given when quantizer matrices are not used.
const NO_QUANTIZER_MATRIX: u8 = 0xff;

/// The `matrix_coefficients` of identity, for RGB streams.
const MATRIX_IDENTITY: u8 = 0;

/// The `anchor_frame` of a tile that is not a large-scale tile.
const NO_ANCHOR_FRAME: u8 = 0xff;

/// The alignment the decoder's bitstream buffer is padded to.
pub(crate) const BITSTREAM_ALIGNMENT: usize = 128;

/// `DXVA_PicEntry_AV1`: one of a frame's references.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PicEntry {
    /// The reference's width.
    pub(crate) width: u32,
    /// The reference's height.
    pub(crate) height: u32,
    /// The reference's global motion parameters.
    pub(crate) wmmat: [i32; 6],
    /// `wminvalid` (bit 0) and `wmtype` (bits 1 and 2).
    pub(crate) global_motion_flags: u8,
    /// The reference slot, an index into `RefFrameMapTextureIndex`, or
    /// [`NO_TEXTURE`].
    pub(crate) index: u8,
    /// Reserved.
    reserved: u16,
}

/// The `tiles` member of `DXVA_PicParams_AV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tiles {
    /// The tile columns.
    pub(crate) cols: u8,
    /// The tile rows.
    pub(crate) rows: u8,
    /// `context_update_tile_id`.
    pub(crate) context_update_id: u16,
    /// Each column's width, in superblocks.
    pub(crate) widths: [u16; 64],
    /// Each row's height, in superblocks.
    pub(crate) heights: [u16; 64],
}

/// The `loop_filter` member of `DXVA_PicParams_AV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct LoopFilter {
    /// The luma levels, vertical and horizontal.
    pub(crate) filter_level: [u8; 2],
    /// The U level.
    pub(crate) filter_level_u: u8,
    /// The V level.
    pub(crate) filter_level_v: u8,
    /// `loop_filter_sharpness`.
    pub(crate) sharpness_level: u8,
    /// `mode_ref_delta_enabled` (bit 0), `mode_ref_delta_update` (1),
    /// `delta_lf_multi` (2) and `delta_lf_present` (3).
    pub(crate) control_flags: u8,
    /// `loop_filter_ref_deltas`.
    pub(crate) ref_deltas: [i8; 8],
    /// `loop_filter_mode_deltas`.
    pub(crate) mode_deltas: [i8; 2],
    /// `delta_lf_res`.
    pub(crate) delta_lf_res: u8,
    /// `FrameRestorationType` of each plane.
    pub(crate) frame_restoration_type: [u8; 3],
    /// The log2 of each plane's restoration unit size.
    pub(crate) log2_restoration_unit_size: [u16; 3],
    /// Reserved.
    reserved: u16,
}

/// The `quantization` member of `DXVA_PicParams_AV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Quantization {
    /// `delta_q_present` (bit 0) and `delta_q_res` (bits 1 and 2).
    pub(crate) control_flags: u8,
    /// `base_q_idx`.
    pub(crate) base_qindex: u8,
    /// `DeltaQYDc`.
    pub(crate) y_dc_delta_q: i8,
    /// `DeltaQUDc`.
    pub(crate) u_dc_delta_q: i8,
    /// `DeltaQVDc`.
    pub(crate) v_dc_delta_q: i8,
    /// `DeltaQUAc`.
    pub(crate) u_ac_delta_q: i8,
    /// `DeltaQVAc`.
    pub(crate) v_ac_delta_q: i8,
    /// `qm_y`, or [`NO_QUANTIZER_MATRIX`].
    pub(crate) qm_y: u8,
    /// `qm_u`, or [`NO_QUANTIZER_MATRIX`].
    pub(crate) qm_u: u8,
    /// `qm_v`, or [`NO_QUANTIZER_MATRIX`].
    pub(crate) qm_v: u8,
    /// Reserved.
    reserved: u16,
}

/// The `cdef` member of `DXVA_PicParams_AV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Cdef {
    /// `cdef_damping_minus_3` (bits 0 and 1) and `cdef_bits` (bits 2 and 3).
    pub(crate) control_flags: u8,
    /// Each luma strength: primary (bits 0 to 5) and secondary as coded
    /// (bits 6 and 7).
    pub(crate) y_strengths: [u8; 8],
    /// Each chroma strength, as the luma ones.
    pub(crate) uv_strengths: [u8; 8],
}

/// The `segmentation` member of `DXVA_PicParams_AV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Segmentation {
    /// `enabled` (bit 0), `update_map` (1), `update_data` (2) and
    /// `temporal_update` (3).
    pub(crate) control_flags: u8,
    /// Reserved.
    reserved: [u8; 3],
    /// The features each segment enables, a bit per feature.
    pub(crate) feature_mask: [u8; 8],
    /// Each segment's feature values.
    pub(crate) feature_data: [[i16; 8]; 8],
}

/// The `film_grain` member of `DXVA_PicParams_AV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FilmGrain {
    /// `apply_grain` (bit 0), `scaling_shift_minus8` (1, 2),
    /// `chroma_scaling_from_luma` (3), `ar_coeff_lag` (4, 5),
    /// `ar_coeff_shift_minus6` (6, 7), `grain_scale_shift` (8, 9),
    /// `overlap_flag` (10), `clip_to_restricted_range` (11) and
    /// `matrix_coeff_is_identity` (12).
    pub(crate) control_flags: u16,
    /// `grain_seed`.
    pub(crate) grain_seed: u16,
    /// The luma scaling points: value and scaling.
    pub(crate) scaling_points_y: [[u8; 2]; 14],
    /// The luma scaling points used.
    pub(crate) num_y_points: u8,
    /// The Cb scaling points.
    pub(crate) scaling_points_cb: [[u8; 2]; 10],
    /// The Cb scaling points used.
    pub(crate) num_cb_points: u8,
    /// The Cr scaling points.
    pub(crate) scaling_points_cr: [[u8; 2]; 10],
    /// The Cr scaling points used.
    pub(crate) num_cr_points: u8,
    /// `ar_coeffs_y_plus_128`.
    pub(crate) ar_coeffs_y: [u8; 24],
    /// `ar_coeffs_cb_plus_128`.
    pub(crate) ar_coeffs_cb: [u8; 25],
    /// `ar_coeffs_cr_plus_128`.
    pub(crate) ar_coeffs_cr: [u8; 25],
    /// `cb_mult`.
    pub(crate) cb_mult: u8,
    /// `cb_luma_mult`.
    pub(crate) cb_luma_mult: u8,
    /// `cr_mult`.
    pub(crate) cr_mult: u8,
    /// `cr_luma_mult`.
    pub(crate) cr_luma_mult: u8,
    /// Reserved.
    reserved: u8,
    /// `cb_offset`.
    pub(crate) cb_offset: i16,
    /// `cr_offset`.
    pub(crate) cr_offset: i16,
}

/// `DXVA_PicParams_AV1`: everything about a frame but its tiles' data.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PicParams {
    /// The frame's width after super-resolution.
    pub(crate) width: u32,
    /// The frame's height.
    pub(crate) height: u32,
    /// The stream's widest frame.
    pub(crate) max_width: u32,
    /// The stream's tallest frame.
    pub(crate) max_height: u32,
    /// The texture the frame is decoded into.
    pub(crate) curr_pic_texture_index: u8,
    /// `SuperresDenom`.
    pub(crate) superres_denom: u8,
    /// The bit depth.
    pub(crate) bitdepth: u8,
    /// `seq_profile`.
    pub(crate) seq_profile: u8,
    /// The tiles.
    pub(crate) tiles: Tiles,
    /// The coding tools: see [`coding`] for the bits.
    pub(crate) coding: u32,
    /// `frame_type` (bits 0 and 1), `show_frame` (2), `showable_frame` (3),
    /// `subsampling_x` (4), `subsampling_y` (5) and `mono_chrome` (6).
    pub(crate) format: u8,
    /// `primary_ref_frame`.
    pub(crate) primary_ref_frame: u8,
    /// `order_hint`.
    pub(crate) order_hint: u8,
    /// `OrderHintBits`.
    pub(crate) order_hint_bits: u8,
    /// The references, `LAST_FRAME` … `ALTREF_FRAME`.
    pub(crate) frame_refs: [PicEntry; REFS_PER_FRAME],
    /// The texture of the frame each reference slot holds, or
    /// [`NO_TEXTURE`].
    pub(crate) ref_frame_map_texture_index: [u8; NUM_REF_FRAMES],
    /// The loop filter and loop restoration.
    pub(crate) loop_filter: LoopFilter,
    /// The quantizers.
    pub(crate) quantization: Quantization,
    /// CDEF.
    pub(crate) cdef: Cdef,
    /// `interpolation_filter`.
    pub(crate) interp_filter: u8,
    /// The segmentation.
    pub(crate) segmentation: Segmentation,
    /// The film grain.
    pub(crate) film_grain: FilmGrain,
    /// Reserved.
    reserved: u32,
    /// The status report number, which is not used: some drivers decode
    /// wrongly when it is set.
    status_report_feedback_number: u32,
}

/// `DXVA_Tile_AV1`: where a tile's data lies in the bitstream buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tile {
    /// The offset of the tile's data in the bitstream buffer.
    pub(crate) data_offset: u32,
    /// The bytes of the tile's data.
    pub(crate) data_size: u32,
    /// The tile's row.
    pub(crate) row: u16,
    /// The tile's column.
    pub(crate) column: u16,
    /// Reserved.
    reserved16: u16,
    /// [`NO_ANCHOR_FRAME`]: tiles are not large-scale tiles.
    pub(crate) anchor_frame: u8,
    /// Reserved.
    reserved8: u8,
}

// The layouts dxva.h gives these structures, as a C compiler lays them out.
const _: () = {
    assert!(size_of::<PicEntry>() == 36);
    assert!(offset_of!(PicEntry, global_motion_flags) == 32);
    assert!(size_of::<Tile>() == 16);
    assert!(offset_of!(Tile, anchor_frame) == 14);
    assert!(size_of::<PicParams>() == 912);
    assert!(offset_of!(PicParams, tiles) == 20);
    assert!(offset_of!(PicParams, coding) == 280);
    assert!(offset_of!(PicParams, format) == 284);
    assert!(offset_of!(PicParams, frame_refs) == 288);
    assert!(offset_of!(PicParams, ref_frame_map_texture_index) == 540);
    assert!(offset_of!(PicParams, loop_filter) == 548);
    assert!(offset_of!(LoopFilter, log2_restoration_unit_size) == 20);
    assert!(offset_of!(PicParams, quantization) == 576);
    assert!(offset_of!(PicParams, cdef) == 588);
    assert!(offset_of!(PicParams, interp_filter) == 605);
    assert!(offset_of!(PicParams, segmentation) == 606);
    assert!(offset_of!(Segmentation, feature_data) == 12);
    assert!(offset_of!(PicParams, film_grain) == 746);
    assert!(offset_of!(FilmGrain, cb_offset) == 154);
    assert!(offset_of!(PicParams, status_report_feedback_number) == 908);
};

/// The bits of `DXVA_PicParams_AV1`'s `coding` member.
pub(crate) mod coding {
    /// `use_128x128_superblock`.
    pub(crate) const USE_128X128_SUPERBLOCK: u32 = 1 << 0;
    /// `intra_edge_filter`.
    pub(crate) const INTRA_EDGE_FILTER: u32 = 1 << 1;
    /// `interintra_compound`.
    pub(crate) const INTERINTRA_COMPOUND: u32 = 1 << 2;
    /// `masked_compound`.
    pub(crate) const MASKED_COMPOUND: u32 = 1 << 3;
    /// `warped_motion`.
    pub(crate) const WARPED_MOTION: u32 = 1 << 4;
    /// `dual_filter`.
    pub(crate) const DUAL_FILTER: u32 = 1 << 5;
    /// `jnt_comp`.
    pub(crate) const JNT_COMP: u32 = 1 << 6;
    /// `screen_content_tools`.
    pub(crate) const SCREEN_CONTENT_TOOLS: u32 = 1 << 7;
    /// `integer_mv`.
    pub(crate) const INTEGER_MV: u32 = 1 << 8;
    /// `cdef`.
    pub(crate) const CDEF: u32 = 1 << 9;
    /// `restoration`.
    pub(crate) const RESTORATION: u32 = 1 << 10;
    /// `film_grain`.
    pub(crate) const FILM_GRAIN: u32 = 1 << 11;
    /// `intrabc`.
    pub(crate) const INTRABC: u32 = 1 << 12;
    /// `high_precision_mv`.
    pub(crate) const HIGH_PRECISION_MV: u32 = 1 << 13;
    /// `switchable_motion_mode`.
    pub(crate) const SWITCHABLE_MOTION_MODE: u32 = 1 << 14;
    /// `filter_intra`.
    pub(crate) const FILTER_INTRA: u32 = 1 << 15;
    /// `disable_frame_end_update_cdf`.
    pub(crate) const DISABLE_FRAME_END_UPDATE_CDF: u32 = 1 << 16;
    /// `disable_cdf_update`.
    pub(crate) const DISABLE_CDF_UPDATE: u32 = 1 << 17;
    /// `reference_mode`.
    pub(crate) const REFERENCE_MODE: u32 = 1 << 18;
    /// `skip_mode`.
    pub(crate) const SKIP_MODE: u32 = 1 << 19;
    /// `reduced_tx_set`.
    pub(crate) const REDUCED_TX_SET: u32 = 1 << 20;
    /// `superres`.
    pub(crate) const SUPERRES: u32 = 1 << 21;
    /// The shift of `tx_mode`, two bits.
    pub(crate) const TX_MODE_SHIFT: u32 = 22;
    /// `use_ref_frame_mvs`.
    pub(crate) const USE_REF_FRAME_MVS: u32 = 1 << 24;
    /// `enable_ref_frame_mvs`.
    pub(crate) const ENABLE_REF_FRAME_MVS: u32 = 1 << 25;
    /// `reference_frame_update`.
    pub(crate) const REFERENCE_FRAME_UPDATE: u32 = 1 << 26;
}

/// Returns `bit` if `set`, or 0.
fn flag<T: Default>(set: bool, bit: T) -> T {
    if set { bit } else { T::default() }
}

/// Returns the coding tool bits of a frame of `sequence`.
fn coding_flags(sequence: &SequenceHeader, header: &FrameHeader) -> u32 {
    let tools = &sequence.coding;
    flag(tools.use_128x128_superblock, coding::USE_128X128_SUPERBLOCK)
        | flag(tools.enable_intra_edge_filter, coding::INTRA_EDGE_FILTER)
        | flag(tools.enable_interintra_compound, coding::INTERINTRA_COMPOUND)
        | flag(tools.enable_masked_compound, coding::MASKED_COMPOUND)
        | flag(header.allow_warped_motion, coding::WARPED_MOTION)
        | flag(tools.enable_dual_filter, coding::DUAL_FILTER)
        | flag(tools.enable_jnt_comp, coding::JNT_COMP)
        | flag(header.allow_screen_content_tools, coding::SCREEN_CONTENT_TOOLS)
        | flag(header.force_integer_mv, coding::INTEGER_MV)
        | flag(tools.enable_cdef, coding::CDEF)
        | flag(tools.enable_restoration, coding::RESTORATION)
        | flag(sequence.film_grain, coding::FILM_GRAIN)
        | flag(header.allow_intrabc, coding::INTRABC)
        | flag(header.allow_high_precision_mv, coding::HIGH_PRECISION_MV)
        | flag(header.is_motion_mode_switchable, coding::SWITCHABLE_MOTION_MODE)
        | flag(tools.enable_filter_intra, coding::FILTER_INTRA)
        | flag(
            header.disable_frame_end_update_cdf,
            coding::DISABLE_FRAME_END_UPDATE_CDF,
        )
        | flag(header.disable_cdf_update, coding::DISABLE_CDF_UPDATE)
        | flag(header.reference_select, coding::REFERENCE_MODE)
        | flag(header.skip_mode_present, coding::SKIP_MODE)
        | flag(header.reduced_tx_set, coding::REDUCED_TX_SET)
        | flag(header.size.uses_superres(), coding::SUPERRES)
        | (header.tx_mode as u32) << coding::TX_MODE_SHIFT
        | flag(header.use_ref_frame_mvs, coding::USE_REF_FRAME_MVS)
        | flag(tools.enable_ref_frame_mvs, coding::ENABLE_REF_FRAME_MVS)
        // Frames shown again are not decoded, so every frame decoded
        // updates the references as its refresh_frame_flags say.
        | coding::REFERENCE_FRAME_UPDATE
}

impl PicParams {
    /// Returns the parameters of `frame`, a frame of `sequence` that is not
    /// an existing frame shown again, decoded into texture `current`, when
    /// each reference slot holds the frame of the texture
    /// `slot_textures` names.
    pub(crate) fn new(
        sequence: &SequenceHeader,
        frame: &ParsedFrame,
        slot_textures: &[u8; NUM_REF_FRAMES],
        current: u8,
    ) -> Self {
        let header = &frame.header;
        let intra = header.frame_type.is_intra();
        let mut tiles = Tiles {
            cols: header.tiles.cols() as u8,
            rows: header.tiles.rows() as u8,
            context_update_id: header.tiles.context_update_tile_id as u16,
            widths: [0; 64],
            heights: [0; 64],
        };
        for (target, &width) in tiles.widths.iter_mut().zip(&header.tiles.col_widths) {
            *target = width as u16;
        }
        for (target, &height) in tiles.heights.iter_mut().zip(&header.tiles.row_heights) {
            *target = height as u16;
        }
        let frame_refs = std::array::from_fn(|index| {
            let motion = &header.global_motion[index + 1];
            let slot = header.ref_frame_idx[index];
            let reference = frame.references[index];
            let held = !intra && slot_textures[usize::from(slot)] != NO_TEXTURE;
            PicEntry {
                width: if held { reference.upscaled_width } else { 0 },
                height: if held { reference.height } else { 0 },
                wmmat: motion.params,
                global_motion_flags: u8::from(motion.invalid) | (motion.model as u8) << 1,
                index: if held { slot } else { NO_TEXTURE },
                reserved: 0,
            }
        });
        Self {
            width: header.size.upscaled_width,
            height: header.size.height,
            max_width: sequence.max_width,
            max_height: sequence.max_height,
            curr_pic_texture_index: current,
            superres_denom: header.size.superres_denom,
            bitdepth: sequence.bit_depth,
            seq_profile: sequence.profile,
            tiles,
            coding: coding_flags(sequence, header),
            format: header.frame_type as u8
                | flag(header.show_frame, 1 << 2)
                | flag(header.showable_frame, 1 << 3)
                | flag(sequence.subsampling_x, 1 << 4)
                | flag(sequence.subsampling_y, 1 << 5)
                | flag(sequence.monochrome, 1 << 6),
            primary_ref_frame: header.primary_ref_frame,
            order_hint: header.order_hint as u8,
            order_hint_bits: sequence.coding.order_hint_bits as u8,
            frame_refs,
            ref_frame_map_texture_index: *slot_textures,
            loop_filter: loop_filter(header),
            quantization: quantization(header),
            cdef: cdef(header),
            interp_filter: header.interpolation_filter,
            segmentation: segmentation(header),
            film_grain: film_grain(sequence, header),
            reserved: 0,
            status_report_feedback_number: 0,
        }
    }

    /// Returns the parameters as the bytes the decoder's buffer takes.
    pub(crate) fn as_bytes(&self) -> &[u8] {
        bytes_of(self)
    }
}

/// Returns the bytes of one of this module's structures.
#[allow(unsafe_code)]
fn bytes_of<T: DxvaStructure>(value: &T) -> &[u8] {
    // SAFETY: the structures are `repr(C)` plain data whose fields leave no
    // padding, as the layout checks above show, so every byte is
    // initialized; the slice borrows `value` for its lifetime.
    unsafe { std::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

/// The structures of this module, which are plain data without padding.
trait DxvaStructure: Copy {}

impl DxvaStructure for PicParams {}

impl DxvaStructure for Tile {}

/// Returns the `loop_filter` member for `header`.
fn loop_filter(header: &FrameHeader) -> LoopFilter {
    let filter = &header.loop_filter;
    let delta = &header.delta;
    let restoration = &header.restoration;
    let used = restoration.is_used();
    LoopFilter {
        filter_level: [filter.levels[0], filter.levels[1]],
        filter_level_u: filter.levels[2],
        filter_level_v: filter.levels[3],
        sharpness_level: filter.sharpness,
        control_flags: flag(filter.delta_enabled, 1)
            | flag(filter.delta_update, 1 << 1)
            | flag(delta.lf_multi, 1 << 2)
            | flag(delta.lf_present, 1 << 3),
        ref_deltas: filter.ref_deltas,
        mode_deltas: filter.mode_deltas,
        delta_lf_res: delta.lf_res,
        frame_restoration_type: restoration.types.map(|kind: RestorationType| kind as u8),
        log2_restoration_unit_size: restoration.unit_size_log2.map(|size| {
            if used {
                u16::from(size)
            } else {
                UNUSED_RESTORATION_UNIT_LOG2
            }
        }),
        reserved: 0,
    }
}

/// Returns the `quantization` member for `header`.
fn quantization(header: &FrameHeader) -> Quantization {
    let quantization = &header.quantization;
    let delta = &header.delta;
    let matrices = quantization.matrices;
    Quantization {
        control_flags: flag(delta.q_present, 1) | delta.q_res << 1,
        base_qindex: quantization.base_q_idx,
        y_dc_delta_q: quantization.delta_q_y_dc,
        u_dc_delta_q: quantization.delta_q_u_dc,
        v_dc_delta_q: quantization.delta_q_v_dc,
        u_ac_delta_q: quantization.delta_q_u_ac,
        v_ac_delta_q: quantization.delta_q_v_ac,
        qm_y: matrices.map_or(NO_QUANTIZER_MATRIX, |matrices| matrices.y),
        qm_u: matrices.map_or(NO_QUANTIZER_MATRIX, |matrices| matrices.u),
        qm_v: matrices.map_or(NO_QUANTIZER_MATRIX, |matrices| matrices.v),
        reserved: 0,
    }
}

/// Returns the `cdef` member for `header`.
fn cdef(header: &FrameHeader) -> Cdef {
    let cdef = &header.cdef;
    let strength = |primary: u8, secondary: u8| primary | secondary << 6;
    Cdef {
        control_flags: cdef.damping_minus_3 | cdef.bits << 2,
        y_strengths: std::array::from_fn(|index| {
            strength(cdef.y_primary[index], cdef.y_secondary[index])
        }),
        uv_strengths: std::array::from_fn(|index| {
            strength(cdef.uv_primary[index], cdef.uv_secondary[index])
        }),
    }
}

/// Returns the `segmentation` member for `header`.
fn segmentation(header: &FrameHeader) -> Segmentation {
    let segmentation = &header.segmentation;
    Segmentation {
        control_flags: flag(segmentation.enabled, 1)
            | flag(segmentation.update_map, 1 << 1)
            | flag(segmentation.update_data, 1 << 2)
            | flag(segmentation.temporal_update, 1 << 3),
        reserved: [0; 3],
        feature_mask: segmentation.features.enabled,
        feature_data: segmentation.features.data,
    }
}

/// Returns the `film_grain` member for `header`, all zero unless the frame
/// applies grain.
fn film_grain(sequence: &SequenceHeader, header: &FrameHeader) -> FilmGrain {
    let grain = &header.film_grain;
    if !grain.apply_grain {
        return FilmGrain::default();
    }
    let points = |source: &[[u8; 2]], target: &mut [[u8; 2]]| {
        target[..source.len()].copy_from_slice(source);
        source.len() as u8
    };
    let mut result = FilmGrain {
        control_flags: 1
            | u16::from(grain.scaling_shift_minus_8) << 1
            | flag(grain.chroma_scaling_from_luma, 1 << 3)
            | u16::from(grain.ar_coeff_lag) << 4
            | u16::from(grain.ar_coeff_shift_minus_6) << 6
            | u16::from(grain.grain_scale_shift) << 8
            | flag(grain.overlap, 1 << 10)
            | flag(grain.clip_to_restricted_range, 1 << 11)
            | flag(
                sequence
                    .color_description
                    .is_some_and(|color| color.matrix == MATRIX_IDENTITY),
                1 << 12,
            ),
        grain_seed: grain.grain_seed,
        ar_coeffs_y: grain.ar_coeffs_y,
        ar_coeffs_cb: grain.ar_coeffs_cb,
        ar_coeffs_cr: grain.ar_coeffs_cr,
        cb_mult: grain.cb_mult,
        cb_luma_mult: grain.cb_luma_mult,
        cr_mult: grain.cr_mult,
        cr_luma_mult: grain.cr_luma_mult,
        cb_offset: grain.cb_offset as i16,
        cr_offset: grain.cr_offset as i16,
        ..FilmGrain::default()
    };
    result.num_y_points = points(&grain.y_points, &mut result.scaling_points_y);
    result.num_cb_points = points(&grain.cb_points, &mut result.scaling_points_cb);
    result.num_cr_points = points(&grain.cr_points, &mut result.scaling_points_cr);
    result
}

/// Returns the bytes of a frame's tile data in the bitstream buffer: its
/// tile groups' data, one after another, padded to
/// [`BITSTREAM_ALIGNMENT`].
pub(crate) fn bitstream_len(frame: &ParsedFrame) -> usize {
    frame
        .tile_data
        .iter()
        .map(Range::len)
        .sum::<usize>()
        .next_multiple_of(BITSTREAM_ALIGNMENT)
}

/// Copies a frame's tile groups' data from `temporal_unit` into
/// `destination`, zeroing the padding after it, and returns where each tile
/// lies in it.
pub(crate) fn copy_tiles(
    temporal_unit: &[u8],
    frame: &ParsedFrame,
    destination: &mut [u8],
) -> Result<Vec<Tile>, CodecError> {
    let length = bitstream_len(frame);
    if destination.len() < length {
        return Err(CodecError::HardwareUnavailable(format!(
            "the decoder's bitstream buffer holds {} bytes, and a frame needs {length}",
            destination.len()
        )));
    }
    let mut written = 0;
    let mut group_starts = Vec::with_capacity(frame.tile_data.len());
    for range in &frame.tile_data {
        let data = temporal_unit
            .get(range.clone())
            .ok_or(CodecError::MalformedBitstream(
                "tile data outside the temporal unit",
            ))?;
        destination[written..written + data.len()].copy_from_slice(data);
        group_starts.push((range.start, written));
        written += data.len();
    }
    destination[written..length].fill(0);
    frame
        .tiles
        .iter()
        .map(|tile| {
            let &(group_start, buffer_start) = group_starts
                .iter()
                .rev()
                .find(|(start, _)| *start <= tile.offset)
                .ok_or(CodecError::MalformedBitstream("a tile outside its group"))?;
            Ok(Tile {
                data_offset: (buffer_start + tile.offset - group_start) as u32,
                data_size: tile.size as u32,
                row: tile.row,
                column: tile.column,
                reserved16: 0,
                anchor_frame: NO_ANCHOR_FRAME,
                reserved8: 0,
            })
        })
        .collect()
}

/// Returns the tiles' descriptions as the bytes the decoder's buffer takes.
pub(crate) fn tile_bytes(tiles: &[Tile]) -> Vec<u8> {
    tiles
        .iter()
        .flat_map(|tile| bytes_of(tile).to_vec())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_parser::FrameParser;
    use crate::{Av1Encoder, EncoderSettings, FASTEST_SPEED, KeyFrameRequest, Yuv420Frame};

    /// Encodes two frames of a moving gradient with QDP's encoder, in two
    /// tiles.
    fn stream() -> Vec<Vec<u8>> {
        let (width, height) = (512, 256);
        let mut encoder = Av1Encoder::new(EncoderSettings {
            width,
            height,
            frame_rate: 60,
            speed: FASTEST_SPEED,
            quantizer: 80,
            threads: 1,
        })
        .unwrap();
        (0..2)
            .flat_map(|index| {
                let y = (0..width * height)
                    .map(|i| ((i % width + 4 * index) % 256) as u8)
                    .collect();
                let chroma = vec![128; width * height / 4];
                let frame =
                    Yuv420Frame::from_planes(width, height, y, chroma.clone(), chroma).unwrap();
                let request = if index == 0 {
                    KeyFrameRequest::Force
                } else {
                    KeyFrameRequest::Auto
                };
                encoder
                    .encode(&frame, request)
                    .unwrap()
                    .into_iter()
                    .map(|encoded| encoded.temporal_unit)
            })
            .collect()
    }

    /// A key frame's parameters describe it: its size, its tiles, the
    /// texture it goes to, and no references.
    #[test]
    fn describes_a_key_frame() {
        let units = stream();
        let mut parser = FrameParser::new();
        let [key] = parser.parse(&units[0]).unwrap().try_into().unwrap();
        let sequence = parser.sequence().unwrap();
        let params = PicParams::new(sequence, &key, &[NO_TEXTURE; NUM_REF_FRAMES], 3);
        assert_eq!((params.width, params.height), (512, 256));
        assert_eq!((params.max_width, params.max_height), (512, 256));
        assert_eq!(params.curr_pic_texture_index, 3);
        assert_eq!((params.bitdepth, params.seq_profile), (8, 0));
        assert_eq!(params.superres_denom, 8);
        // A shown key frame (type 0) with 4:2:0 subsampling.
        assert_eq!(params.format, 1 << 2 | 1 << 4 | 1 << 5);
        assert_eq!(params.primary_ref_frame, 7);
        assert_eq!(
            params.order_hint_bits,
            sequence.coding.order_hint_bits as u8
        );
        assert!(
            params
                .frame_refs
                .iter()
                .all(|entry| entry.index == NO_TEXTURE)
        );
        assert_eq!(
            usize::from(params.tiles.cols) * usize::from(params.tiles.rows),
            key.header.tiles.count()
        );
        let widths: Vec<u16> = key
            .header
            .tiles
            .col_widths
            .iter()
            .map(|&w| w as u16)
            .collect();
        assert_eq!(params.tiles.widths[..widths.len()], widths[..]);
        assert_eq!(
            params.quantization.base_qindex,
            key.header.quantization.base_q_idx
        );
        assert_eq!(
            params.coding & coding::REFERENCE_FRAME_UPDATE,
            coding::REFERENCE_FRAME_UPDATE
        );
        assert_eq!(params.as_bytes().len(), 912);
    }

    /// An inter frame names the slots it refers to and their textures.
    #[test]
    fn describes_an_inter_frame() {
        let units = stream();
        let mut parser = FrameParser::new();
        parser.parse(&units[0]).unwrap();
        let [inter] = parser.parse(&units[1]).unwrap().try_into().unwrap();
        let sequence = parser.sequence().unwrap();
        let mut textures = [NO_TEXTURE; NUM_REF_FRAMES];
        textures
            .iter_mut()
            .enumerate()
            .for_each(|(slot, texture)| *texture = slot as u8 + 8);
        let params = PicParams::new(sequence, &inter, &textures, 1);
        assert_eq!(params.format & 3, 1, "an inter frame");
        assert_eq!(params.ref_frame_map_texture_index, textures);
        for (index, entry) in params.frame_refs.iter().enumerate() {
            assert_eq!(entry.index, inter.header.ref_frame_idx[index]);
            assert_eq!((entry.width, entry.height), (512, 256));
            // Identity motion, valid.
            assert_eq!(entry.global_motion_flags, 0);
            assert_eq!(entry.wmmat, [0, 0, 1 << 16, 0, 0, 1 << 16]);
        }
    }

    /// The bitstream buffer gets each tile group's data, padded, and each
    /// tile's offset in it points at the tile's own bytes.
    #[test]
    fn copies_tiles() {
        let units = stream();
        let mut parser = FrameParser::new();
        let [key] = parser.parse(&units[0]).unwrap().try_into().unwrap();
        let length = bitstream_len(&key);
        assert_eq!(length % BITSTREAM_ALIGNMENT, 0);
        let mut buffer = vec![0xaa; length + 7];
        let tiles = copy_tiles(&units[0], &key, &mut buffer).unwrap();
        assert_eq!(tiles.len(), key.tiles.len());
        for (described, parsed) in tiles.iter().zip(&key.tiles) {
            let start = described.data_offset as usize;
            assert_eq!(
                buffer[start..start + described.data_size as usize],
                units[0][parsed.offset..parsed.offset + parsed.size]
            );
            assert_eq!(described.anchor_frame, NO_ANCHOR_FRAME);
        }
        let data: usize = key.tile_data.iter().map(Range::len).sum();
        assert!(buffer[data..length].iter().all(|&byte| byte == 0));
        assert!(copy_tiles(&units[0], &key, &mut buffer[..length - 1]).is_err());
        assert_eq!(tile_bytes(&tiles).len(), 16 * tiles.len());
    }
}
