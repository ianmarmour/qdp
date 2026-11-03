//! The parts of VA-API (libva 2, API 1.16 or later) the AV1 encoder and
//! decoder use, loaded at run time so that `qdp` neither needs libva to build
//! nor fails to start without it.
//!
//! The structures mirror `va.h`, `va_enc_av1.h`, `va_vpp.h` and
//! `va_drmcommon.h`; the decoder's own are in `av1_parameters`. Each C union
//! of bit fields is a plain integer here, whose bits are set with the shifts
//! below; C compilers on Linux allocate bit fields from the least significant
//! bit. The tests check every size and the offsets relied on against values
//! from the C headers.

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::sync::OnceLock;

/// A VA display.
pub(super) type VaDisplay = *mut c_void;

/// A VA function's result.
pub(super) type VaStatus = c_int;

/// The identifier of a VA object.
pub(super) type VaId = c_uint;

/// `VAMessageCallback`: receives a message libva or its driver logs.
pub(super) type MessageCallback = unsafe extern "C" fn(*mut c_void, *const c_char);

/// The status of a call that succeeded.
pub(super) const VA_STATUS_SUCCESS: VaStatus = 0;

/// An identifier that names nothing.
pub(super) const VA_INVALID_ID: VaId = 0xffff_ffff;

/// The value of a configuration attribute the driver does not support.
pub(super) const VA_ATTRIB_NOT_SUPPORTED: u32 = 0x8000_0000;

/// `VAProfileAV1Profile0`.
pub(super) const PROFILE_AV1_MAIN: c_int = 32;

/// `VAEntrypointVLD`: the decoder that reads each frame's data itself,
/// given its parameters.
pub(super) const ENTRYPOINT_DECODE: c_int = 1;

/// `VAEntrypointEncSlice`: the full-featured encoder.
pub(super) const ENTRYPOINT_ENCODE: c_int = 6;

/// `VAEntrypointEncSliceLP`: the low-power encoder some GPUs offer instead.
pub(super) const ENTRYPOINT_ENCODE_LOW_POWER: c_int = 8;

/// `VAConfigAttribRTFormat`.
pub(super) const ATTRIB_RT_FORMAT: c_int = 0;

/// `VAConfigAttribRateControl`.
pub(super) const ATTRIB_RATE_CONTROL: c_int = 5;

/// `VAConfigAttribEncPackedHeaders`.
pub(super) const ATTRIB_PACKED_HEADERS: c_int = 10;

/// `VAConfigAttribEncAV1Ext2`.
pub(super) const ATTRIB_AV1_EXT2: c_int = 54;

/// `VA_RT_FORMAT_YUV420`: 8-bit 4:2:0 surfaces.
pub(super) const RT_FORMAT_YUV420: u32 = 0x1;

/// `VA_RC_CQP`: a constant quantizer.
pub(super) const RATE_CONTROL_CQP: u32 = 0x10;

/// `VA_ENC_PACKED_HEADER_SEQUENCE`.
pub(super) const PACKED_HEADER_SEQUENCE: u32 = 0x1;

/// `VA_ENC_PACKED_HEADER_PICTURE`.
pub(super) const PACKED_HEADER_PICTURE: u32 = 0x2;

/// `VA_PROGRESSIVE`.
pub(super) const PROGRESSIVE: c_int = 0x1;

/// `VA_FOURCC_NV12`: a luma plane followed by interleaved chroma.
pub(super) const FOURCC_NV12: u32 = u32::from_le_bytes(*b"NV12");

/// `VA_LSB_FIRST`.
pub(super) const LSB_FIRST: u32 = 1;

/// The bits per pixel of NV12.
pub(super) const NV12_BITS_PER_PIXEL: u32 = 12;

/// `VAPictureParameterBufferType`.
pub(super) const BUFFER_DECODE_PICTURE_PARAMETERS: c_int = 0;

/// `VASliceParameterBufferType`, which carries an AV1 tile group's tiles.
pub(super) const BUFFER_DECODE_TILE_PARAMETERS: c_int = 4;

/// `VASliceDataBufferType`, which carries a tile group's data.
pub(super) const BUFFER_DECODE_TILE_DATA: c_int = 5;

/// `VAEncCodedBufferType`.
pub(super) const BUFFER_CODED: c_int = 21;

/// `VAEncSequenceParameterBufferType`.
pub(super) const BUFFER_SEQUENCE_PARAMETERS: c_int = 22;

/// `VAEncPictureParameterBufferType`.
pub(super) const BUFFER_PICTURE_PARAMETERS: c_int = 23;

/// `VAEncSliceParameterBufferType`, which carries AV1 tile groups.
pub(super) const BUFFER_TILE_GROUP_PARAMETERS: c_int = 24;

/// `VAEncPackedHeaderParameterBufferType`.
pub(super) const BUFFER_PACKED_HEADER_PARAMETERS: c_int = 25;

/// `VAEncPackedHeaderDataBufferType`.
pub(super) const BUFFER_PACKED_HEADER_DATA: c_int = 26;

/// `VAEncPackedHeaderSequence`.
pub(super) const PACKED_HEADER_TYPE_SEQUENCE: u32 = 1;

/// `VAEncPackedHeaderPicture`.
pub(super) const PACKED_HEADER_TYPE_PICTURE: u32 = 2;

/// The coded buffer status bit set when a frame did not fit the buffer.
pub(super) const CODED_BUFFER_OVERFLOW: u32 = 0x100;

/// `VAProfileNone`, the profile of video processing.
pub(super) const PROFILE_NONE: c_int = -1;

/// `VAEntrypointVideoProc`: scaling and colour conversion.
pub(super) const ENTRYPOINT_VIDEO_PROC: c_int = 10;

/// `VA_RT_FORMAT_RGB32`: surfaces of 32-bit RGB pixels.
pub(super) const RT_FORMAT_RGB32: u32 = 0x0002_0000;

/// `VA_FOURCC_BGRX`: blue, green, red and an unused byte.
pub(super) const FOURCC_BGRX: u32 = u32::from_le_bytes(*b"BGRX");

/// `VA_FOURCC_RGBX`: red, green, blue and an unused byte.
pub(super) const FOURCC_RGBX: u32 = u32::from_le_bytes(*b"RGBX");

/// `VA_RT_FORMAT_RGB32_10`: surfaces of 32-bit RGB pixels, ten bits a
/// colour.
pub(super) const RT_FORMAT_RGB32_10: u32 = 0x0020_0000;

/// `VA_FOURCC_X2R10G10B10`: blue, green and red in ten bits each from the
/// lowest, then two unused bits.
pub(super) const FOURCC_X2R10G10B10: u32 = u32::from_le_bytes(*b"XR30");

/// `VA_FOURCC_X2B10G10R10`: red, green and blue in ten bits each from the
/// lowest, then two unused bits.
pub(super) const FOURCC_X2B10G10R10: u32 = u32::from_le_bytes(*b"XB30");

/// `VASurfaceAttribPixelFormat`.
pub(super) const SURFACE_ATTRIB_PIXEL_FORMAT: c_int = 1;

/// `VASurfaceAttribMemoryType`.
pub(super) const SURFACE_ATTRIB_MEMORY_TYPE: c_int = 6;

/// `VASurfaceAttribExternalBufferDescriptor`.
pub(super) const SURFACE_ATTRIB_EXTERNAL_BUFFER: c_int = 7;

/// `VA_SURFACE_ATTRIB_SETTABLE`.
pub(super) const SURFACE_ATTRIB_SETTABLE: u32 = 0x2;

/// `VAGenericValueTypeInteger`.
pub(super) const GENERIC_VALUE_INTEGER: c_int = 1;

/// `VAGenericValueTypePointer`.
pub(super) const GENERIC_VALUE_POINTER: c_int = 3;

/// `VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2`: memory a
/// [`DrmPrimeSurfaceDescriptor`] describes.
pub(super) const MEM_TYPE_DRM_PRIME_2: u32 = 0x4000_0000;

/// `VA_EXPORT_SURFACE_READ_ONLY`: the exported buffers are only read.
pub(super) const EXPORT_READ_ONLY: u32 = 0x1;

/// `VA_EXPORT_SURFACE_COMPOSED_LAYERS`: the surface is exported as one
/// layer, its planes together.
pub(super) const EXPORT_COMPOSED_LAYERS: u32 = 0x8;

/// `VAProcPipelineParameterBufferType`.
pub(super) const BUFFER_PROC_PIPELINE_PARAMETERS: c_int = 41;

/// `VAProcColorStandardBT709`.
pub(super) const COLOR_STANDARD_BT709: c_int = 2;

/// `VA_SOURCE_RANGE_REDUCED`: luma from 16 to 235, chroma from 16 to 240.
pub(super) const SOURCE_RANGE_REDUCED: u8 = 1;

/// `VA_SOURCE_RANGE_FULL`: every value from 0 to 255.
pub(super) const SOURCE_RANGE_FULL: u8 = 2;

/// The H.273 code of BT.709, for `colour_primaries`,
/// `transfer_characteristics` and `matrix_coefficients`.
pub(super) const H273_BT709: u8 = 1;

/// Bit positions in `VAEncSequenceParameterBufferAV1::seq_fields`.
pub(super) mod seq_fields {
    /// `enable_order_hint`.
    pub(in super::super) const ENABLE_ORDER_HINT: u32 = 1 << 8;
    /// `enable_cdef`.
    pub(in super::super) const ENABLE_CDEF: u32 = 1 << 12;
    /// `subsampling_x`.
    pub(in super::super) const SUBSAMPLING_X: u32 = 1 << 17;
    /// `subsampling_y`.
    pub(in super::super) const SUBSAMPLING_Y: u32 = 1 << 18;
}

/// Bit positions in `VAEncPictureParameterBufferAV1::picture_flags`.
pub(super) mod picture_flags {
    /// The shift of `frame_type`.
    pub(in super::super) const FRAME_TYPE_SHIFT: u32 = 0;
    /// `error_resilient_mode`.
    pub(in super::super) const ERROR_RESILIENT_MODE: u32 = 1 << 2;
}

/// The shift of `tx_mode` in `VAEncPictureParameterBufferAV1::mode_control_flags`.
pub(super) const TX_MODE_SHIFT: u32 = 7;

/// `VAEncPictureParameterBufferAV1::tile_group_obu_hdr_info` with only
/// `obu_has_size_field` set.
pub(super) const TILE_GROUP_HAS_SIZE_FIELD: u8 = 1 << 1;

/// `VARefFrameCtrlAV1` naming `LAST_FRAME` as the only reference to search.
pub(super) const SEARCH_LAST_FRAME_ONLY: u32 = 1;

/// The fields of `VAConfigAttribValEncAV1Ext2`.
pub(super) mod av1_ext2 {
    /// The mask of `tile_size_bytes_minus1`.
    pub(in super::super) const TILE_SIZE_BYTES_MINUS_1_MASK: u32 = 0x3;
    /// The shift of `tx_mode_support`.
    pub(in super::super) const TX_MODE_SUPPORT_SHIFT: u32 = 4;
    /// The `tx_mode_support` bit for `TX_MODE_SELECT`.
    pub(in super::super) const TX_MODE_SELECT_SUPPORTED: u32 = 0x4;
}

/// `VAConfigAttrib`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ConfigAttrib {
    /// The attribute's type.
    pub(super) attrib_type: c_int,
    /// Its value.
    pub(super) value: u32,
}

/// `VAEncSequenceParameterBufferAV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct SequenceParameters {
    /// `seq_profile`.
    pub(super) seq_profile: u8,
    /// `seq_level_idx`.
    pub(super) seq_level_idx: u8,
    /// `seq_tier`.
    pub(super) seq_tier: u8,
    /// `hierarchical_flag`.
    pub(super) hierarchical_flag: u8,
    /// `intra_period`.
    pub(super) intra_period: u32,
    /// `ip_period`.
    pub(super) ip_period: u32,
    /// `bits_per_second`.
    pub(super) bits_per_second: u32,
    /// `seq_fields`.
    pub(super) seq_fields: u32,
    /// `order_hint_bits_minus_1`.
    pub(super) order_hint_bits_minus_1: u8,
    /// `va_reserved`.
    pub(super) va_reserved: [u32; 16],
}

/// `VAEncSegParamAV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct SegmentParameters {
    /// `seg_flags`.
    pub(super) seg_flags: u8,
    /// `segment_number`.
    pub(super) segment_number: u8,
    /// `feature_data`.
    pub(super) feature_data: [[i16; 8]; 8],
    /// `feature_mask`.
    pub(super) feature_mask: [u8; 8],
    /// `va_reserved`.
    pub(super) va_reserved: [u32; 4],
}

/// `VAEncWarpedMotionParamsAV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct WarpedMotionParameters {
    /// `wmtype`.
    pub(super) wmtype: c_int,
    /// `wmmat`.
    pub(super) wmmat: [i32; 8],
    /// `invalid`.
    pub(super) invalid: u8,
    /// `va_reserved`.
    pub(super) va_reserved: [u32; 4],
}

/// `VAEncPictureParameterBufferAV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct PictureParameters {
    /// `frame_width_minus_1`.
    pub(super) frame_width_minus_1: u16,
    /// `frame_height_minus_1`.
    pub(super) frame_height_minus_1: u16,
    /// `reconstructed_frame`.
    pub(super) reconstructed_frame: VaId,
    /// `coded_buf`.
    pub(super) coded_buf: VaId,
    /// `reference_frames`.
    pub(super) reference_frames: [VaId; 8],
    /// `ref_frame_idx`.
    pub(super) ref_frame_idx: [u8; 7],
    /// `hierarchical_level_plus1`.
    pub(super) hierarchical_level_plus1: u8,
    /// `primary_ref_frame`.
    pub(super) primary_ref_frame: u8,
    /// `order_hint`.
    pub(super) order_hint: u8,
    /// `refresh_frame_flags`.
    pub(super) refresh_frame_flags: u8,
    /// `reserved8bits1`.
    pub(super) reserved8bits1: u8,
    /// `ref_frame_ctrl_l0`.
    pub(super) ref_frame_ctrl_l0: u32,
    /// `ref_frame_ctrl_l1`.
    pub(super) ref_frame_ctrl_l1: u32,
    /// `picture_flags`.
    pub(super) picture_flags: u32,
    /// `seg_id_block_size`.
    pub(super) seg_id_block_size: u8,
    /// `num_tile_groups_minus1`.
    pub(super) num_tile_groups_minus1: u8,
    /// `temporal_id`.
    pub(super) temporal_id: u8,
    /// `filter_level`.
    pub(super) filter_level: [u8; 2],
    /// `filter_level_u`.
    pub(super) filter_level_u: u8,
    /// `filter_level_v`.
    pub(super) filter_level_v: u8,
    /// `loop_filter_flags`.
    pub(super) loop_filter_flags: u8,
    /// `superres_scale_denominator`.
    pub(super) superres_scale_denominator: u8,
    /// `interpolation_filter`.
    pub(super) interpolation_filter: u8,
    /// `ref_deltas`.
    pub(super) ref_deltas: [i8; 8],
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
    /// `min_base_qindex`.
    pub(super) min_base_qindex: u8,
    /// `max_base_qindex`.
    pub(super) max_base_qindex: u8,
    /// `qmatrix_flags`.
    pub(super) qmatrix_flags: u16,
    /// `reserved16bits1`.
    pub(super) reserved16bits1: u16,
    /// `mode_control_flags`.
    pub(super) mode_control_flags: u32,
    /// `segments`.
    pub(super) segments: SegmentParameters,
    /// `tile_cols`.
    pub(super) tile_cols: u8,
    /// `tile_rows`.
    pub(super) tile_rows: u8,
    /// `reserved16bits2`.
    pub(super) reserved16bits2: u16,
    /// `width_in_sbs_minus_1`.
    pub(super) width_in_sbs_minus_1: [u16; 63],
    /// `height_in_sbs_minus_1`.
    pub(super) height_in_sbs_minus_1: [u16; 63],
    /// `context_update_tile_id`.
    pub(super) context_update_tile_id: u16,
    /// `cdef_damping_minus_3`.
    pub(super) cdef_damping_minus_3: u8,
    /// `cdef_bits`.
    pub(super) cdef_bits: u8,
    /// `cdef_y_strengths`.
    pub(super) cdef_y_strengths: [u8; 8],
    /// `cdef_uv_strengths`.
    pub(super) cdef_uv_strengths: [u8; 8],
    /// `loop_restoration_flags`.
    pub(super) loop_restoration_flags: u16,
    /// `wm`.
    pub(super) wm: [WarpedMotionParameters; 7],
    /// `bit_offset_qindex`.
    pub(super) bit_offset_qindex: u32,
    /// `bit_offset_segmentation`.
    pub(super) bit_offset_segmentation: u32,
    /// `bit_offset_loopfilter_params`.
    pub(super) bit_offset_loopfilter_params: u32,
    /// `bit_offset_cdef_params`.
    pub(super) bit_offset_cdef_params: u32,
    /// `size_in_bits_cdef_params`.
    pub(super) size_in_bits_cdef_params: u32,
    /// `byte_offset_frame_hdr_obu_size`.
    pub(super) byte_offset_frame_hdr_obu_size: u32,
    /// `size_in_bits_frame_hdr_obu`.
    pub(super) size_in_bits_frame_hdr_obu: u32,
    /// `tile_group_obu_hdr_info`.
    pub(super) tile_group_obu_hdr_info: u8,
    /// `number_skip_frames`.
    pub(super) number_skip_frames: u8,
    /// `reserved16bits3`.
    pub(super) reserved16bits3: u16,
    /// `skip_frames_reduced_size`.
    pub(super) skip_frames_reduced_size: i32,
    /// `va_reserved`.
    pub(super) va_reserved: [u32; 16],
}

/// `VAEncTileGroupBufferAV1`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct TileGroupParameters {
    /// `tg_start`.
    pub(super) tg_start: u8,
    /// `tg_end`.
    pub(super) tg_end: u8,
    /// `va_reserved`.
    pub(super) va_reserved: [u32; 4],
}

/// `VAEncPackedHeaderParameterBuffer`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct PackedHeaderParameters {
    /// `type`.
    pub(super) header_type: u32,
    /// `bit_length`.
    pub(super) bit_length: u32,
    /// `has_emulation_bytes`.
    pub(super) has_emulation_bytes: u8,
    /// `va_reserved`.
    pub(super) va_reserved: [u32; 4],
}

/// `VACodedBufferSegment`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct CodedBufferSegment {
    /// `size`.
    pub(super) size: u32,
    /// `bit_offset`.
    pub(super) bit_offset: u32,
    /// `status`.
    pub(super) status: u32,
    /// `reserved`.
    pub(super) reserved: u32,
    /// `buf`.
    pub(super) buf: *mut c_void,
    /// `next`.
    pub(super) next: *mut c_void,
    /// `va_reserved`.
    pub(super) va_reserved: [u32; 4],
}

/// `VAImageFormat`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct ImageFormat {
    /// `fourcc`.
    pub(super) fourcc: u32,
    /// `byte_order`.
    pub(super) byte_order: u32,
    /// `bits_per_pixel`.
    pub(super) bits_per_pixel: u32,
    /// `depth`.
    pub(super) depth: u32,
    /// `red_mask`.
    pub(super) red_mask: u32,
    /// `green_mask`.
    pub(super) green_mask: u32,
    /// `blue_mask`.
    pub(super) blue_mask: u32,
    /// `alpha_mask`.
    pub(super) alpha_mask: u32,
    /// `va_reserved`.
    pub(super) va_reserved: [u32; 4],
}

/// `VAImage`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct Image {
    /// `image_id`.
    pub(super) image_id: VaId,
    /// `format`.
    pub(super) format: ImageFormat,
    /// `buf`.
    pub(super) buf: VaId,
    /// `width`.
    pub(super) width: u16,
    /// `height`.
    pub(super) height: u16,
    /// `data_size`.
    pub(super) data_size: u32,
    /// `num_planes`.
    pub(super) num_planes: u32,
    /// `pitches`.
    pub(super) pitches: [u32; 3],
    /// `offsets`.
    pub(super) offsets: [u32; 3],
    /// `num_palette_entries`.
    pub(super) num_palette_entries: i32,
    /// `entry_bytes`.
    pub(super) entry_bytes: i32,
    /// `component_order`.
    pub(super) component_order: [i8; 4],
    /// `va_reserved`.
    pub(super) va_reserved: [u32; 4],
}

/// The value of a `VAGenericValue`: an integer or a pointer, as its type
/// says.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) union GenericValueData {
    /// `i`.
    pub(super) i: i32,
    /// `p`.
    pub(super) p: *mut c_void,
}

/// `VAGenericValue`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct GenericValue {
    /// `type`.
    pub(super) value_type: c_int,
    /// `value`.
    pub(super) value: GenericValueData,
}

/// `VASurfaceAttrib`.
#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct SurfaceAttrib {
    /// `type`.
    pub(super) attrib_type: c_int,
    /// `flags`.
    pub(super) flags: u32,
    /// `value`.
    pub(super) value: GenericValue,
}

impl SurfaceAttrib {
    /// An integer attribute to create surfaces with.
    pub(super) fn integer(attrib_type: c_int, value: i32) -> Self {
        Self {
            attrib_type,
            flags: SURFACE_ATTRIB_SETTABLE,
            value: GenericValue {
                value_type: GENERIC_VALUE_INTEGER,
                value: GenericValueData { i: value },
            },
        }
    }

    /// A pointer attribute to create surfaces with.
    pub(super) fn pointer(attrib_type: c_int, value: *mut c_void) -> Self {
        Self {
            attrib_type,
            flags: SURFACE_ATTRIB_SETTABLE,
            value: GenericValue {
                value_type: GENERIC_VALUE_POINTER,
                value: GenericValueData { p: value },
            },
        }
    }

    /// Returns the integer value, if the attribute holds one.
    pub(super) fn integer_value(&self) -> Option<i32> {
        if self.value.value_type != GENERIC_VALUE_INTEGER {
            return None;
        }
        // SAFETY: an integer attribute's value is its `i` member.
        Some(unsafe { self.value.value.i })
    }
}

/// One DRM object of a [`DrmPrimeSurfaceDescriptor`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct DrmPrimeObject {
    /// `fd`.
    pub(super) fd: c_int,
    /// `size`.
    pub(super) size: u32,
    /// `drm_format_modifier`.
    pub(super) drm_format_modifier: u64,
}

/// One layer of a [`DrmPrimeSurfaceDescriptor`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct DrmPrimeLayer {
    /// `drm_format`.
    pub(super) drm_format: u32,
    /// `num_planes`.
    pub(super) num_planes: u32,
    /// `object_index`.
    pub(super) object_index: [u32; 4],
    /// `offset`.
    pub(super) offset: [u32; 4],
    /// `pitch`.
    pub(super) pitch: [u32; 4],
}

/// `VADRMPRIMESurfaceDescriptor`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct DrmPrimeSurfaceDescriptor {
    /// `fourcc`.
    pub(super) fourcc: u32,
    /// `width`.
    pub(super) width: u32,
    /// `height`.
    pub(super) height: u32,
    /// `num_objects`.
    pub(super) num_objects: u32,
    /// `objects`.
    pub(super) objects: [DrmPrimeObject; 4],
    /// `num_layers`.
    pub(super) num_layers: u32,
    /// `layers`.
    pub(super) layers: [DrmPrimeLayer; 4],
}

/// `VAProcColorProperties`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct ProcColorProperties {
    /// `chroma_sample_location`.
    pub(super) chroma_sample_location: u8,
    /// `color_range`.
    pub(super) color_range: u8,
    /// `colour_primaries`.
    pub(super) colour_primaries: u8,
    /// `transfer_characteristics`.
    pub(super) transfer_characteristics: u8,
    /// `matrix_coefficients`.
    pub(super) matrix_coefficients: u8,
    /// `reserved`.
    pub(super) reserved: [u8; 3],
}

/// `VARectangle`: a part of a surface.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Rectangle {
    /// `x`.
    pub(super) x: i16,
    /// `y`.
    pub(super) y: i16,
    /// `width`.
    pub(super) width: u16,
    /// `height`.
    pub(super) height: u16,
}

/// `VAProcPipelineParameterBuffer`, on 64-bit targets.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(super) struct ProcPipelineParameters {
    /// `surface`.
    pub(super) surface: VaId,
    /// `surface_region`: null for the whole surface.
    pub(super) surface_region: *const Rectangle,
    /// `surface_color_standard`.
    pub(super) surface_color_standard: c_int,
    /// `output_region`: null for the whole output.
    pub(super) output_region: *const Rectangle,
    /// `output_background_color`.
    pub(super) output_background_color: u32,
    /// `output_color_standard`.
    pub(super) output_color_standard: c_int,
    /// `pipeline_flags`.
    pub(super) pipeline_flags: u32,
    /// `filter_flags`.
    pub(super) filter_flags: u32,
    /// `filters`.
    pub(super) filters: *mut VaId,
    /// `num_filters`.
    pub(super) num_filters: u32,
    /// `forward_references`.
    pub(super) forward_references: *mut VaId,
    /// `num_forward_references`.
    pub(super) num_forward_references: u32,
    /// `backward_references`.
    pub(super) backward_references: *mut VaId,
    /// `num_backward_references`.
    pub(super) num_backward_references: u32,
    /// `rotation_state`.
    pub(super) rotation_state: u32,
    /// `blend_state`.
    pub(super) blend_state: *const c_void,
    /// `mirror_state`.
    pub(super) mirror_state: u32,
    /// `additional_outputs`.
    pub(super) additional_outputs: *mut VaId,
    /// `num_additional_outputs`.
    pub(super) num_additional_outputs: u32,
    /// `input_surface_flag`.
    pub(super) input_surface_flag: u32,
    /// `output_surface_flag`.
    pub(super) output_surface_flag: u32,
    /// `input_color_properties`.
    pub(super) input_color_properties: ProcColorProperties,
    /// `output_color_properties`.
    pub(super) output_color_properties: ProcColorProperties,
    /// `processing_mode`.
    pub(super) processing_mode: c_int,
    /// `output_hdr_metadata`.
    pub(super) output_hdr_metadata: *mut c_void,
    /// `va_reserved`: `VA_PADDING_LARGE - 16` words on 64-bit targets.
    pub(super) va_reserved: [u32; 16],
}

/// A structure of plain integers and pointers, for which all zero bits is a
/// valid value: zero, or null.
pub(super) trait Zeroed: Copy {
    /// Returns the value with every field zero.
    fn zeroed() -> Self {
        // SAFETY: implementors are `repr(C)` structures of integers, arrays of
        // integers and raw pointers, all valid when zero.
        unsafe { std::mem::zeroed() }
    }
}

impl Zeroed for SequenceParameters {}
impl Zeroed for PictureParameters {}
impl Zeroed for TileGroupParameters {}
impl Zeroed for PackedHeaderParameters {}
impl Zeroed for ImageFormat {}
impl Zeroed for Image {}
impl Zeroed for DrmPrimeSurfaceDescriptor {}
impl Zeroed for ProcColorProperties {}
impl Zeroed for ProcPipelineParameters {}

/// The libva functions the encoder and decoder call.
#[derive(Debug, Clone, Copy)]
pub(super) struct LibVa {
    /// `vaGetDisplayDRM`, from libva-drm.
    pub(super) get_display_drm: unsafe extern "C" fn(c_int) -> VaDisplay,
    /// `vaInitialize`.
    pub(super) initialize: unsafe extern "C" fn(VaDisplay, *mut c_int, *mut c_int) -> VaStatus,
    /// `vaSetErrorCallback`.
    pub(super) set_error_callback: unsafe extern "C" fn(
        VaDisplay,
        Option<MessageCallback>,
        *mut c_void,
    ) -> Option<MessageCallback>,
    /// `vaSetInfoCallback`.
    pub(super) set_info_callback: unsafe extern "C" fn(
        VaDisplay,
        Option<MessageCallback>,
        *mut c_void,
    ) -> Option<MessageCallback>,
    /// `vaTerminate`.
    pub(super) terminate: unsafe extern "C" fn(VaDisplay) -> VaStatus,
    /// `vaErrorStr`.
    pub(super) error_str: unsafe extern "C" fn(VaStatus) -> *const c_char,
    /// `vaQueryVendorString`.
    pub(super) query_vendor_string: unsafe extern "C" fn(VaDisplay) -> *const c_char,
    /// `vaMaxNumEntrypoints`.
    pub(super) max_num_entrypoints: unsafe extern "C" fn(VaDisplay) -> c_int,
    /// `vaQueryConfigEntrypoints`.
    pub(super) query_config_entrypoints:
        unsafe extern "C" fn(VaDisplay, c_int, *mut c_int, *mut c_int) -> VaStatus,
    /// `vaGetConfigAttributes`.
    pub(super) get_config_attributes:
        unsafe extern "C" fn(VaDisplay, c_int, c_int, *mut ConfigAttrib, c_int) -> VaStatus,
    /// `vaCreateConfig`.
    pub(super) create_config: unsafe extern "C" fn(
        VaDisplay,
        c_int,
        c_int,
        *mut ConfigAttrib,
        c_int,
        *mut VaId,
    ) -> VaStatus,
    /// `vaDestroyConfig`.
    pub(super) destroy_config: unsafe extern "C" fn(VaDisplay, VaId) -> VaStatus,
    /// `vaCreateSurfaces`.
    pub(super) create_surfaces: unsafe extern "C" fn(
        VaDisplay,
        c_uint,
        c_uint,
        c_uint,
        *mut VaId,
        c_uint,
        *mut c_void,
        c_uint,
    ) -> VaStatus,
    /// `vaDestroySurfaces`.
    pub(super) destroy_surfaces: unsafe extern "C" fn(VaDisplay, *mut VaId, c_int) -> VaStatus,
    /// `vaCreateContext`.
    pub(super) create_context: unsafe extern "C" fn(
        VaDisplay,
        VaId,
        c_int,
        c_int,
        c_int,
        *mut VaId,
        c_int,
        *mut VaId,
    ) -> VaStatus,
    /// `vaDestroyContext`.
    pub(super) destroy_context: unsafe extern "C" fn(VaDisplay, VaId) -> VaStatus,
    /// `vaCreateBuffer`.
    pub(super) create_buffer: unsafe extern "C" fn(
        VaDisplay,
        VaId,
        c_int,
        c_uint,
        c_uint,
        *mut c_void,
        *mut VaId,
    ) -> VaStatus,
    /// `vaDestroyBuffer`.
    pub(super) destroy_buffer: unsafe extern "C" fn(VaDisplay, VaId) -> VaStatus,
    /// `vaMapBuffer`.
    pub(super) map_buffer: unsafe extern "C" fn(VaDisplay, VaId, *mut *mut c_void) -> VaStatus,
    /// `vaUnmapBuffer`.
    pub(super) unmap_buffer: unsafe extern "C" fn(VaDisplay, VaId) -> VaStatus,
    /// `vaBeginPicture`.
    pub(super) begin_picture: unsafe extern "C" fn(VaDisplay, VaId, VaId) -> VaStatus,
    /// `vaRenderPicture`.
    pub(super) render_picture: unsafe extern "C" fn(VaDisplay, VaId, *mut VaId, c_int) -> VaStatus,
    /// `vaEndPicture`.
    pub(super) end_picture: unsafe extern "C" fn(VaDisplay, VaId) -> VaStatus,
    /// `vaSyncSurface`.
    pub(super) sync_surface: unsafe extern "C" fn(VaDisplay, VaId) -> VaStatus,
    /// `vaDeriveImage`.
    pub(super) derive_image: unsafe extern "C" fn(VaDisplay, VaId, *mut Image) -> VaStatus,
    /// `vaCreateImage`.
    pub(super) create_image:
        unsafe extern "C" fn(VaDisplay, *mut ImageFormat, c_int, c_int, *mut Image) -> VaStatus,
    /// `vaPutImage`.
    pub(super) put_image: unsafe extern "C" fn(
        VaDisplay,
        VaId,
        VaId,
        c_int,
        c_int,
        c_uint,
        c_uint,
        c_int,
        c_int,
        c_uint,
        c_uint,
    ) -> VaStatus,
    /// `vaGetImage`.
    pub(super) get_image:
        unsafe extern "C" fn(VaDisplay, VaId, c_int, c_int, c_uint, c_uint, VaId) -> VaStatus,
    /// `vaDestroyImage`.
    pub(super) destroy_image: unsafe extern "C" fn(VaDisplay, VaId) -> VaStatus,
    /// `vaQuerySurfaceAttributes`.
    pub(super) query_surface_attributes:
        unsafe extern "C" fn(VaDisplay, VaId, *mut SurfaceAttrib, *mut c_uint) -> VaStatus,
    /// `vaExportSurfaceHandle`.
    pub(super) export_surface_handle:
        unsafe extern "C" fn(VaDisplay, VaId, u32, u32, *mut c_void) -> VaStatus,
}

/// The file name of libva.
const LIBVA_NAME: &CStr = c"libva.so.2";

/// The file name of libva's DRM backend.
const LIBVA_DRM_NAME: &CStr = c"libva-drm.so.2";

/// libva, once loaded, or why it could not be.
static LIBVA: OnceLock<Result<LibVa, String>> = OnceLock::new();

/// Returns libva's functions, loading the library the first time.
pub(super) fn libva() -> Result<&'static LibVa, String> {
    LIBVA.get_or_init(load).as_ref().map_err(Clone::clone)
}

/// Returns the message of the last `dlopen` or `dlsym` failure.
fn dl_error() -> String {
    // SAFETY: `dlerror` returns null or a NUL-terminated string that stays
    // valid until the next `dl*` call on this thread.
    unsafe {
        let message = libc::dlerror();
        if message.is_null() {
            "unknown error".to_owned()
        } else {
            CStr::from_ptr(message).to_string_lossy().into_owned()
        }
    }
}

/// Loads a shared library. It is never unloaded: graphics drivers do not
/// support being unloaded reliably.
fn open_library(name: &CStr) -> Result<*mut c_void, String> {
    // SAFETY: `name` is NUL-terminated; loading libva runs only its own
    // initializers.
    let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        Err(format!(
            "cannot load {}: {}",
            name.to_string_lossy(),
            dl_error()
        ))
    } else {
        Ok(handle)
    }
}

/// Looks up a function in a loaded library.
///
/// # Safety
///
/// `T` must be the function pointer type of the symbol's C declaration.
unsafe fn symbol<T: Copy>(library: *mut c_void, name: &CStr) -> Result<T, String> {
    assert_eq!(size_of::<T>(), size_of::<*mut c_void>());
    // SAFETY: `library` is a handle from `dlopen` and `name` is
    // NUL-terminated.
    let address = unsafe { libc::dlsym(library, name.as_ptr()) };
    if address.is_null() {
        return Err(format!(
            "libva has no {}: {}",
            name.to_string_lossy(),
            dl_error()
        ));
    }
    // SAFETY: the caller guarantees `T` is the symbol's function type, and the
    // sizes match.
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&address) })
}

/// Loads libva and looks up every function the encoder and decoder call.
fn load() -> Result<LibVa, String> {
    let va = open_library(LIBVA_NAME)?;
    let drm = open_library(LIBVA_DRM_NAME)?;
    // SAFETY: each type is the function's declaration in `va.h` or `va_drm.h`.
    unsafe {
        Ok(LibVa {
            get_display_drm: symbol(drm, c"vaGetDisplayDRM")?,
            initialize: symbol(va, c"vaInitialize")?,
            set_error_callback: symbol(va, c"vaSetErrorCallback")?,
            set_info_callback: symbol(va, c"vaSetInfoCallback")?,
            terminate: symbol(va, c"vaTerminate")?,
            error_str: symbol(va, c"vaErrorStr")?,
            query_vendor_string: symbol(va, c"vaQueryVendorString")?,
            max_num_entrypoints: symbol(va, c"vaMaxNumEntrypoints")?,
            query_config_entrypoints: symbol(va, c"vaQueryConfigEntrypoints")?,
            get_config_attributes: symbol(va, c"vaGetConfigAttributes")?,
            create_config: symbol(va, c"vaCreateConfig")?,
            destroy_config: symbol(va, c"vaDestroyConfig")?,
            create_surfaces: symbol(va, c"vaCreateSurfaces")?,
            destroy_surfaces: symbol(va, c"vaDestroySurfaces")?,
            create_context: symbol(va, c"vaCreateContext")?,
            destroy_context: symbol(va, c"vaDestroyContext")?,
            create_buffer: symbol(va, c"vaCreateBuffer")?,
            destroy_buffer: symbol(va, c"vaDestroyBuffer")?,
            map_buffer: symbol(va, c"vaMapBuffer")?,
            unmap_buffer: symbol(va, c"vaUnmapBuffer")?,
            begin_picture: symbol(va, c"vaBeginPicture")?,
            render_picture: symbol(va, c"vaRenderPicture")?,
            end_picture: symbol(va, c"vaEndPicture")?,
            sync_surface: symbol(va, c"vaSyncSurface")?,
            derive_image: symbol(va, c"vaDeriveImage")?,
            create_image: symbol(va, c"vaCreateImage")?,
            put_image: symbol(va, c"vaPutImage")?,
            get_image: symbol(va, c"vaGetImage")?,
            destroy_image: symbol(va, c"vaDestroyImage")?,
            query_surface_attributes: symbol(va, c"vaQuerySurfaceAttributes")?,
            export_surface_handle: symbol(va, c"vaExportSurfaceHandle")?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;

    /// The structures match the C headers' layouts, as `sizeof` and
    /// `offsetof` report them for libva 2.20 on x86-64 Linux.
    #[test]
    fn layouts_match_the_c_headers() {
        assert_eq!(size_of::<ConfigAttrib>(), 8);
        assert_eq!(size_of::<SequenceParameters>(), 88);
        assert_eq!(offset_of!(SequenceParameters, intra_period), 4);
        assert_eq!(offset_of!(SequenceParameters, seq_fields), 16);
        assert_eq!(offset_of!(SequenceParameters, order_hint_bits_minus_1), 20);
        assert_eq!(offset_of!(SequenceParameters, va_reserved), 24);
        assert_eq!(size_of::<SegmentParameters>(), 156);
        assert_eq!(offset_of!(SegmentParameters, feature_mask), 130);
        assert_eq!(size_of::<WarpedMotionParameters>(), 56);
        assert_eq!(size_of::<PictureParameters>(), 1032);
        assert_eq!(offset_of!(PictureParameters, reference_frames), 12);
        assert_eq!(offset_of!(PictureParameters, primary_ref_frame), 52);
        assert_eq!(offset_of!(PictureParameters, ref_frame_ctrl_l0), 56);
        assert_eq!(offset_of!(PictureParameters, picture_flags), 64);
        assert_eq!(offset_of!(PictureParameters, filter_level), 71);
        assert_eq!(offset_of!(PictureParameters, loop_filter_flags), 75);
        assert_eq!(offset_of!(PictureParameters, ref_deltas), 78);
        assert_eq!(offset_of!(PictureParameters, base_qindex), 88);
        assert_eq!(offset_of!(PictureParameters, qmatrix_flags), 96);
        assert_eq!(offset_of!(PictureParameters, mode_control_flags), 100);
        assert_eq!(offset_of!(PictureParameters, segments), 104);
        assert_eq!(offset_of!(PictureParameters, tile_cols), 260);
        assert_eq!(offset_of!(PictureParameters, width_in_sbs_minus_1), 264);
        assert_eq!(offset_of!(PictureParameters, height_in_sbs_minus_1), 390);
        assert_eq!(offset_of!(PictureParameters, context_update_tile_id), 516);
        assert_eq!(offset_of!(PictureParameters, cdef_damping_minus_3), 518);
        assert_eq!(offset_of!(PictureParameters, cdef_y_strengths), 520);
        assert_eq!(offset_of!(PictureParameters, loop_restoration_flags), 536);
        assert_eq!(offset_of!(PictureParameters, wm), 540);
        assert_eq!(offset_of!(PictureParameters, bit_offset_qindex), 932);
        assert_eq!(
            offset_of!(PictureParameters, size_in_bits_frame_hdr_obu),
            956
        );
        assert_eq!(offset_of!(PictureParameters, tile_group_obu_hdr_info), 960);
        assert_eq!(offset_of!(PictureParameters, skip_frames_reduced_size), 964);
        assert_eq!(offset_of!(PictureParameters, va_reserved), 968);
        assert_eq!(size_of::<TileGroupParameters>(), 20);
        assert_eq!(size_of::<PackedHeaderParameters>(), 28);
        assert_eq!(offset_of!(PackedHeaderParameters, has_emulation_bytes), 8);
        assert_eq!(size_of::<CodedBufferSegment>(), 48);
        assert_eq!(offset_of!(CodedBufferSegment, buf), 16);
        assert_eq!(offset_of!(CodedBufferSegment, next), 24);
        assert_eq!(size_of::<ImageFormat>(), 48);
        assert_eq!(size_of::<Image>(), 120);
        assert_eq!(offset_of!(Image, buf), 52);
        assert_eq!(offset_of!(Image, data_size), 60);
        assert_eq!(offset_of!(Image, pitches), 68);
        assert_eq!(offset_of!(Image, offsets), 80);
        assert_eq!(size_of::<GenericValue>(), 16);
        assert_eq!(offset_of!(GenericValue, value), 8);
        assert_eq!(size_of::<SurfaceAttrib>(), 24);
        assert_eq!(offset_of!(SurfaceAttrib, flags), 4);
        assert_eq!(offset_of!(SurfaceAttrib, value), 8);
        assert_eq!(size_of::<DrmPrimeSurfaceDescriptor>(), 312);
        assert_eq!(offset_of!(DrmPrimeSurfaceDescriptor, objects), 16);
        assert_eq!(offset_of!(DrmPrimeObject, drm_format_modifier), 8);
        assert_eq!(size_of::<DrmPrimeObject>(), 16);
        assert_eq!(offset_of!(DrmPrimeSurfaceDescriptor, num_layers), 80);
        assert_eq!(offset_of!(DrmPrimeSurfaceDescriptor, layers), 84);
        assert_eq!(offset_of!(DrmPrimeLayer, offset), 24);
        assert_eq!(offset_of!(DrmPrimeLayer, pitch), 40);
        assert_eq!(size_of::<DrmPrimeLayer>(), 56);
        assert_eq!(size_of::<ProcColorProperties>(), 8);
        assert_eq!(size_of::<Rectangle>(), 8);
        assert_eq!(size_of::<ProcPipelineParameters>(), 224);
        assert_eq!(offset_of!(ProcPipelineParameters, surface_region), 8);
        assert_eq!(
            offset_of!(ProcPipelineParameters, surface_color_standard),
            16
        );
        assert_eq!(offset_of!(ProcPipelineParameters, output_region), 24);
        assert_eq!(
            offset_of!(ProcPipelineParameters, output_background_color),
            32
        );
        assert_eq!(
            offset_of!(ProcPipelineParameters, output_color_standard),
            36
        );
        assert_eq!(offset_of!(ProcPipelineParameters, filter_flags), 44);
        assert_eq!(offset_of!(ProcPipelineParameters, filters), 48);
        assert_eq!(offset_of!(ProcPipelineParameters, num_filters), 56);
        assert_eq!(offset_of!(ProcPipelineParameters, forward_references), 64);
        assert_eq!(offset_of!(ProcPipelineParameters, backward_references), 80);
        assert_eq!(offset_of!(ProcPipelineParameters, rotation_state), 92);
        assert_eq!(offset_of!(ProcPipelineParameters, blend_state), 96);
        assert_eq!(offset_of!(ProcPipelineParameters, mirror_state), 104);
        assert_eq!(offset_of!(ProcPipelineParameters, additional_outputs), 112);
        assert_eq!(
            offset_of!(ProcPipelineParameters, num_additional_outputs),
            120
        );
        assert_eq!(offset_of!(ProcPipelineParameters, input_surface_flag), 124);
        assert_eq!(offset_of!(ProcPipelineParameters, output_surface_flag), 128);
        assert_eq!(
            offset_of!(ProcPipelineParameters, input_color_properties),
            132
        );
        assert_eq!(
            offset_of!(ProcPipelineParameters, output_color_properties),
            140
        );
        assert_eq!(offset_of!(ProcPipelineParameters, processing_mode), 148);
        assert_eq!(offset_of!(ProcPipelineParameters, output_hdr_metadata), 152);
        assert_eq!(offset_of!(ProcPipelineParameters, va_reserved), 160);
    }

    /// The pixel formats match `va.h`.
    #[test]
    fn fourccs_match_the_c_headers() {
        assert_eq!(FOURCC_BGRX, 0x5852_4742);
        assert_eq!(FOURCC_RGBX, 0x5842_4752);
    }

    /// The bit positions match the C bit fields, as a C compiler packs them.
    #[test]
    fn bit_positions_match_the_c_headers() {
        // Values a C program printed after setting each bit field alone.
        assert_eq!(seq_fields::ENABLE_ORDER_HINT, 256);
        assert_eq!(seq_fields::ENABLE_CDEF, 4096);
        assert_eq!(seq_fields::SUBSAMPLING_X, 131_072);
        assert_eq!(seq_fields::SUBSAMPLING_Y, 262_144);
        assert_eq!(2 << TX_MODE_SHIFT, 256);
        assert_eq!(TILE_GROUP_HAS_SIZE_FIELD, 2);
        assert_eq!(1 << av1_ext2::TX_MODE_SUPPORT_SHIFT, 16);
    }
}
