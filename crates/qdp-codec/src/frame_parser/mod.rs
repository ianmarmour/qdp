//! AV1 frame headers and tile groups, read for hardware decoders that are
//! given them by the application rather than parsing the bitstream
//! themselves, as DXVA decoders on Windows are.
//!
//! [`FrameParser`] keeps the state the AV1 specification keeps for each of
//! the eight reference slots (Section 7.20), which later frames' headers
//! depend on, and returns for each frame of a temporal unit its header and
//! where each of its tiles lies.

mod grain;
mod header;
mod motion;
#[cfg(test)]
mod tests;

use std::ops::Range;

use crate::bits::BitReader;
use crate::{CodecError, ObuType, SequenceHeader, parse_obus};

pub(crate) use grain::FilmGrain;
pub(crate) use header::{FrameHeader, FrameSize, FrameType, RestorationType, SegmentFeatures};

/// `NUM_REF_FRAMES`: the reference slots, and the reference frames
/// `INTRA_FRAME` … `ALTREF_FRAME`.
pub(crate) const NUM_REF_FRAMES: usize = 8;

/// `REFS_PER_FRAME`: the references an inter frame has, `LAST_FRAME` …
/// `ALTREF_FRAME`.
pub(crate) const REFS_PER_FRAME: usize = 7;

/// What a reference slot holds of the frame last stored in it, for the
/// frames that refer to it (Section 7.20).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceSlot {
    /// `RefValid`: whether the slot holds a frame.
    pub(crate) valid: bool,
    /// `RefFrameId`.
    pub(crate) frame_id: u32,
    /// The frame's sizes.
    pub(crate) size: FrameSize,
    /// `RefFrameType`.
    pub(crate) frame_type: FrameType,
    /// `RefOrderHint`.
    pub(crate) order_hint: u32,
    /// `SavedGmParams`, by reference frame.
    pub(crate) global_motion: [[i32; 6]; NUM_REF_FRAMES],
    /// `SavedLoopFilterRefDeltas`.
    pub(crate) loop_filter_ref_deltas: [i8; NUM_REF_FRAMES],
    /// `SavedLoopFilterModeDeltas`.
    pub(crate) loop_filter_mode_deltas: [i8; 2],
    /// `SavedFeatureEnabled` and `SavedFeatureData`.
    pub(crate) segment_features: SegmentFeatures,
    /// The saved film grain parameters.
    pub(crate) film_grain: FilmGrain,
}

impl Default for ReferenceSlot {
    fn default() -> Self {
        Self {
            valid: false,
            frame_id: 0,
            size: FrameSize::default(),
            frame_type: FrameType::default(),
            order_hint: 0,
            global_motion: motion::DEFAULT_PARAMS,
            loop_filter_ref_deltas: header::DEFAULT_LOOP_FILTER_REF_DELTAS,
            loop_filter_mode_deltas: [0; 2],
            segment_features: SegmentFeatures::default(),
            film_grain: FilmGrain::default(),
        }
    }
}

/// One tile of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tile {
    /// Where the tile's data lies in the temporal unit.
    pub(crate) offset: usize,
    /// The bytes of the tile's data.
    pub(crate) size: usize,
    /// The tile's row.
    pub(crate) row: u16,
    /// The tile's column.
    pub(crate) column: u16,
}

/// A frame of a temporal unit: its header, and, unless it shows an existing
/// frame, its tiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedFrame {
    /// The frame's header.
    pub(crate) header: FrameHeader,
    /// The data of each tile group, in the temporal unit: what follows each
    /// tile group's header, the size fields between its tiles included.
    pub(crate) tile_data: Vec<Range<usize>>,
    /// The frame's tiles, in tile order.
    pub(crate) tiles: Vec<Tile>,
    /// The reference slots the frame refers to, as they were before it
    /// replaced any: the sizes of the frames it predicts from.
    pub(crate) references: [FrameSize; REFS_PER_FRAME],
}

/// Reads the frames of a stream's temporal units, keeping the state their
/// headers depend on from one to the next.
#[derive(Debug, Clone, Default)]
pub(crate) struct FrameParser {
    /// The stream's sequence header, once one arrived.
    sequence: Option<SequenceHeader>,
    /// The reference slots.
    refs: [ReferenceSlot; NUM_REF_FRAMES],
}

/// The frame whose tile groups are being read.
struct PendingFrame {
    /// The frame, its tiles so far.
    frame: ParsedFrame,
    /// The tile the next tile group starts at.
    next_tile: usize,
}

impl FrameParser {
    /// Creates a parser that has seen no sequence header.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Returns the reference slots, as the frames read so far left them.
    pub(crate) fn refs(&self) -> &[ReferenceSlot; NUM_REF_FRAMES] {
        &self.refs
    }

    /// Reads the frames of `temporal_unit`, in decoding order, storing each
    /// in the reference slots it refreshes once it is complete.
    ///
    /// On an error the slots may hold a partial update, so the stream must
    /// start again from a key frame.
    pub(crate) fn parse(&mut self, temporal_unit: &[u8]) -> Result<Vec<ParsedFrame>, CodecError> {
        let mut frames = Vec::new();
        let mut pending: Option<PendingFrame> = None;
        let mut obu_start = 0;
        for obu in parse_obus(temporal_unit)? {
            let payload_start = payload_offset(obu.bytes)?;
            let payload = &obu.bytes[payload_start..];
            let base = obu_start + payload_start;
            obu_start += obu.bytes.len();
            match obu.obu_type {
                ObuType::SequenceHeader => {
                    self.sequence = Some(SequenceHeader::parse(obu.bytes)?);
                }
                ObuType::FrameHeader | ObuType::Frame => {
                    if pending.is_some() {
                        if obu.obu_type == ObuType::Frame {
                            return Err(CodecError::MalformedBitstream(
                                "a frame starts before the last one's tiles",
                            ));
                        }
                        // A copy of the header of the frame being read.
                        continue;
                    }
                    let mut reader = BitReader::new(payload);
                    let frame = self.frame_header(&mut reader)?;
                    if frame.header.show_existing_frame {
                        self.finish(&frame.header);
                        frames.push(frame);
                        continue;
                    }
                    let mut next = PendingFrame {
                        frame,
                        next_tile: 0,
                    };
                    if obu.obu_type == ObuType::Frame {
                        reader.byte_alignment();
                        let start = reader.position() / 8;
                        Self::tile_group(&mut next, &payload[start..], base + start)?;
                    }
                    if next.next_tile == next.frame.header.tiles.count() {
                        self.finish(&next.frame.header);
                        frames.push(next.frame);
                    } else {
                        pending = Some(next);
                    }
                }
                ObuType::RedundantFrameHeader => {}
                ObuType::TileGroup => {
                    let Some(mut current) = pending.take() else {
                        return Err(CodecError::MalformedBitstream(
                            "a tile group without a frame header",
                        ));
                    };
                    Self::tile_group(&mut current, payload, base)?;
                    if current.next_tile == current.frame.header.tiles.count() {
                        self.finish(&current.frame.header);
                        frames.push(current.frame);
                    } else {
                        pending = Some(current);
                    }
                }
                _ => {}
            }
        }
        if pending.is_some() {
            return Err(CodecError::MalformedBitstream(
                "the temporal unit ends inside a frame",
            ));
        }
        Ok(frames)
    }

    /// Reads a frame header.
    fn frame_header(&mut self, reader: &mut BitReader<'_>) -> Result<ParsedFrame, CodecError> {
        let sequence = self
            .sequence
            .as_ref()
            .ok_or(CodecError::MalformedBitstream(
                "a frame before the sequence header",
            ))?;
        let header = header::parse(reader, sequence, &mut self.refs)?;
        let references = header
            .ref_frame_idx
            .map(|slot| self.refs[usize::from(slot)].size);
        Ok(ParsedFrame {
            header,
            tile_data: Vec::new(),
            tiles: Vec::new(),
            references,
        })
    }

    /// Reads `tile_group_obu()` from `payload`, which starts at `base` in
    /// the temporal unit.
    fn tile_group(
        current: &mut PendingFrame,
        payload: &[u8],
        base: usize,
    ) -> Result<(), CodecError> {
        let tiles = &current.frame.header.tiles;
        let count = tiles.count();
        let mut reader = BitReader::new(payload);
        let (start, end) = if count > 1 && reader.bit()? {
            let bits = tiles.cols_log2 + tiles.rows_log2;
            (reader.bits(bits)? as usize, reader.bits(bits)? as usize)
        } else {
            (0, count - 1)
        };
        if start != current.next_tile || end < start || end >= count {
            return Err(CodecError::MalformedBitstream("tile groups out of order"));
        }
        reader.byte_alignment();
        let header_bytes = reader.position() / 8;
        let data = payload
            .get(header_bytes..)
            .ok_or(CodecError::MalformedBitstream("truncated tile group"))?;
        let size_bytes = tiles.tile_size_bytes as usize;
        let mut offset = 0;
        for tile in start..=end {
            let size = if tile == end {
                data.len() - offset
            } else {
                let field = data
                    .get(offset..offset + size_bytes)
                    .ok_or(CodecError::MalformedBitstream("truncated tile size"))?;
                let size = field
                    .iter()
                    .rev()
                    .fold(0usize, |size, &byte| (size << 8) | usize::from(byte))
                    + 1;
                offset += size_bytes;
                if size > data.len() - offset {
                    return Err(CodecError::MalformedBitstream("tile larger than its group"));
                }
                size
            };
            current.frame.tiles.push(Tile {
                offset: base + header_bytes + offset,
                size,
                row: (tile / tiles.cols()) as u16,
                column: (tile % tiles.cols()) as u16,
            });
            offset += size;
        }
        current
            .frame
            .tile_data
            .push(base + header_bytes..base + payload.len());
        current.next_tile = end + 1;
        Ok(())
    }

    /// The reference frame update process (Section 7.20): stores a complete
    /// frame in the slots it refreshes.
    fn finish(&mut self, header: &FrameHeader) {
        if header.show_existing_frame {
            // Showing a key frame again loads its state and refreshes every
            // slot with it (Section 7.21); other frames refresh none.
            if header.refresh_frame_flags != 0 {
                let shown = self.refs[usize::from(header.frame_to_show_map_idx)].clone();
                self.refs = std::array::from_fn(|_| shown.clone());
            }
            return;
        }
        let saved = ReferenceSlot {
            valid: true,
            frame_id: header.current_frame_id,
            size: header.size,
            frame_type: header.frame_type,
            order_hint: header.order_hint,
            global_motion: header.global_motion.map(|motion| motion.params),
            loop_filter_ref_deltas: header.loop_filter.ref_deltas,
            loop_filter_mode_deltas: header.loop_filter.mode_deltas,
            segment_features: header.segmentation.features,
            film_grain: header.film_grain.clone(),
        };
        for (index, slot) in self.refs.iter_mut().enumerate() {
            if header.refresh_frame_flags & (1 << index) != 0 {
                *slot = saved.clone();
            }
        }
    }
}

/// Returns where an OBU's payload starts: after its header byte and size
/// field.
fn payload_offset(obu: &[u8]) -> Result<usize, CodecError> {
    obu[1..]
        .iter()
        .position(|byte| byte & 0x80 == 0)
        .map(|index| index + 2)
        .ok_or(CodecError::MalformedBitstream("truncated OBU size"))
}
