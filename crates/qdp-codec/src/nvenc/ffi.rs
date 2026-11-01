#![cfg_attr(not(windows), allow(dead_code))]

//! The parts of NVENC's interface, NVIDIA's `nvEncodeAPI.h` of the Video
//! Codec SDK 12.0, that the encoder uses, as the driver's
//! `nvEncodeAPI64.dll` exports them. API 12.0, the first with AV1, needs
//! driver 522 or later.
//!
//! Each structure has the header's layout, which the tests check field by
//! field against the offsets clang gives the header on x86-64, where
//! Windows and Linux lay these structures out alike; fields the encoder
//! does not use are kept as reserved space, and runs of bit fields as one
//! word with constants for their bits. The header is NVIDIA's, under the
//! MIT license.

use std::ffi::{c_char, c_void};

/// NVENC's status codes, `NVENCSTATUS`.
pub(crate) type Status = u32;

/// `NV_ENC_SUCCESS`.
pub(crate) const SUCCESS: Status = 0;

/// `NV_ENC_ERR_NEED_MORE_INPUT`: the encoder holds the picture for later.
pub(crate) const ERR_NEED_MORE_INPUT: Status = 17;

/// The interface version: `NVENCAPI_MAJOR_VERSION` 12 in the low byte and
/// `NVENCAPI_MINOR_VERSION` 0 in the high one.
pub(crate) const API_VERSION: u32 = 12;

/// The interface version as `NvEncodeAPIGetMaxSupportedVersion` reports
/// versions: the major version shifted left by 4, plus the minor.
pub(crate) const API_VERSION_REPORTED: u32 = 12 << 4;

/// Returns the version a structure of revision `revision` carries,
/// `NVENCAPI_STRUCT_VERSION`.
const fn struct_version(revision: u32) -> u32 {
    API_VERSION | (revision << 16) | (0x7 << 28)
}

/// The bit some structures' versions add, for their larger layouts.
const LARGE_STRUCT: u32 = 1 << 31;

/// `NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS_VER`.
pub(crate) const OPEN_ENCODE_SESSION_EX_PARAMS_VER: u32 = struct_version(1);
/// `NV_ENC_CONFIG_VER`.
pub(crate) const CONFIG_VER: u32 = struct_version(8) | LARGE_STRUCT;
/// `NV_ENC_INITIALIZE_PARAMS_VER`.
pub(crate) const INITIALIZE_PARAMS_VER: u32 = struct_version(5) | LARGE_STRUCT;
/// `NV_ENC_RECONFIGURE_PARAMS_VER`.
pub(crate) const RECONFIGURE_PARAMS_VER: u32 = struct_version(1) | LARGE_STRUCT;
/// `NV_ENC_PRESET_CONFIG_VER`.
pub(crate) const PRESET_CONFIG_VER: u32 = struct_version(4) | LARGE_STRUCT;
/// `NV_ENC_CREATE_INPUT_BUFFER_VER`.
pub(crate) const CREATE_INPUT_BUFFER_VER: u32 = struct_version(1);
/// `NV_ENC_LOCK_INPUT_BUFFER_VER`.
pub(crate) const LOCK_INPUT_BUFFER_VER: u32 = struct_version(1);
/// `NV_ENC_CREATE_BITSTREAM_BUFFER_VER`.
pub(crate) const CREATE_BITSTREAM_BUFFER_VER: u32 = struct_version(1);
/// `NV_ENC_PIC_PARAMS_VER`.
pub(crate) const PIC_PARAMS_VER: u32 = struct_version(6) | LARGE_STRUCT;
/// `NV_ENC_LOCK_BITSTREAM_VER`.
pub(crate) const LOCK_BITSTREAM_VER: u32 = struct_version(2);
/// `NV_ENC_RC_PARAMS_VER`.
pub(crate) const RC_PARAMS_VER: u32 = struct_version(1);
/// `NV_ENCODE_API_FUNCTION_LIST_VER`.
pub(crate) const FUNCTION_LIST_VER: u32 = struct_version(2);

/// `NVENC_INFINITE_GOPLENGTH`: no key frame unless one is asked for.
pub(crate) const INFINITE_GOP_LENGTH: u32 = u32::MAX;

/// `NV_ENC_DEVICE_TYPE_DIRECTX`: the device is a Direct3D one.
pub(crate) const DEVICE_TYPE_DIRECTX: u32 = 0;

/// `NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY`.
pub(crate) const TUNING_ULTRA_LOW_LATENCY: u32 = 3;

/// `NV_ENC_PARAMS_RC_CONSTQP`: every frame at a quantizer of the caller's.
pub(crate) const RC_CONST_QP: u32 = 0;

/// `NV_ENC_BUFFER_FORMAT_NV12`.
pub(crate) const BUFFER_FORMAT_NV12: u32 = 0x1;

/// `NV_ENC_MEMORY_HEAP_AUTOSELECT`.
pub(crate) const MEMORY_HEAP_AUTOSELECT: u32 = 0;

/// `NV_ENC_PIC_STRUCT_FRAME`: progressive frames.
pub(crate) const PIC_STRUCT_FRAME: u32 = 0x1;

/// `NV_ENC_PIC_TYPE_IDR`: a key frame.
pub(crate) const PIC_TYPE_IDR: u32 = 0x3;

/// `NV_ENC_PIC_FLAG_FORCEIDR`: encode the picture as a key frame.
pub(crate) const PIC_FLAG_FORCE_IDR: u32 = 0x2;

/// `NV_ENC_BFRAME_REF_MODE_DISABLED`.
pub(crate) const BFRAME_REF_MODE_DISABLED: u32 = 0;

/// `NV_ENC_TIER_AV1_0`, the main tier.
pub(crate) const TIER_AV1_MAIN: u32 = 0;

/// `NV_ENC_CONFIG_AV1`'s bit field: `repeatSeqHdr`, a sequence header on
/// every key frame.
pub(crate) const AV1_REPEAT_SEQUENCE_HEADER: u32 = 1 << 5;
/// `NV_ENC_CONFIG_AV1`'s bit fields that `outputAnnexBFormat`,
/// `enableTimingInfo`, `enableDecoderModelInfo`, `enableFrameIdNumbers`,
/// `disableSeqHdr` and `enableIntraRefresh` take.
pub(crate) const AV1_UNWANTED: u32 = 0b101_1111;
/// `NV_ENC_CONFIG_AV1`'s bit field `chromaFormatIDC`, two bits from bit 7.
pub(crate) const AV1_CHROMA_FORMAT_SHIFT: u32 = 7;
/// The mask of `chromaFormatIDC` once shifted.
pub(crate) const AV1_CHROMA_FORMAT_MASK: u32 = 0b11;
/// `chromaFormatIDC` for 4:2:0.
pub(crate) const AV1_CHROMA_420: u32 = 1;
/// `NV_ENC_CONFIG_AV1`'s bit fields `enableBitstreamPadding`,
/// `enableCustomTileConfig` and `enableFilmGrainParams`, bits 9 to 11.
pub(crate) const AV1_PADDING_TILES_GRAIN: u32 = 0b111 << 9;
/// `NV_ENC_CONFIG_AV1`'s bit fields `inputPixelBitDepthMinus8` and
/// `pixelBitDepthMinus8`, three bits each from bit 12: 8-bit input and
/// output when clear.
pub(crate) const AV1_BIT_DEPTHS: u32 = 0b11_1111 << 12;

/// `NV_ENC_RC_PARAMS`'s bit fields `enableMinQP` to `strictGOPTarget` and
/// `aqStrength`, bits 0 to 15, which constant quantizers want clear.
pub(crate) const RC_ADAPTIVE: u32 = 0xffff;

/// `NV_ENC_VUI_COLOR_PRIMARIES_BT709`.
pub(crate) const COLOR_PRIMARIES_BT709: u32 = 1;
/// `NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SRGB`.
pub(crate) const TRANSFER_SRGB: u32 = 13;
/// `NV_ENC_VUI_MATRIX_COEFFS_BT709`.
pub(crate) const MATRIX_BT709: u32 = 1;

/// A COM-style GUID, `GUID`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Guid {
    /// The first 4 bytes.
    pub data1: u32,
    /// The next 2.
    pub data2: u16,
    /// The next 2.
    pub data3: u16,
    /// The last 8.
    pub data4: [u8; 8],
}

/// `NV_ENC_CODEC_AV1_GUID`.
pub(crate) const CODEC_AV1_GUID: Guid = Guid {
    data1: 0x0a35_2289,
    data2: 0x0aa7,
    data3: 0x4759,
    data4: [0x86, 0x2d, 0x5d, 0x15, 0xcd, 0x16, 0xd2, 0x54],
};

/// `NV_ENC_AV1_PROFILE_MAIN_GUID`: profile 0, 8 or 10-bit 4:2:0.
pub(crate) const AV1_PROFILE_MAIN_GUID: Guid = Guid {
    data1: 0x5f2a_39f5,
    data2: 0xf14e,
    data3: 0x4f95,
    data4: [0x9a, 0x9e, 0xb7, 0x6d, 0x56, 0x8f, 0xcf, 0x97],
};

/// `NV_ENC_PRESET_P1_GUID`: the fastest preset.
pub(crate) const PRESET_P1_GUID: Guid = Guid {
    data1: 0xfc0a_8d3e,
    data2: 0x45f8,
    data3: 0x4cf8,
    data4: [0x80, 0xc7, 0x29, 0x88, 0x71, 0x59, 0x0e, 0xbf],
};

/// An opaque pointer of NVENC's, such as an encoder or a buffer.
pub(crate) type Handle = *mut c_void;

/// `NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS`.
#[repr(C)]
pub(crate) struct OpenEncodeSessionExParams {
    /// The structure's version.
    pub version: u32,
    /// The kind of device.
    pub device_type: u32,
    /// The device.
    pub device: Handle,
    /// Reserved.
    pub reserved: Handle,
    /// [`API_VERSION`].
    pub api_version: u32,
    /// Reserved.
    pub reserved1: [u32; 253],
    /// Reserved.
    pub reserved2: [Handle; 64],
}

/// `NV_ENC_QP`: a quantizer for each kind of frame.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Qp {
    /// For P frames.
    pub inter_p: u32,
    /// For B frames.
    pub inter_b: u32,
    /// For intra frames.
    pub intra: u32,
}

/// `NV_ENC_RC_PARAMS`: rate control.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct RcParams {
    /// The structure's version.
    pub version: u32,
    /// The rate control mode.
    pub rate_control_mode: u32,
    /// The quantizers in [`RC_CONST_QP`] mode.
    pub const_qp: Qp,
    /// The average bit rate.
    pub average_bit_rate: u32,
    /// The highest bit rate.
    pub max_bit_rate: u32,
    /// The VBV buffer's size.
    pub vbv_buffer_size: u32,
    /// The VBV buffer's initial delay.
    pub vbv_initial_delay: u32,
    /// The bit fields from `enableMinQP` to `aqStrength`.
    pub flags: u32,
    /// The lowest quantizers.
    pub min_qp: Qp,
    /// The highest quantizers.
    pub max_qp: Qp,
    /// The first quantizers.
    pub initial_rc_qp: Qp,
    /// The temporal layers' mask.
    pub temporal_layer_idx_mask: u32,
    /// The temporal layers' quantizers.
    pub temporal_layer_qp: [u8; 8],
    /// The target quality.
    pub target_quality: u8,
    /// The target quality's fraction.
    pub target_quality_lsb: u8,
    /// The lookahead's depth.
    pub lookahead_depth: u16,
    /// The low-delay key frames' scale.
    pub low_delay_key_frame_scale: u8,
    /// The luma DC quantizer's offset.
    pub y_dc_qp_index_offset: i8,
    /// The Cb DC quantizer's offset.
    pub u_dc_qp_index_offset: i8,
    /// The Cr DC quantizer's offset.
    pub v_dc_qp_index_offset: i8,
    /// The quantizer map's mode.
    pub qp_map_mode: u32,
    /// The multi-pass mode.
    pub multi_pass: u32,
    /// The alpha layer's share of the bit rate.
    pub alpha_layer_bitrate_ratio: u32,
    /// The Cb quantizer's offset.
    pub cb_qp_index_offset: i8,
    /// The Cr quantizer's offset.
    pub cr_qp_index_offset: i8,
    /// Reserved.
    pub reserved2: u16,
    /// Reserved.
    pub reserved: [u32; 4],
}

/// `NV_ENC_CONFIG_AV1`: AV1's encoding parameters.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ConfigAv1 {
    /// The level, `seq_level_idx`.
    pub level: u32,
    /// The tier.
    pub tier: u32,
    /// The smallest partition.
    pub min_part_size: u32,
    /// The largest partition.
    pub max_part_size: u32,
    /// The bit fields from `outputAnnexBFormat` to `pixelBitDepthMinus8`.
    pub flags: u32,
    /// The key frame interval.
    pub idr_period: u32,
    /// The intra refresh interval.
    pub intra_refresh_period: u32,
    /// The intra refresh's length.
    pub intra_refresh_cnt: u32,
    /// The most reference frames held.
    pub max_num_ref_frames_in_dpb: u32,
    /// The tile columns, a power of 2, or 0 for the fewest allowed.
    pub num_tile_columns: u32,
    /// The tile rows, a power of 2, or 0 for the fewest allowed.
    pub num_tile_rows: u32,
    /// Custom tile widths.
    pub tile_widths: *mut u32,
    /// Custom tile heights.
    pub tile_heights: *mut u32,
    /// The temporal layers, less one.
    pub max_temporal_layers_minus1: u32,
    /// `color_primaries`.
    pub color_primaries: u32,
    /// `transfer_characteristics`.
    pub transfer_characteristics: u32,
    /// `matrix_coefficients`.
    pub matrix_coefficients: u32,
    /// `color_range`: 0 for limited.
    pub color_range: u32,
    /// `chroma_sample_position`.
    pub chroma_sample_position: u32,
    /// Whether B frames are references.
    pub use_b_frames_as_ref: u32,
    /// Custom film grain.
    pub film_grain_params: Handle,
    /// The forward references.
    pub num_fwd_refs: u32,
    /// The backward references.
    pub num_bwd_refs: u32,
    /// Reserved.
    pub reserved1: [u32; 235],
    /// Reserved.
    pub reserved2: [Handle; 62],
}

/// The size of `NV_ENC_CODEC_CONFIG`, a union whose largest member is the
/// H.264 one, in 32-bit words.
const CODEC_CONFIG_WORDS: usize = 448;

/// `NV_ENC_CODEC_CONFIG`: the codec's parameters, of which only AV1's are
/// used.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union CodecConfig {
    /// AV1's.
    pub av1: ConfigAv1,
    /// The union's whole size, which other codecs' members fill.
    pub words: [u32; CODEC_CONFIG_WORDS],
}

/// `NV_ENC_CONFIG`: the encoding parameters.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Config {
    /// The structure's version.
    pub version: u32,
    /// The profile.
    pub profile_guid: Guid,
    /// The frames from one key frame to the next.
    pub gop_length: u32,
    /// The pattern of frame types: 1 for P frames only.
    pub frame_interval_p: i32,
    /// Whether the output is monochrome.
    pub mono_chrome_encoding: u32,
    /// Frames or fields.
    pub frame_field_mode: u32,
    /// The motion vectors' precision.
    pub mv_precision: u32,
    /// Rate control.
    pub rc_params: RcParams,
    /// The codec's parameters.
    pub encode_codec_config: CodecConfig,
    /// Reserved.
    pub reserved: [u32; 278],
    /// Reserved.
    pub reserved2: [Handle; 64],
}

/// `NVENC_EXTERNAL_ME_HINT_COUNTS_PER_BLOCKTYPE`, unused.
pub(crate) type MeHintCounts = [u32; 4];

/// `NV_ENC_INITIALIZE_PARAMS`: the encoder's setup.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct InitializeParams {
    /// The structure's version.
    pub version: u32,
    /// The codec.
    pub encode_guid: Guid,
    /// The preset.
    pub preset_guid: Guid,
    /// The frames' width.
    pub encode_width: u32,
    /// The frames' height.
    pub encode_height: u32,
    /// The display aspect ratio's width.
    pub dar_width: u32,
    /// The display aspect ratio's height.
    pub dar_height: u32,
    /// The frame rate's numerator.
    pub frame_rate_num: u32,
    /// The frame rate's denominator.
    pub frame_rate_den: u32,
    /// Whether encoding is asynchronous.
    pub enable_encode_async: u32,
    /// Whether the encoder decides the pictures' types.
    pub enable_ptd: u32,
    /// The bit fields from `reportSliceOffsets` to `enableOutputInVidmem`.
    pub flags: u32,
    /// The private data's size.
    pub priv_data_size: u32,
    /// Private data.
    pub priv_data: Handle,
    /// The encoding parameters.
    pub encode_config: *mut Config,
    /// The largest width a reconfiguration may set.
    pub max_encode_width: u32,
    /// The largest height a reconfiguration may set.
    pub max_encode_height: u32,
    /// External motion hints' counts.
    pub max_me_hint_counts_per_block: [MeHintCounts; 2],
    /// The tuning.
    pub tuning_info: u32,
    /// The input buffers' format, for Direct3D 12 sessions.
    pub buffer_format: u32,
    /// Reserved.
    pub reserved: [u32; 287],
    /// Reserved.
    pub reserved2: [Handle; 64],
}

/// `NV_ENC_PRESET_CONFIG`: a preset's encoding parameters.
#[repr(C)]
pub(crate) struct PresetConfig {
    /// The structure's version.
    pub version: u32,
    /// The parameters.
    pub preset_cfg: Config,
    /// Reserved.
    pub reserved1: [u32; 255],
    /// Reserved.
    pub reserved2: [Handle; 64],
}

/// `NV_ENC_RECONFIGURE_PARAMS`: new parameters for an encoder.
#[repr(C)]
pub(crate) struct ReconfigureParams {
    /// The structure's version.
    pub version: u32,
    /// The new setup.
    pub re_init_encode_params: InitializeParams,
    /// The bit fields `resetEncoder` and `forceIDR`.
    pub flags: u32,
}

/// `NV_ENC_CREATE_INPUT_BUFFER`.
#[repr(C)]
pub(crate) struct CreateInputBuffer {
    /// The structure's version.
    pub version: u32,
    /// The buffer's width.
    pub width: u32,
    /// The buffer's height.
    pub height: u32,
    /// Deprecated.
    pub memory_heap: u32,
    /// The buffer's format.
    pub buffer_fmt: u32,
    /// Reserved.
    pub reserved: u32,
    /// The buffer made.
    pub input_buffer: Handle,
    /// A system memory buffer to use.
    pub sys_mem_buffer: Handle,
    /// Reserved.
    pub reserved1: [u32; 57],
    /// Reserved.
    pub reserved2: [Handle; 63],
}

/// `NV_ENC_LOCK_INPUT_BUFFER`.
#[repr(C)]
pub(crate) struct LockInputBuffer {
    /// The structure's version.
    pub version: u32,
    /// The bit field `doNotWait`.
    pub flags: u32,
    /// The buffer to lock.
    pub input_buffer: Handle,
    /// Where the buffer's pixels are while locked.
    pub buffer_data_ptr: *mut c_void,
    /// The bytes from one row to the next.
    pub pitch: u32,
    /// Reserved.
    pub reserved1: [u32; 251],
    /// Reserved.
    pub reserved2: [Handle; 64],
}

/// `NV_ENC_CREATE_BITSTREAM_BUFFER`.
#[repr(C)]
pub(crate) struct CreateBitstreamBuffer {
    /// The structure's version.
    pub version: u32,
    /// Deprecated.
    pub size: u32,
    /// Deprecated.
    pub memory_heap: u32,
    /// Reserved.
    pub reserved: u32,
    /// The buffer made.
    pub bitstream_buffer: Handle,
    /// Reserved.
    pub bitstream_buffer_ptr: *mut c_void,
    /// Reserved.
    pub reserved1: [u32; 58],
    /// Reserved.
    pub reserved2: [Handle; 64],
}

/// `NV_ENC_PIC_PARAMS_AV1`: AV1's parameters for one picture.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct PicParamsAv1 {
    /// The display order's syntax.
    pub display_poc_syntax: u32,
    /// Whether the picture is a reference.
    pub ref_pic_flag: u32,
    /// The temporal layer.
    pub temporal_id: u32,
    /// Forces an intra refresh.
    pub force_intra_refresh_with_frame_cnt: u32,
    /// The bit fields from `goldenFrameFlag` to `filmGrainParamsUpdate`.
    pub flags: u32,
    /// New tile columns.
    pub num_tile_columns: u32,
    /// New tile rows.
    pub num_tile_rows: u32,
    /// Custom tile widths.
    pub tile_widths: *mut u32,
    /// Custom tile heights.
    pub tile_heights: *mut u32,
    /// The OBUs to insert's count.
    pub obu_payload_array_cnt: u32,
    /// Reserved.
    pub reserved: u32,
    /// OBUs to insert.
    pub obu_payload_array: Handle,
    /// New film grain.
    pub film_grain_params: Handle,
    /// Reserved.
    pub reserved2: [u32; 247],
    /// Reserved.
    pub reserved3: [Handle; 61],
}

/// The size of `NV_ENC_CODEC_PIC_PARAMS`, a union whose largest member is
/// the AV1 one, in 32-bit words.
const CODEC_PIC_PARAMS_WORDS: usize = 388;

/// `NV_ENC_CODEC_PIC_PARAMS`: the codec's parameters for one picture.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) union CodecPicParams {
    /// AV1's.
    pub av1: PicParamsAv1,
    /// The union's whole size, which other codecs' members fill.
    pub words: [u32; CODEC_PIC_PARAMS_WORDS],
}

/// `NV_ENC_PIC_PARAMS`: one picture to encode.
#[repr(C)]
pub(crate) struct PicParams {
    /// The structure's version.
    pub version: u32,
    /// The input's width.
    pub input_width: u32,
    /// The input's height.
    pub input_height: u32,
    /// The input's bytes from one row to the next.
    pub input_pitch: u32,
    /// [`PIC_FLAG_FORCE_IDR`] and other flags.
    pub encode_pic_flags: u32,
    /// The picture's index.
    pub frame_idx: u32,
    /// A value the encoded picture carries back.
    pub input_time_stamp: u64,
    /// The picture's duration.
    pub input_duration: u64,
    /// The input buffer.
    pub input_buffer: Handle,
    /// The bitstream buffer.
    pub output_bitstream: Handle,
    /// The event signalled once encoded, in asynchronous mode.
    pub completion_event: Handle,
    /// The input's format.
    pub buffer_fmt: u32,
    /// Frames or fields.
    pub picture_struct: u32,
    /// The picture's type, unless the encoder decides it.
    pub picture_type: u32,
    /// The codec's parameters.
    pub codec_pic_params: CodecPicParams,
    /// External motion hints' counts.
    pub me_hint_counts_per_block: [MeHintCounts; 2],
    /// External motion hints.
    pub me_external_hints: Handle,
    /// Reserved.
    pub reserved1: [u32; 6],
    /// Reserved.
    pub reserved2: [Handle; 2],
    /// A quantizer delta for each block.
    pub qp_delta_map: *mut i8,
    /// The delta map's size.
    pub qp_delta_map_size: u32,
    /// Reserved.
    pub reserved_bit_fields: u32,
    /// External motion hints' distances.
    pub me_hint_ref_pic_dist: [u16; 2],
    /// An alpha layer's input.
    pub alpha_buffer: Handle,
    /// External superblock motion hints.
    pub me_external_sb_hints: Handle,
    /// Their count.
    pub me_sb_hints_count: u32,
    /// Reserved.
    pub reserved3: [u32; 285],
    /// Reserved.
    pub reserved4: [Handle; 58],
}

/// `NV_ENC_LOCK_BITSTREAM`: an encoded picture, while locked.
#[repr(C)]
pub(crate) struct LockBitstream {
    /// The structure's version.
    pub version: u32,
    /// The bit fields `doNotWait`, `ltrFrame` and `getRCStats`.
    pub flags: u32,
    /// The bitstream buffer to lock.
    pub output_bitstream: Handle,
    /// The slices' offsets.
    pub slice_offsets: *mut u32,
    /// The picture's index.
    pub frame_idx: u32,
    /// The hardware's status.
    pub hw_encode_status: u32,
    /// The slices.
    pub num_slices: u32,
    /// The encoded picture's size.
    pub bitstream_size_in_bytes: u32,
    /// The picture's `input_time_stamp`.
    pub output_time_stamp: u64,
    /// The picture's duration.
    pub output_duration: u64,
    /// Where the encoded picture is while locked.
    pub bitstream_buffer_ptr: *mut c_void,
    /// The picture's type.
    pub picture_type: u32,
    /// Frames or fields.
    pub picture_struct: u32,
    /// The average quantizer.
    pub frame_avg_qp: u32,
    /// The sum of absolute transformed differences.
    pub frame_satd: u32,
    /// The long-term reference index.
    pub ltr_frame_idx: u32,
    /// The long-term references.
    pub ltr_frame_bitmap: u32,
    /// The temporal layer.
    pub temporal_id: u32,
    /// Reserved.
    pub reserved: [u32; 12],
    /// The intra blocks.
    pub intra_mb_count: u32,
    /// The inter blocks.
    pub inter_mb_count: u32,
    /// The average horizontal motion.
    pub average_mvx: i32,
    /// The average vertical motion.
    pub average_mvy: i32,
    /// The alpha layer's size.
    pub alpha_layer_size_in_bytes: u32,
    /// Reserved.
    pub reserved1: [u32; 218],
    /// Reserved.
    pub reserved2: [Handle; 64],
}

/// A function of the list the encoder does not call.
pub(crate) type Unused = Option<unsafe extern "system" fn()>;

/// `NV_ENCODE_API_FUNCTION_LIST`: NVENC's functions, as
/// `NvEncodeAPICreateInstance` fills them in.
#[repr(C)]
pub(crate) struct FunctionList {
    /// The structure's version.
    pub version: u32,
    /// Reserved.
    pub reserved: u32,
    /// `nvEncOpenEncodeSession`.
    pub open_encode_session: Unused,
    /// `nvEncGetEncodeGUIDCount`.
    pub get_encode_guid_count:
        Option<unsafe extern "system" fn(encoder: Handle, count: *mut u32) -> Status>,
    /// `nvEncGetEncodeProfileGUIDCount`.
    pub get_encode_profile_guid_count: Unused,
    /// `nvEncGetEncodeProfileGUIDs`.
    pub get_encode_profile_guids: Unused,
    /// `nvEncGetEncodeGUIDs`.
    pub get_encode_guids: Option<
        unsafe extern "system" fn(
            encoder: Handle,
            guids: *mut Guid,
            capacity: u32,
            count: *mut u32,
        ) -> Status,
    >,
    /// `nvEncGetInputFormatCount`.
    pub get_input_format_count: Unused,
    /// `nvEncGetInputFormats`.
    pub get_input_formats: Unused,
    /// `nvEncGetEncodeCaps`.
    pub get_encode_caps: Unused,
    /// `nvEncGetEncodePresetCount`.
    pub get_encode_preset_count: Unused,
    /// `nvEncGetEncodePresetGUIDs`.
    pub get_encode_preset_guids: Unused,
    /// `nvEncGetEncodePresetConfig`.
    pub get_encode_preset_config: Unused,
    /// `nvEncInitializeEncoder`.
    pub initialize_encoder:
        Option<unsafe extern "system" fn(encoder: Handle, params: *mut InitializeParams) -> Status>,
    /// `nvEncCreateInputBuffer`.
    pub create_input_buffer: Option<
        unsafe extern "system" fn(encoder: Handle, params: *mut CreateInputBuffer) -> Status,
    >,
    /// `nvEncDestroyInputBuffer`.
    pub destroy_input_buffer:
        Option<unsafe extern "system" fn(encoder: Handle, buffer: Handle) -> Status>,
    /// `nvEncCreateBitstreamBuffer`.
    pub create_bitstream_buffer: Option<
        unsafe extern "system" fn(encoder: Handle, params: *mut CreateBitstreamBuffer) -> Status,
    >,
    /// `nvEncDestroyBitstreamBuffer`.
    pub destroy_bitstream_buffer:
        Option<unsafe extern "system" fn(encoder: Handle, buffer: Handle) -> Status>,
    /// `nvEncEncodePicture`.
    pub encode_picture:
        Option<unsafe extern "system" fn(encoder: Handle, params: *mut PicParams) -> Status>,
    /// `nvEncLockBitstream`.
    pub lock_bitstream:
        Option<unsafe extern "system" fn(encoder: Handle, params: *mut LockBitstream) -> Status>,
    /// `nvEncUnlockBitstream`.
    pub unlock_bitstream:
        Option<unsafe extern "system" fn(encoder: Handle, buffer: Handle) -> Status>,
    /// `nvEncLockInputBuffer`.
    pub lock_input_buffer:
        Option<unsafe extern "system" fn(encoder: Handle, params: *mut LockInputBuffer) -> Status>,
    /// `nvEncUnlockInputBuffer`.
    pub unlock_input_buffer:
        Option<unsafe extern "system" fn(encoder: Handle, buffer: Handle) -> Status>,
    /// `nvEncGetEncodeStats`.
    pub get_encode_stats: Unused,
    /// `nvEncGetSequenceParams`.
    pub get_sequence_params: Unused,
    /// `nvEncRegisterAsyncEvent`.
    pub register_async_event: Unused,
    /// `nvEncUnregisterAsyncEvent`.
    pub unregister_async_event: Unused,
    /// `nvEncMapInputResource`.
    pub map_input_resource: Unused,
    /// `nvEncUnmapInputResource`.
    pub unmap_input_resource: Unused,
    /// `nvEncDestroyEncoder`.
    pub destroy_encoder: Option<unsafe extern "system" fn(encoder: Handle) -> Status>,
    /// `nvEncInvalidateRefFrames`.
    pub invalidate_ref_frames: Unused,
    /// `nvEncOpenEncodeSessionEx`.
    pub open_encode_session_ex: Option<
        unsafe extern "system" fn(
            params: *mut OpenEncodeSessionExParams,
            encoder: *mut Handle,
        ) -> Status,
    >,
    /// `nvEncRegisterResource`.
    pub register_resource: Unused,
    /// `nvEncUnregisterResource`.
    pub unregister_resource: Unused,
    /// `nvEncReconfigureEncoder`.
    pub reconfigure_encoder: Option<
        unsafe extern "system" fn(encoder: Handle, params: *mut ReconfigureParams) -> Status,
    >,
    /// Reserved.
    pub reserved1: Handle,
    /// `nvEncCreateMVBuffer`.
    pub create_mv_buffer: Unused,
    /// `nvEncDestroyMVBuffer`.
    pub destroy_mv_buffer: Unused,
    /// `nvEncRunMotionEstimationOnly`.
    pub run_motion_estimation_only: Unused,
    /// `nvEncGetLastErrorString`.
    pub get_last_error_string: Option<unsafe extern "system" fn(encoder: Handle) -> *const c_char>,
    /// `nvEncSetIOCudaStreams`.
    pub set_io_cuda_streams: Unused,
    /// `nvEncGetEncodePresetConfigEx`.
    pub get_encode_preset_config_ex: Option<
        unsafe extern "system" fn(
            encoder: Handle,
            encode_guid: Guid,
            preset_guid: Guid,
            tuning_info: u32,
            preset_config: *mut PresetConfig,
        ) -> Status,
    >,
    /// `nvEncGetSequenceParamEx`.
    pub get_sequence_param_ex: Unused,
    /// Reserved.
    pub reserved2: [Handle; 277],
}

/// `NvEncodeAPICreateInstance`, which fills in the function list.
pub(crate) type CreateInstance = unsafe extern "system" fn(list: *mut FunctionList) -> Status;

/// `NvEncodeAPIGetMaxSupportedVersion`, which reports the newest interface
/// version the driver has, as [`API_VERSION_REPORTED`] gives versions.
pub(crate) type GetMaxSupportedVersion = unsafe extern "system" fn(version: *mut u32) -> Status;

/// Returns a structure of NVENC's with every field zero, as NVENC wants
/// what it reserves.
///
/// # Safety
///
/// `T` must be one of this module's structures, all of whose fields are
/// integers, null pointers, `None` functions or arrays of them when zero.
#[cfg(windows)]
pub(crate) unsafe fn zeroed<T>() -> T {
    // SAFETY: the caller passes a structure that is valid when zeroed.
    unsafe { std::mem::zeroed() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// The structures' sizes on x86-64, as clang lays out `nvEncodeAPI.h`.
    #[test]
    fn sizes_match_the_header() {
        assert_eq!(size_of::<Guid>(), 16);
        assert_eq!(size_of::<OpenEncodeSessionExParams>(), 1552);
        assert_eq!(size_of::<Qp>(), 12);
        assert_eq!(size_of::<RcParams>(), 128);
        assert_eq!(size_of::<ConfigAv1>(), 1552);
        assert_eq!(size_of::<CodecConfig>(), 1792);
        assert_eq!(size_of::<Config>(), 3584);
        assert_eq!(size_of::<InitializeParams>(), 1808);
        assert_eq!(size_of::<PresetConfig>(), 5128);
        assert_eq!(size_of::<ReconfigureParams>(), 1824);
        assert_eq!(size_of::<CreateInputBuffer>(), 776);
        assert_eq!(size_of::<LockInputBuffer>(), 1544);
        assert_eq!(size_of::<CreateBitstreamBuffer>(), 776);
        assert_eq!(size_of::<PicParamsAv1>(), 1552);
        assert_eq!(size_of::<CodecPicParams>(), 1552);
        assert_eq!(size_of::<PicParams>(), 3360);
        assert_eq!(size_of::<LockBitstream>(), 1544);
        assert_eq!(size_of::<FunctionList>(), 2552);
    }

    /// The fields the encoder sets lie where the header puts them.
    #[test]
    fn offsets_match_the_header() {
        assert_eq!(offset_of!(OpenEncodeSessionExParams, api_version), 24);
        assert_eq!(offset_of!(RcParams, flags), 36);
        assert_eq!(offset_of!(RcParams, temporal_layer_idx_mask), 76);
        assert_eq!(offset_of!(RcParams, min_qp), 40);
        assert_eq!(offset_of!(RcParams, qp_map_mode), 96);
        assert_eq!(offset_of!(RcParams, reserved), 112);
        assert_eq!(offset_of!(ConfigAv1, flags), 16);
        assert_eq!(offset_of!(ConfigAv1, num_tile_columns), 36);
        assert_eq!(offset_of!(ConfigAv1, tile_widths), 48);
        assert_eq!(offset_of!(ConfigAv1, color_primaries), 68);
        assert_eq!(offset_of!(ConfigAv1, film_grain_params), 96);
        assert_eq!(offset_of!(ConfigAv1, num_fwd_refs), 104);
        assert_eq!(offset_of!(ConfigAv1, reserved2), 1056);
        assert_eq!(offset_of!(Config, gop_length), 20);
        assert_eq!(offset_of!(Config, rc_params), 40);
        assert_eq!(offset_of!(Config, encode_codec_config), 168);
        assert_eq!(offset_of!(Config, reserved2), 3072);
        assert_eq!(offset_of!(InitializeParams, encode_width), 36);
        assert_eq!(offset_of!(InitializeParams, enable_ptd), 64);
        assert_eq!(offset_of!(InitializeParams, priv_data), 80);
        assert_eq!(offset_of!(InitializeParams, encode_config), 88);
        assert_eq!(offset_of!(InitializeParams, tuning_info), 136);
        assert_eq!(offset_of!(InitializeParams, reserved2), 1296);
        assert_eq!(offset_of!(PresetConfig, preset_cfg), 8);
        assert_eq!(offset_of!(PresetConfig, reserved1), 3592);
        assert_eq!(offset_of!(ReconfigureParams, re_init_encode_params), 8);
        assert_eq!(offset_of!(ReconfigureParams, flags), 1816);
        assert_eq!(offset_of!(CreateInputBuffer, input_buffer), 24);
        assert_eq!(offset_of!(CreateInputBuffer, reserved2), 272);
        assert_eq!(offset_of!(LockInputBuffer, buffer_data_ptr), 16);
        assert_eq!(offset_of!(LockInputBuffer, pitch), 24);
        assert_eq!(offset_of!(LockInputBuffer, reserved2), 1032);
        assert_eq!(offset_of!(CreateBitstreamBuffer, bitstream_buffer), 16);
        assert_eq!(offset_of!(CreateBitstreamBuffer, reserved2), 264);
        assert_eq!(offset_of!(PicParamsAv1, num_tile_columns), 20);
        assert_eq!(offset_of!(PicParamsAv1, obu_payload_array), 56);
        assert_eq!(offset_of!(PicParamsAv1, reserved3), 1064);
        assert_eq!(offset_of!(PicParams, input_time_stamp), 24);
        assert_eq!(offset_of!(PicParams, input_buffer), 40);
        assert_eq!(offset_of!(PicParams, output_bitstream), 48);
        assert_eq!(offset_of!(PicParams, buffer_fmt), 64);
        assert_eq!(offset_of!(PicParams, picture_type), 72);
        assert_eq!(offset_of!(PicParams, codec_pic_params), 80);
        assert_eq!(offset_of!(PicParams, me_hint_counts_per_block), 1632);
        assert_eq!(offset_of!(PicParams, qp_delta_map), 1712);
        assert_eq!(offset_of!(PicParams, me_hint_ref_pic_dist), 1728);
        assert_eq!(offset_of!(PicParams, alpha_buffer), 1736);
        assert_eq!(offset_of!(PicParams, me_sb_hints_count), 1752);
        assert_eq!(offset_of!(PicParams, reserved4), 2896);
        assert_eq!(offset_of!(LockBitstream, output_bitstream), 8);
        assert_eq!(offset_of!(LockBitstream, bitstream_size_in_bytes), 36);
        assert_eq!(offset_of!(LockBitstream, output_time_stamp), 40);
        assert_eq!(offset_of!(LockBitstream, bitstream_buffer_ptr), 56);
        assert_eq!(offset_of!(LockBitstream, picture_type), 64);
        assert_eq!(offset_of!(LockBitstream, reserved), 92);
        assert_eq!(offset_of!(LockBitstream, alpha_layer_size_in_bytes), 156);
        assert_eq!(offset_of!(LockBitstream, reserved2), 1032);
        assert_eq!(offset_of!(FunctionList, get_encode_guid_count), 16);
        assert_eq!(offset_of!(FunctionList, get_encode_guids), 40);
        assert_eq!(offset_of!(FunctionList, initialize_encoder), 96);
        assert_eq!(offset_of!(FunctionList, destroy_encoder), 224);
        assert_eq!(offset_of!(FunctionList, open_encode_session_ex), 240);
        assert_eq!(offset_of!(FunctionList, reconfigure_encoder), 264);
        assert_eq!(offset_of!(FunctionList, get_last_error_string), 304);
        assert_eq!(offset_of!(FunctionList, get_encode_preset_config_ex), 320);
        assert_eq!(offset_of!(FunctionList, reserved2), 336);
    }
}
