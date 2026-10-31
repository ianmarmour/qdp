//! Composition blocks and block maps (Section 6.5.1).

use crate::wire::{Reader, VarInt, WireError, Writer};

/// The width and height of a composition block, in pixels.
pub const BLOCK_SIZE: u32 = 64;

/// The number of blocks one word of a [`BlockSet`] holds.
const BITS_PER_WORD: usize = u64::BITS as usize;

/// The number of blocks one byte of a `BITMAP` block map holds.
const BITS_PER_BYTE: usize = u8::BITS as usize;

/// The grid of composition blocks that divides a monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockGrid {
    /// The number of block columns, Bw.
    columns: u32,
    /// The number of block rows, Bh.
    rows: u32,
}

impl BlockGrid {
    /// Returns the grid of a monitor of `width` × `height` pixels.
    pub const fn for_monitor(width: u32, height: u32) -> Self {
        Self {
            columns: width.div_ceil(BLOCK_SIZE),
            rows: height.div_ceil(BLOCK_SIZE),
        }
    }

    /// Returns the number of block columns.
    pub const fn columns(self) -> u32 {
        self.columns
    }

    /// Returns the number of block rows.
    pub const fn rows(self) -> u32 {
        self.rows
    }

    /// Returns the number of blocks, N.
    pub const fn len(self) -> usize {
        self.columns as usize * self.rows as usize
    }

    /// Returns whether the grid has no blocks.
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// Returns the raster-order index of block (`column`, `row`).
    pub const fn index(self, column: u32, row: u32) -> usize {
        row as usize * self.columns as usize + column as usize
    }
}

/// How a block map is encoded on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum BlockMapEncoding {
    /// No blocks are set.
    None = 0x00,
    /// All blocks are set.
    All = 0x01,
    /// One bit per block.
    Bitmap = 0x02,
    /// Alternating runs of unset and set blocks.
    Runs = 0x03,
}

impl TryFrom<u8> for BlockMapEncoding {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::None),
            0x01 => Ok(Self::All),
            0x02 => Ok(Self::Bitmap),
            0x03 => Ok(Self::Runs),
            _ => Err(WireError::InvalidValue { field: "Encoding" }),
        }
    }
}

/// A set of composition blocks of one monitor, such as the changed blocks of a
/// motion frame or the covered blocks of a refinement frame.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockSet {
    /// The number of blocks of the monitor, N.
    len: usize,
    /// One bit per block, in raster order.
    words: Vec<u64>,
}

impl BlockSet {
    /// Returns a set of `len` blocks with none set.
    pub fn empty(len: usize) -> Self {
        Self {
            len,
            words: vec![0; len.div_ceil(BITS_PER_WORD)],
        }
    }

    /// Returns a set of `len` blocks with all set.
    pub fn full(len: usize) -> Self {
        let mut set = Self::empty(len);
        set.insert_range(0, len);
        set
    }

    /// Returns the number of blocks of the monitor, set or not.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns whether no block is set.
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|&word| word == 0)
    }

    /// Returns whether every block is set.
    pub fn is_full(&self) -> bool {
        self.count() == self.len
    }

    /// Returns the number of blocks set.
    pub fn count(&self) -> usize {
        self.words
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum()
    }

    /// Returns whether block `index` is set.
    pub fn contains(&self, index: usize) -> bool {
        index < self.len && self.words[index / BITS_PER_WORD] & (1 << (index % BITS_PER_WORD)) != 0
    }

    /// Sets block `index`.
    ///
    /// # Panics
    ///
    /// Panics if `index` is not below [`BlockSet::len`].
    pub fn insert(&mut self, index: usize) {
        assert!(index < self.len, "block {index} is outside the monitor");
        self.words[index / BITS_PER_WORD] |= 1 << (index % BITS_PER_WORD);
    }

    /// Sets blocks `start` up to but not including `end`.
    fn insert_range(&mut self, start: usize, end: usize) {
        for index in start..end {
            self.insert(index);
        }
    }

    /// Sets every block that is set in `other` as well.
    ///
    /// # Panics
    ///
    /// Panics if the two sets belong to grids of different sizes.
    pub fn union_with(&mut self, other: &Self) {
        assert_eq!(self.len, other.len, "block sets of different grids");
        for (word, other) in self.words.iter_mut().zip(&other.words) {
            *word |= other;
        }
    }

    /// Returns the indices of the blocks that are set, in raster order.
    pub fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.len).filter(|&index| self.contains(index))
    }

    /// Returns the lengths of the alternating unset and set runs, starting with an
    /// unset run and omitting a final unset run.
    fn runs(&self) -> Vec<usize> {
        let mut runs = Vec::new();
        let mut current = false;
        let mut length = 0;
        for index in 0..self.len {
            if self.contains(index) == current {
                length += 1;
            } else {
                runs.push(length);
                current = !current;
                length = 1;
            }
        }
        if current {
            runs.push(length);
        }
        runs
    }

    /// Appends the block map in its shortest encoding to `writer`.
    pub fn encode(&self, writer: &mut Writer) {
        if self.is_empty() {
            writer.write_u8(BlockMapEncoding::None as u8);
            return;
        }
        if self.is_full() {
            writer.write_u8(BlockMapEncoding::All as u8);
            return;
        }

        let runs = self.runs();
        let runs_len = encoded_count_len(runs.len())
            + runs
                .iter()
                .map(|&run| encoded_count_len(run))
                .sum::<usize>();
        let bitmap_len = self.len.div_ceil(BITS_PER_BYTE);
        if runs_len < bitmap_len {
            writer.write_u8(BlockMapEncoding::Runs as u8);
            writer.write_count(runs.len());
            for run in runs {
                writer.write_count(run);
            }
        } else {
            writer.write_u8(BlockMapEncoding::Bitmap as u8);
            let mut bitmap = vec![0; bitmap_len];
            for index in self.iter() {
                bitmap[index / BITS_PER_BYTE] |= 1 << (index % BITS_PER_BYTE);
            }
            writer.write_bytes(&bitmap);
        }
    }

    /// Reads a block map for a monitor of `len` blocks from `reader`.
    pub fn decode(reader: &mut Reader<'_>, len: usize) -> Result<Self, WireError> {
        match BlockMapEncoding::try_from(reader.read_u8()?)? {
            BlockMapEncoding::None => Ok(Self::empty(len)),
            BlockMapEncoding::All => Ok(Self::full(len)),
            BlockMapEncoding::Bitmap => Self::decode_bitmap(reader, len),
            BlockMapEncoding::Runs => Self::decode_runs(reader, len),
        }
    }

    /// Reads the bytes of a `BITMAP` block map.
    fn decode_bitmap(reader: &mut Reader<'_>, len: usize) -> Result<Self, WireError> {
        let bitmap = reader.read_bytes(len.div_ceil(BITS_PER_BYTE))?;
        let mut set = Self::empty(len);
        for (byte_index, &byte) in bitmap.iter().enumerate() {
            for bit in 0..BITS_PER_BYTE {
                if byte & (1 << bit) == 0 {
                    continue;
                }
                let index = byte_index * BITS_PER_BYTE + bit;
                if index >= len {
                    return Err(WireError::InvalidValue { field: "Bitmap" });
                }
                set.insert(index);
            }
        }
        Ok(set)
    }

    /// Reads the run lengths of a `RUNS` block map.
    fn decode_runs(reader: &mut Reader<'_>, len: usize) -> Result<Self, WireError> {
        let run_count = reader.read_count("Run Count", u64::MAX)?;
        let mut set = Self::empty(len);
        let mut start = 0usize;
        for run in 0..run_count {
            let length = usize::try_from(reader.read_varint()?.into_inner())
                .ok()
                .filter(|&length| length <= len - start)
                .ok_or(WireError::InvalidValue {
                    field: "Run Length",
                })?;
            let is_set_run = run % 2 == 1;
            if is_set_run {
                set.insert_range(start, start + length);
            }
            start += length;
        }
        Ok(set)
    }
}

/// Returns the number of bytes of the variable-length integer encoding of `count`.
fn encoded_count_len(count: usize) -> usize {
    VarInt::try_from(count).map_or(VarInt::MAX.encoded_len(), VarInt::encoded_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes `set` and returns the bytes.
    fn encode(set: &BlockSet) -> Vec<u8> {
        let mut writer = Writer::new();
        set.encode(&mut writer);
        writer.into_bytes()
    }

    /// Decodes a block map of `len` blocks from `bytes`.
    fn decode(bytes: &[u8], len: usize) -> Result<BlockSet, WireError> {
        BlockSet::decode(&mut Reader::new(bytes), len)
    }

    /// A 1920 × 1080 monitor has 30 × 17 blocks, the last row partial.
    #[test]
    fn grid_dimensions() {
        let grid = BlockGrid::for_monitor(1920, 1080);
        assert_eq!((grid.columns(), grid.rows()), (30, 17));
        assert_eq!(grid.len(), 510);
        assert_eq!(grid.index(2, 1), 32);
        assert_eq!(BlockGrid::for_monitor(16, 16).len(), 1);
    }

    /// Empty and full sets use the one-byte encodings.
    #[test]
    fn none_and_all() {
        assert_eq!(encode(&BlockSet::empty(510)), [0x00]);
        assert_eq!(encode(&BlockSet::full(510)), [0x01]);
        assert_eq!(decode(&[0x00], 510).unwrap(), BlockSet::empty(510));
        assert_eq!(decode(&[0x01], 510).unwrap(), BlockSet::full(510));
    }

    /// A few contiguous blocks use runs, which are shorter than a bitmap.
    #[test]
    fn runs_encoding() {
        let mut set = BlockSet::empty(510);
        for index in 100..110 {
            set.insert(index);
        }
        let bytes = encode(&set);
        assert_eq!(bytes, [0x03, 0x02, 0x40, 0x64, 0x0a]);
        assert_eq!(decode(&bytes, 510).unwrap(), set);
    }

    /// A scattered set uses a bitmap, with bit i mod 8 of byte i / 8.
    #[test]
    fn bitmap_encoding() {
        let mut set = BlockSet::empty(16);
        for index in [0, 2, 4, 6, 9, 11, 13, 15] {
            set.insert(index);
        }
        let bytes = encode(&set);
        assert_eq!(bytes, [0x02, 0b0101_0101, 0b1010_1010]);
        assert_eq!(decode(&bytes, 16).unwrap(), set);
    }

    /// A set that starts with a set block has an empty first run.
    #[test]
    fn runs_starting_with_set_block() {
        assert_eq!(
            decode(&[0x03, 0x02, 0x00, 0x03], 400)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    /// Every set round-trips whatever encoding is chosen.
    #[test]
    fn round_trip_patterns() {
        let len = 37;
        for pattern in 0..512u32 {
            let mut set = BlockSet::empty(len);
            for index in (0..len).filter(|index| pattern & (1 << (index % 9)) != 0) {
                set.insert(index);
            }
            assert_eq!(decode(&encode(&set), len).unwrap(), set, "{pattern}");
        }
    }

    /// Bits beyond the last block of a bitmap are an error.
    #[test]
    fn bitmap_unused_bits() {
        assert_eq!(
            decode(&[0x02, 0x00, 0x80], 10),
            Err(WireError::InvalidValue { field: "Bitmap" })
        );
    }

    /// Runs that sum to more than N are an error.
    #[test]
    fn runs_past_end() {
        assert_eq!(
            decode(&[0x03, 0x02, 0x05, 0x06], 10),
            Err(WireError::InvalidValue {
                field: "Run Length"
            })
        );
    }

    /// An unknown encoding is an error.
    #[test]
    fn unknown_encoding() {
        assert_eq!(
            decode(&[0x04], 10),
            Err(WireError::InvalidValue { field: "Encoding" })
        );
    }

    /// Union combines the changed blocks of several frames.
    #[test]
    fn union() {
        let mut first = BlockSet::empty(8);
        first.insert(1);
        let mut second = BlockSet::empty(8);
        second.insert(6);
        first.union_with(&second);
        assert_eq!(first.iter().collect::<Vec<_>>(), [1, 6]);
        assert_eq!(first.count(), 2);
    }
}
