//! AV1 sequence and frame headers, written for hardware encoders that take
//! them from the application (VA-API's packed headers).
//!
//! The headers describe the one configuration QDP's hardware encoders use:
//! profile 0, 8-bit 4:2:0 in BT.709 limited range, 64 × 64 superblocks, order
//! hints, CDEF, and no other optional coding tools. Every inter frame refers to
//! the previous frame only, and every frame replaces all eight reference slots,
//! so frames never depend on anything older than the frame before them.

use crate::bits::{BitWriter, write_leb128};
use crate::level::TileCount;

/// The OBU type of a sequence header (AV1 specification, Section 6.2.2).
const OBU_SEQUENCE_HEADER: u8 = 1;

/// The OBU type of a frame header.
const OBU_FRAME_HEADER: u8 = 3;

/// The shift of the type field in an OBU header byte.
const OBU_TYPE_SHIFT: u8 = 3;

/// The OBU header flag that says a size field follows.
const OBU_HAS_SIZE_FIELD: u8 = 0x02;

/// `seq_profile` 0: 8- or 10-bit 4:2:0.
const PROFILE_MAIN: u64 = 0;

/// The highest `seq_level_idx` that has no tier bit.
const MAX_LEVEL_WITHOUT_TIER: u8 = 7;

/// The number of bits of `order_hint`.
pub(crate) const ORDER_HINT_BITS: u32 = 8;

/// `color_primaries` for BT.709.
const COLOR_PRIMARIES_BT709: u64 = 1;

/// `transfer_characteristics` for sRGB.
const TRANSFER_SRGB: u64 = 13;

/// `matrix_coefficients` for BT.709.
const MATRIX_BT709: u64 = 1;

/// `chroma_sample_position` when unknown.
const CHROMA_SAMPLE_POSITION_UNKNOWN: u64 = 0;

/// `frame_type` of a key frame.
const KEY_FRAME_TYPE: u64 = 0;

/// `frame_type` of an inter frame.
const INTER_FRAME_TYPE: u64 = 1;

/// `primary_ref_frame` naming the first reference, whose probabilities an
/// inter frame starts from.
pub(crate) const PRIMARY_REF_LAST: u8 = 0;

/// `primary_ref_frame` when no reference provides probabilities.
pub(crate) const PRIMARY_REF_NONE: u8 = 7;

/// `refresh_frame_flags` that replaces every reference slot.
pub(crate) const ALL_REFERENCE_SLOTS: u8 = 0xff;

/// The number of references an inter frame names (`REFS_PER_FRAME`).
pub(crate) const REFS_PER_FRAME: usize = 7;

/// The reference slot every reference of an inter frame names.
pub(crate) const PREVIOUS_FRAME_SLOT: u8 = 0;

/// `interpolation_filter` for the regular eight-tap filter.
pub(crate) const INTERPOLATION_EIGHTTAP: u8 = 0;

/// `cdef_damping_minus_3`, for a damping of 5.
pub(crate) const CDEF_DAMPING_MINUS_3: u8 = 2;

/// The width of a superblock in luma samples.
pub(crate) const SUPERBLOCK_SIZE: u32 = 64;

/// The widest tile in luma samples (`MAX_TILE_WIDTH`).
const MAX_TILE_WIDTH: u32 = 4096;

/// The largest tile in luma samples (`MAX_TILE_AREA`).
const MAX_TILE_AREA: u32 = 4096 * 2304;

/// The most tile columns or rows (`MAX_TILE_COLS`, `MAX_TILE_ROWS`).
const MAX_TILE_COLS: u32 = 64;

/// Returns the number of bits needed to write `value`, at least one.
fn bit_length(value: u32) -> u32 {
    (u32::BITS - value.leading_zeros()).max(1)
}

/// The smallest `k` for which `block << k` reaches `target` (`tile_log2`).
fn tile_log2(block: u32, target: u32) -> u32 {
    let mut k = 0;
    while (block << k) < target {
        k += 1;
    }
    k
}

/// How a frame is divided into tiles of uniform size (`tile_info()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TileLayout {
    /// `TileColsLog2`.
    pub(crate) cols_log2: u32,
    /// `TileRowsLog2`.
    pub(crate) rows_log2: u32,
    /// The smallest `TileColsLog2` the frame width allows.
    min_cols_log2: u32,
    /// The largest `TileColsLog2` the frame width allows.
    max_cols_log2: u32,
    /// The smallest `TileRowsLog2`, given `cols_log2`.
    min_rows_log2: u32,
    /// The largest `TileRowsLog2` the frame height allows.
    max_rows_log2: u32,
    /// The number of tile columns.
    pub(crate) cols: u32,
    /// The number of tile rows.
    pub(crate) rows: u32,
    /// The width of every tile column but the last, in superblocks.
    pub(crate) col_width_sb: u32,
    /// The height of every tile row but the last, in superblocks.
    pub(crate) row_height_sb: u32,
}

impl TileLayout {
    /// Returns the layout with at least `min_tiles` tiles, as close to square
    /// as the frame allows, for a `width × height` frame.
    pub(crate) fn new(width: u32, height: u32, min_tiles: usize) -> Self {
        // Mode info units are 4 × 4 luma samples, rounded up to 8 × 8.
        let mi_cols = 2 * width.div_ceil(8);
        let mi_rows = 2 * height.div_ceil(8);
        let mi_per_superblock = SUPERBLOCK_SIZE / 4;
        let sb_cols = mi_cols.div_ceil(mi_per_superblock);
        let sb_rows = mi_rows.div_ceil(mi_per_superblock);
        let superblock_area = SUPERBLOCK_SIZE * SUPERBLOCK_SIZE;

        let min_cols_log2 = tile_log2(MAX_TILE_WIDTH / SUPERBLOCK_SIZE, sb_cols);
        let max_cols_log2 = tile_log2(1, sb_cols.min(MAX_TILE_COLS));
        let max_rows_log2 = tile_log2(1, sb_rows.min(MAX_TILE_COLS));
        let min_log2_tiles = min_cols_log2.max(tile_log2(
            MAX_TILE_AREA / superblock_area,
            sb_cols * sb_rows,
        ));

        let wanted_log2 = tile_log2(1, u32::try_from(min_tiles).unwrap_or(u32::MAX));
        let cols_log2 = wanted_log2
            .div_ceil(2)
            .max(min_cols_log2)
            .min(max_cols_log2);
        let min_rows_log2 = min_log2_tiles.saturating_sub(cols_log2);
        let rows_log2 = wanted_log2
            .saturating_sub(cols_log2)
            .max(min_rows_log2)
            .min(max_rows_log2);

        let col_width_sb = sb_cols.div_ceil(1 << cols_log2);
        let row_height_sb = sb_rows.div_ceil(1 << rows_log2);
        Self {
            cols_log2,
            rows_log2,
            min_cols_log2,
            max_cols_log2,
            min_rows_log2,
            max_rows_log2,
            cols: sb_cols.div_ceil(col_width_sb),
            rows: sb_rows.div_ceil(row_height_sb),
            col_width_sb,
            row_height_sb,
        }
    }

    /// Returns the number of tiles.
    pub(crate) fn count(&self) -> u32 {
        self.cols * self.rows
    }

    /// Returns the number of tiles and tile columns, for choosing a level.
    pub(crate) fn count_and_columns(&self) -> TileCount {
        TileCount {
            tiles: self.count(),
            columns: self.cols,
        }
    }

    /// Writes `tile_info()` with `uniform_tile_spacing_flag` set.
    fn write(&self, writer: &mut BitWriter, tile_size_bytes: u8) {
        writer.bit(true); // uniform_tile_spacing_flag
        for _ in self.min_cols_log2..self.cols_log2 {
            writer.bit(true); // increment_tile_cols_log2
        }
        if self.cols_log2 < self.max_cols_log2 {
            writer.bit(false); // increment_tile_cols_log2
        }
        for _ in self.min_rows_log2..self.rows_log2 {
            writer.bit(true); // increment_tile_rows_log2
        }
        if self.rows_log2 < self.max_rows_log2 {
            writer.bit(false); // increment_tile_rows_log2
        }
        if self.cols_log2 > 0 || self.rows_log2 > 0 {
            writer.bits(0, self.cols_log2 + self.rows_log2); // context_update_tile_id
            writer.bits(u64::from(tile_size_bytes - 1), 2); // tile_size_bytes_minus_1
        }
    }
}

/// What every header of a stream shares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StreamFormat {
    /// The frame width in luma samples.
    pub(crate) width: u32,
    /// The frame height in luma samples.
    pub(crate) height: u32,
    /// The `seq_level_idx` the stream meets.
    pub(crate) level: u8,
}

/// Wraps `payload` in an OBU of `obu_type` with a size field.
fn obu(obu_type: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut bytes = vec![(obu_type << OBU_TYPE_SHIFT) | OBU_HAS_SIZE_FIELD];
    write_leb128(&mut bytes, payload.len() as u64);
    bytes.extend(payload);
    bytes
}

/// Returns the sequence header OBU of a stream.
pub(crate) fn sequence_header_obu(format: &StreamFormat) -> Vec<u8> {
    let mut writer = BitWriter::new();
    writer.bits(PROFILE_MAIN, 3); // seq_profile
    writer.bit(false); // still_picture
    writer.bit(false); // reduced_still_picture_header
    writer.bit(false); // timing_info_present_flag
    writer.bit(false); // initial_display_delay_present_flag
    writer.bits(0, 5); // operating_points_cnt_minus_1
    writer.bits(0, 12); // operating_point_idc[0]
    writer.bits(u64::from(format.level), 5); // seq_level_idx[0]
    if format.level > MAX_LEVEL_WITHOUT_TIER {
        writer.bit(false); // seq_tier[0]: Main
    }
    let width_bits = bit_length(format.width - 1);
    let height_bits = bit_length(format.height - 1);
    writer.bits(u64::from(width_bits - 1), 4); // frame_width_bits_minus_1
    writer.bits(u64::from(height_bits - 1), 4); // frame_height_bits_minus_1
    writer.bits(u64::from(format.width - 1), width_bits); // max_frame_width_minus_1
    writer.bits(u64::from(format.height - 1), height_bits); // max_frame_height_minus_1
    writer.bit(false); // frame_id_numbers_present_flag
    writer.bit(false); // use_128x128_superblock
    writer.bit(false); // enable_filter_intra
    writer.bit(false); // enable_intra_edge_filter
    writer.bit(false); // enable_interintra_compound
    writer.bit(false); // enable_masked_compound
    writer.bit(false); // enable_warped_motion
    writer.bit(false); // enable_dual_filter
    writer.bit(true); // enable_order_hint
    writer.bit(false); // enable_jnt_comp
    writer.bit(false); // enable_ref_frame_mvs
    writer.bit(false); // seq_choose_screen_content_tools
    writer.bit(false); // seq_force_screen_content_tools
    writer.bits(u64::from(ORDER_HINT_BITS - 1), 3); // order_hint_bits_minus_1
    writer.bit(false); // enable_superres
    writer.bit(true); // enable_cdef
    writer.bit(false); // enable_restoration
    // color_config()
    writer.bit(false); // high_bitdepth
    writer.bit(false); // mono_chrome
    writer.bit(true); // color_description_present_flag
    writer.bits(COLOR_PRIMARIES_BT709, 8); // color_primaries
    writer.bits(TRANSFER_SRGB, 8); // transfer_characteristics
    writer.bits(MATRIX_BT709, 8); // matrix_coefficients
    writer.bit(false); // color_range: limited
    writer.bits(CHROMA_SAMPLE_POSITION_UNKNOWN, 2); // chroma_sample_position
    writer.bit(false); // separate_uv_delta_q
    writer.bit(false); // film_grain_params_present
    writer.trailing_bits();
    obu(OBU_SEQUENCE_HEADER, writer.into_bytes())
}

/// The type of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FrameKind {
    /// A key frame, which refers to no other frame.
    Key,
    /// An inter frame, which refers to the previous frame.
    Inter,
}

/// Whether the encoder may choose a transform size per block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TxMode {
    /// The largest transform that fits each block (`TX_MODE_LARGEST`).
    Largest,
    /// Chosen per block (`TX_MODE_SELECT`).
    Select,
}

/// What differs between the frame headers of a stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameHeader {
    /// The frame's type.
    pub(crate) kind: FrameKind,
    /// `order_hint`.
    pub(crate) order_hint: u8,
    /// `base_q_idx`, from 1 to 255; 0 would make the frame lossless.
    pub(crate) base_q_idx: u8,
    /// The deblocking filter level of every direction and plane, from 0 to 63.
    pub(crate) loop_filter_level: u8,
    /// How transform sizes are chosen.
    pub(crate) tx_mode: TxMode,
    /// The tiles.
    pub(crate) tiles: TileLayout,
    /// The size in bytes of each tile's size field (`TileSizeBytes`).
    pub(crate) tile_size_bytes: u8,
}

/// Returns the frame header OBU of one frame.
///
/// The render size is always signalled, equal to the stream's size, so that a
/// hardware encoder that pads frames to its alignment still tells receivers
/// the size to show.
pub(crate) fn frame_header_obu(format: &StreamFormat, frame: &FrameHeader) -> Vec<u8> {
    let intra = frame.kind == FrameKind::Key;
    let mut writer = BitWriter::new();
    writer.bit(false); // show_existing_frame
    writer.bits(
        match frame.kind {
            FrameKind::Key => KEY_FRAME_TYPE,
            FrameKind::Inter => INTER_FRAME_TYPE,
        },
        2,
    ); // frame_type
    writer.bit(true); // show_frame
    if !intra {
        // A shown key frame is error resilient without saying so.
        writer.bit(false); // error_resilient_mode
    }
    writer.bit(false); // disable_cdf_update
    writer.bit(false); // frame_size_override_flag
    writer.bits(u64::from(frame.order_hint), ORDER_HINT_BITS); // order_hint
    if !intra {
        writer.bits(u64::from(PRIMARY_REF_LAST), 3); // primary_ref_frame
        writer.bits(u64::from(ALL_REFERENCE_SLOTS), 8); // refresh_frame_flags
        writer.bit(false); // frame_refs_short_signaling
        for _ in 0..REFS_PER_FRAME {
            writer.bits(u64::from(PREVIOUS_FRAME_SLOT), 3); // ref_frame_idx[i]
        }
    }
    // render_size()
    writer.bit(true); // render_and_frame_size_different
    writer.bits(u64::from(format.width - 1), 16); // render_width_minus_1
    writer.bits(u64::from(format.height - 1), 16); // render_height_minus_1
    if !intra {
        writer.bit(false); // allow_high_precision_mv
        writer.bit(false); // is_filter_switchable
        writer.bits(u64::from(INTERPOLATION_EIGHTTAP), 2); // interpolation_filter
        writer.bit(false); // is_motion_mode_switchable
    }
    writer.bit(false); // disable_frame_end_update_cdf
    frame.tiles.write(&mut writer, frame.tile_size_bytes);
    // quantization_params()
    writer.bits(u64::from(frame.base_q_idx), 8); // base_q_idx
    writer.bit(false); // delta_coded, for DeltaQYDc
    writer.bit(false); // delta_coded, for DeltaQUDc
    writer.bit(false); // delta_coded, for DeltaQUAc
    writer.bit(false); // using_qmatrix
    writer.bit(false); // segmentation_enabled
    writer.bit(false); // delta_q_present
    // loop_filter_params()
    let level = u64::from(frame.loop_filter_level);
    writer.bits(level, 6); // loop_filter_level[0]
    writer.bits(level, 6); // loop_filter_level[1]
    if level != 0 {
        writer.bits(level, 6); // loop_filter_level[2]
        writer.bits(level, 6); // loop_filter_level[3]
    }
    writer.bits(0, 3); // loop_filter_sharpness
    writer.bit(false); // loop_filter_delta_enabled
    // cdef_params(), with strengths of zero for the encoder to replace
    writer.bits(u64::from(CDEF_DAMPING_MINUS_3), 2); // cdef_damping_minus_3
    writer.bits(0, 2); // cdef_bits
    writer.bits(0, 4); // cdef_y_pri_strength[0]
    writer.bits(0, 2); // cdef_y_sec_strength[0]
    writer.bits(0, 4); // cdef_uv_pri_strength[0]
    writer.bits(0, 2); // cdef_uv_sec_strength[0]
    writer.bit(frame.tx_mode == TxMode::Select); // tx_mode_select
    if !intra {
        writer.bit(false); // reference_select
    }
    writer.bit(false); // reduced_tx_set
    if !intra {
        for _ in 0..REFS_PER_FRAME {
            writer.bit(false); // is_global
        }
    }
    writer.trailing_bits();
    obu(OBU_FRAME_HEADER, writer.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level_for;

    /// Tile layouts meet the minimum counts and the width limit.
    #[test]
    fn tile_layouts() {
        let hd = TileLayout::new(1920, 1080, 2);
        assert_eq!((hd.cols, hd.rows, hd.col_width_sb), (2, 1, 15));
        let qhd = TileLayout::new(2560, 1440, 4);
        assert_eq!((qhd.cols, qhd.rows), (2, 2));
        let uhd = TileLayout::new(3840, 2160, 8);
        assert_eq!((uhd.cols, uhd.rows), (4, 2));
        // A frame wider than 4096 samples needs two columns even for one tile.
        let wide = TileLayout::new(5120, 1440, 1);
        assert_eq!((wide.cols, wide.rows), (2, 1));
        let tiny = TileLayout::new(64, 64, 8);
        assert_eq!(tiny.count(), 1);
    }

    /// The headers of a 1080p stream, byte for byte. These bytes were checked
    /// with an independent AV1 parser (FFmpeg's `trace_headers`), which read
    /// every syntax element back as intended.
    #[test]
    fn writes_verified_headers() {
        let tiles = TileLayout::new(1920, 1080, 2);
        let format = StreamFormat {
            width: 1920,
            height: 1080,
            level: level_for(1920, 1080, 60, tiles.count_and_columns()),
        };
        assert_eq!(
            sequence_header_obu(&format),
            [
                0x0a, 0x0e, 0x00, 0x00, 0x00, 0x4a, 0xab, 0xbf, 0xc3, 0x70, 0x08, 0x74, 0x40, 0x43,
                0x40, 0x41
            ]
        );
        let mut frame = FrameHeader {
            kind: FrameKind::Key,
            order_hint: 0,
            base_q_idx: 100,
            loop_filter_level: 12,
            tx_mode: TxMode::Select,
            tiles,
            tile_size_bytes: 4,
        };
        assert_eq!(
            frame_header_obu(&format, &frame),
            [
                0x1a, 0x0f, 0x10, 0x02, 0x0e, 0xfe, 0x08, 0x6e, 0xc6, 0xc8, 0x01, 0x86, 0x18, 0x60,
                0x40, 0x00, 0x50
            ]
        );
        frame.kind = FrameKind::Inter;
        frame.order_hint = 1;
        assert_eq!(
            frame_header_obu(&format, &frame),
            [
                0x1a, 0x15, 0x30, 0x02, 0x3f, 0xc0, 0x00, 0x00, 0x83, 0xbf, 0x82, 0x1b, 0x81, 0x8d,
                0x90, 0x03, 0x0c, 0x30, 0xc0, 0x80, 0x00, 0x80, 0x20
            ]
        );
    }
}
