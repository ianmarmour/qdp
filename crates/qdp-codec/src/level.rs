//! AV1 levels (AV1 specification, Annex A.3). A stream's sequence header
//! signals a level the stream meets, which must not exceed the client's Max
//! Level (Section 6.4.1, rule 8).

/// The `seq_level_idx` that signals no level constraints.
pub const NO_LEVEL: u8 = 31;

/// The limits of one AV1 level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LevelLimits {
    /// The level's `seq_level_idx`.
    seq_level_idx: u8,
    /// The most luma samples in a frame.
    max_picture_size: u64,
    /// The widest frame.
    max_width: u32,
    /// The tallest frame.
    max_height: u32,
    /// The most luma samples shown per second.
    max_display_rate: u64,
    /// The most tiles in a frame.
    max_tiles: u32,
    /// The most tile columns.
    max_tile_cols: u32,
}

/// The defined levels, lowest first.
const LEVELS: [LevelLimits; 14] = [
    level(0, 147_456, 2048, 1152, 4_423_680, 8, 4),
    level(1, 278_784, 2816, 1584, 8_363_520, 8, 4),
    level(4, 665_856, 4352, 2448, 19_975_680, 16, 6),
    level(5, 1_065_024, 5504, 3096, 31_950_720, 16, 6),
    level(8, 2_359_296, 6144, 3456, 70_778_880, 32, 8),
    level(9, 2_359_296, 6144, 3456, 141_557_760, 32, 8),
    level(12, 8_912_896, 8192, 4352, 267_386_880, 64, 8),
    level(13, 8_912_896, 8192, 4352, 534_773_760, 64, 8),
    level(14, 8_912_896, 8192, 4352, 1_069_547_520, 64, 8),
    level(15, 8_912_896, 8192, 4352, 1_069_547_520, 64, 8),
    level(16, 35_651_584, 16384, 8704, 1_069_547_520, 128, 16),
    level(17, 35_651_584, 16384, 8704, 2_139_095_040, 128, 16),
    level(18, 35_651_584, 16384, 8704, 4_278_190_080, 128, 16),
    level(19, 35_651_584, 16384, 8704, 4_278_190_080, 128, 16),
];

/// Builds one row of [`LEVELS`].
const fn level(
    seq_level_idx: u8,
    max_picture_size: u64,
    max_width: u32,
    max_height: u32,
    max_display_rate: u64,
    max_tiles: u32,
    max_tile_cols: u32,
) -> LevelLimits {
    LevelLimits {
        seq_level_idx,
        max_picture_size,
        max_width,
        max_height,
        max_display_rate,
        max_tiles,
        max_tile_cols,
    }
}

/// How many tiles a frame is divided into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TileCount {
    /// The number of tiles.
    pub tiles: u32,
    /// The number of tile columns.
    pub columns: u32,
}

impl TileCount {
    /// Returns a count of `tiles` tiles that may all lie in one row, for
    /// encoders that choose the layout themselves.
    pub fn at_most_columns(tiles: u32) -> Self {
        Self {
            tiles,
            columns: tiles,
        }
    }
}

/// Returns the lowest level a stream of `width × height` frames at
/// `frame_rate` frames per second in `tiles` meets, or [`NO_LEVEL`] if none
/// does.
pub fn level_for(width: u32, height: u32, frame_rate: u32, tiles: TileCount) -> u8 {
    let picture_size = u64::from(width) * u64::from(height);
    let display_rate = picture_size * u64::from(frame_rate);
    LEVELS
        .iter()
        .find(|limits| {
            picture_size <= limits.max_picture_size
                && width <= limits.max_width
                && height <= limits.max_height
                && display_rate <= limits.max_display_rate
                && tiles.tiles <= limits.max_tiles
                && tiles.columns <= limits.max_tile_cols
        })
        .map_or(NO_LEVEL, |limits| limits.seq_level_idx)
}

/// Returns the highest frame rate, up to `frame_rate`, at which a stream of
/// `width × height` frames in `tiles` meets a level no higher than
/// `max_level`, or `None` if the frames are too large for it at any rate.
pub fn frame_rate_within(
    width: u32,
    height: u32,
    frame_rate: u32,
    tiles: TileCount,
    max_level: u8,
) -> Option<u32> {
    if max_level == NO_LEVEL {
        return Some(frame_rate);
    }
    (1..=frame_rate)
        .rev()
        .find(|&rate| level_for(width, height, rate, tiles) <= max_level)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Levels follow the size and display rate limits of Annex A.3.
    #[test]
    fn levels() {
        let level =
            |width, height, rate| level_for(width, height, rate, TileCount::at_most_columns(2));
        assert_eq!(level(1280, 720, 30), 5);
        assert_eq!(level(1920, 1080, 30), 8);
        assert_eq!(level(1920, 1080, 60), 9);
        assert_eq!(level(2560, 1440, 60), 12);
        assert_eq!(level(3840, 2160, 60), 13);
        assert_eq!(level(3840, 2160, 120), 14);
        assert_eq!(level(16384, 16384, 60), NO_LEVEL);
    }

    /// The frame rate drops until the stream fits the level, if it can.
    #[test]
    fn fits_frame_rates_to_levels() {
        let tiles = TileCount::at_most_columns(2);
        assert_eq!(frame_rate_within(1920, 1080, 60, tiles, NO_LEVEL), Some(60));
        assert_eq!(frame_rate_within(1920, 1080, 60, tiles, 9), Some(60));
        assert_eq!(frame_rate_within(1920, 1080, 60, tiles, 8), Some(34));
        assert_eq!(frame_rate_within(1920, 1080, 60, tiles, 5), None);
    }
}
