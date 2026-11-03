//! What the probe shows, where, and how the measurer reads it back: the time
//! the probe drew each frame, as a grid of black and white cells, and a
//! marker that is white while the pointer's button is held.
//!
//! Positions are fractions of the screen, so the measurer reads a viewer
//! that shows the host's screen scaled, as long as it fills its own.

/// The bits of the time, in microseconds, that the grid carries.
const TIME_BITS: u32 = 48;

/// The bits of the check that tells a whole, current grid from a torn or
/// garbled one.
const CHECK_BITS: u32 = 16;

/// The grid's columns.
const COLUMNS: u32 = 16;

/// The grid's rows.
const ROWS: u32 = (TIME_BITS + CHECK_BITS) / COLUMNS;

/// What the check is mixed with, so that an all-black grid does not pass.
const CHECK_SALT: u64 = 0xA5A5;

/// The luma that separates black cells from white ones.
const LUMA_THRESHOLD: u32 = 128;

/// The grid's area of the screen.
pub const GRID: Area = Area {
    left: 0.025,
    top: 0.05,
    width: 0.4,
    height: 0.177_777_8,
};

/// The marker's area of the screen.
pub const MARKER: Area = Area {
    left: 0.5,
    top: 0.05,
    width: 0.45,
    height: 0.4,
};

/// Where the measurer clicks: the middle of the screen, away from the grid
/// and the marker, which the pointer would cover.
pub const CLICK: (f64, f64) = (0.5, 0.75);

/// A rectangle of the screen, in fractions of its width and height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    /// The left edge.
    pub left: f64,
    /// The top edge.
    pub top: f64,
    /// The width.
    pub width: f64,
    /// The height.
    pub height: f64,
}

/// A rectangle of a picture, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pixels {
    /// The left edge.
    pub x: usize,
    /// The top edge.
    pub y: usize,
    /// The width.
    pub width: usize,
    /// The height.
    pub height: usize,
}

impl Area {
    /// Returns the area in a `width × height` picture.
    pub fn in_picture(self, width: usize, height: usize) -> Pixels {
        let x = (self.left * width as f64).round() as usize;
        let y = (self.top * height as f64).round() as usize;
        let right = ((self.left + self.width) * width as f64).round() as usize;
        let bottom = ((self.top + self.height) * height as f64).round() as usize;
        Pixels {
            x,
            y,
            width: right.min(width) - x,
            height: bottom.min(height) - y,
        }
    }

    /// Returns the grid cell at `column` and `row` of this area.
    fn cell(self, column: u32, row: u32) -> Area {
        let width = self.width / f64::from(COLUMNS);
        let height = self.height / f64::from(ROWS);
        Area {
            left: self.left + width * f64::from(column),
            top: self.top + height * f64::from(row),
            width,
            height,
        }
    }

    /// Returns the middle half of the area, both ways, which a reader
    /// samples to stay clear of blurred edges.
    fn middle(self) -> Area {
        Area {
            left: self.left + self.width / 4.0,
            top: self.top + self.height / 4.0,
            width: self.width / 2.0,
            height: self.height / 2.0,
        }
    }
}

/// Returns the grid's cells, left to right then top to bottom, each with
/// whether it is white, for a frame drawn at `time` microseconds.
pub fn cells(time: u64) -> impl Iterator<Item = (Area, bool)> {
    let word = encode(time);
    (0..ROWS).flat_map(move |row| {
        (0..COLUMNS).map(move |column| {
            let bit = row * COLUMNS + column;
            (GRID.cell(column, row), word >> bit & 1 == 1)
        })
    })
}

/// Returns the time a frame was drawn, in microseconds, read from its
/// `width × height` luma plane `luma`, with `stride` bytes a row, or `None`
/// if the grid is not there whole.
pub fn read_time(luma: &[u8], stride: usize, width: usize, height: usize) -> Option<u64> {
    let mut word = 0_u64;
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let cell = GRID.cell(column, row).middle().in_picture(width, height);
            if average_luma(luma, stride, cell)? >= LUMA_THRESHOLD {
                word |= 1 << (row * COLUMNS + column);
            }
        }
    }
    decode(word)
}

/// Returns whether the marker is white in a `width × height` luma plane.
pub fn marker_lit(luma: &[u8], stride: usize, width: usize, height: usize) -> Option<bool> {
    let area = MARKER.middle().in_picture(width, height);
    Some(average_luma(luma, stride, area)? >= LUMA_THRESHOLD)
}

/// Returns the average luma of `area`, or `None` if it is empty or lies
/// outside `luma`.
fn average_luma(luma: &[u8], stride: usize, area: Pixels) -> Option<u32> {
    let count = area.width * area.height;
    if count == 0 {
        return None;
    }
    let mut sum = 0_u64;
    for y in area.y..area.y + area.height {
        let start = y * stride + area.x;
        let row = luma.get(start..start + area.width)?;
        sum += row.iter().map(|&value| u64::from(value)).sum::<u64>();
    }
    u32::try_from(sum / count as u64).ok()
}

/// Returns the grid's bits for `time`: the time's low bits, then its check.
fn encode(time: u64) -> u64 {
    let time = time & ((1 << TIME_BITS) - 1);
    time | check(time) << TIME_BITS
}

/// Returns the time the grid's bits carry, if their check matches.
fn decode(word: u64) -> Option<u64> {
    let time = word & ((1 << TIME_BITS) - 1);
    (word >> TIME_BITS == check(time)).then_some(time)
}

/// Returns the check of `time`'s bits: its 16-bit words folded together.
fn check(time: u64) -> u64 {
    (time ^ time >> 16 ^ time >> 32 ^ CHECK_SALT) & ((1 << CHECK_BITS) - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The width of the pictures the tests draw.
    const WIDTH: usize = 640;

    /// The height of the pictures the tests draw.
    const HEIGHT: usize = 360;

    /// Returns a luma plane showing the grid for `time`, and the marker if
    /// `lit`.
    fn picture(time: u64, lit: bool) -> Vec<u8> {
        let mut luma = vec![16; WIDTH * HEIGHT];
        let mut fill = |area: Area| {
            let area = area.in_picture(WIDTH, HEIGHT);
            for y in area.y..area.y + area.height {
                luma[y * WIDTH + area.x..y * WIDTH + area.x + area.width].fill(235);
            }
        };
        for (cell, white) in cells(time) {
            if white {
                fill(cell);
            }
        }
        if lit {
            fill(MARKER);
        }
        luma
    }

    /// A drawn time reads back.
    #[test]
    fn reads_the_time_back() {
        for time in [1, 0x1234_5678_9abc, (1 << TIME_BITS) - 1] {
            let luma = picture(time, false);
            assert_eq!(read_time(&luma, WIDTH, WIDTH, HEIGHT), Some(time));
        }
    }

    /// A picture without the grid has no time.
    #[test]
    fn finds_no_time_without_the_grid() {
        let luma = vec![16; WIDTH * HEIGHT];
        assert_eq!(read_time(&luma, WIDTH, WIDTH, HEIGHT), None);
    }

    /// A grid with one wrong cell fails its check.
    #[test]
    fn refuses_a_garbled_grid() {
        let word = encode(0x1234_5678_9abc);
        assert_eq!(decode(word ^ 1 << 5), None);
        assert_eq!(decode(word ^ 1 << 50), None);
    }

    /// The marker reads as lit only while it is drawn.
    #[test]
    fn reads_the_marker() {
        assert_eq!(
            marker_lit(&picture(7, true), WIDTH, WIDTH, HEIGHT),
            Some(true)
        );
        assert_eq!(
            marker_lit(&picture(7, false), WIDTH, WIDTH, HEIGHT),
            Some(false)
        );
    }

    /// The click lands on neither the grid nor the marker.
    #[test]
    fn clicks_away_from_what_is_read() {
        for area in [GRID, MARKER] {
            let (x, y) = CLICK;
            let inside = (area.left..area.left + area.width).contains(&x)
                && (area.top..area.top + area.height).contains(&y);
            assert!(!inside);
        }
    }
}
