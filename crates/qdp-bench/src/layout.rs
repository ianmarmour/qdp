//! What the probe shows, where, and how the measurer reads it back: the time
//! the probe drew each frame, as a grid of black and white cells, and a
//! marker that is white while the pointer's button is held, on a green
//! background no desktop has.
//!
//! Positions are fractions of the probe's picture, which the measurer finds
//! by its green on the client's screen, wherever a viewer shows it and at
//! whatever size.

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

/// The chroma below which, in both planes, a pixel is the probe's green:
/// greys, black and white sit at 128.
const GREEN_CHROMA: u8 = 90;

/// The share of the screen's pixels, in thousandths, that must be green for
/// the probe to be taken as shown.
const MIN_GREEN_PER_MILLE: usize = 20;

/// The grid's area of the probe.
pub const GRID: Area = Area {
    left: 0.025,
    top: 0.05,
    width: 0.4,
    height: 0.177_777_8,
};

/// The marker's area of the probe.
pub const MARKER: Area = Area {
    left: 0.5,
    top: 0.05,
    width: 0.45,
    height: 0.4,
};

/// Where the measurer clicks: low in the middle of the probe, away from the
/// grid and the marker, which the pointer would cover.
pub const CLICK: (f64, f64) = (0.5, 0.75);

/// A rectangle of the probe, in fractions of its width and height.
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

impl Pixels {
    /// Returns the point `(x, y)`, in fractions of this rectangle, in pixels.
    pub fn point(self, (x, y): (f64, f64)) -> (f64, f64) {
        (
            self.x as f64 + x * self.width as f64,
            self.y as f64 + y * self.height as f64,
        )
    }
}

impl Area {
    /// Returns the area within `probe`, the probe's picture.
    pub fn within(self, probe: Pixels) -> Pixels {
        let (left, top) = probe.point((self.left, self.top));
        let (right, bottom) = probe.point((self.left + self.width, self.top + self.height));
        let (left, top) = (left.round() as usize, top.round() as usize);
        let right = (right.round() as usize).min(probe.x + probe.width);
        let bottom = (bottom.round() as usize).min(probe.y + probe.height);
        Pixels {
            x: left,
            y: top,
            width: right.saturating_sub(left),
            height: bottom.saturating_sub(top),
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

/// A picture of the client's screen, as 4:2:0 planes.
#[derive(Debug, Clone, Copy)]
pub struct Picture<'a> {
    /// The luma plane, `width` bytes a row.
    pub luma: &'a [u8],
    /// The blue-difference plane, `(width + 1) / 2` bytes a row.
    pub blue: &'a [u8],
    /// The red-difference plane, as wide.
    pub red: &'a [u8],
    /// The width, in pixels.
    pub width: usize,
    /// The height, in pixels.
    pub height: usize,
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

/// Returns where `picture` shows the probe: the box around its green, if
/// enough of the picture is green.
pub fn locate(picture: Picture<'_>) -> Option<Pixels> {
    let chroma_width = picture.width.div_ceil(2);
    let chroma_height = picture.height.div_ceil(2);
    let (mut left, mut top, mut right, mut bottom) = (usize::MAX, usize::MAX, 0, 0);
    let mut green = 0;
    for y in 0..chroma_height {
        let row = y * chroma_width;
        let blue = picture.blue.get(row..row + chroma_width)?;
        let red = picture.red.get(row..row + chroma_width)?;
        for (x, (&blue, &red)) in blue.iter().zip(red).enumerate() {
            if blue < GREEN_CHROMA && red < GREEN_CHROMA {
                green += 1;
                left = left.min(x);
                right = right.max(x);
                top = top.min(y);
                bottom = bottom.max(y);
            }
        }
    }
    if green * 1000 < chroma_width * chroma_height * MIN_GREEN_PER_MILLE {
        return None;
    }
    let x = left * 2;
    let y = top * 2;
    Some(Pixels {
        x,
        y,
        width: ((right + 1) * 2).min(picture.width) - x,
        height: ((bottom + 1) * 2).min(picture.height) - y,
    })
}

/// Returns the time a frame was drawn, in microseconds, read from the probe
/// that `picture` shows at `probe`, or `None` if the grid is not there
/// whole.
pub fn read_time(picture: Picture<'_>, probe: Pixels) -> Option<u64> {
    let mut word = 0_u64;
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let cell = GRID.cell(column, row).middle().within(probe);
            if average_luma(picture, cell)? >= LUMA_THRESHOLD {
                word |= 1 << (row * COLUMNS + column);
            }
        }
    }
    decode(word)
}

/// Returns whether the marker of the probe that `picture` shows at `probe`
/// is white.
pub fn marker_lit(picture: Picture<'_>, probe: Pixels) -> Option<bool> {
    Some(average_luma(picture, MARKER.middle().within(probe))? >= LUMA_THRESHOLD)
}

/// Returns the average luma of `area`, or `None` if it is empty or lies
/// outside the picture.
fn average_luma(picture: Picture<'_>, area: Pixels) -> Option<u32> {
    let count = area.width * area.height;
    if count == 0 {
        return None;
    }
    let mut sum = 0_u64;
    for y in area.y..area.y + area.height {
        let start = y * picture.width + area.x;
        let row = picture.luma.get(start..start + area.width)?;
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

    /// The width of the screens the tests draw.
    const WIDTH: usize = 640;

    /// The height of the screens the tests draw.
    const HEIGHT: usize = 360;

    /// Where the tests' screens show the probe: smaller than the screen, and
    /// away from its corner, as a viewer in a window would.
    const PROBE: Pixels = Pixels {
        x: 40,
        y: 30,
        width: 480,
        height: 270,
    };

    /// A screen's planes.
    struct Screen {
        /// The luma plane.
        luma: Vec<u8>,
        /// The blue-difference plane.
        blue: Vec<u8>,
        /// The red-difference plane.
        red: Vec<u8>,
    }

    impl Screen {
        /// Returns the screen as a picture.
        fn picture(&self) -> Picture<'_> {
            Picture {
                luma: &self.luma,
                blue: &self.blue,
                red: &self.red,
                width: WIDTH,
                height: HEIGHT,
            }
        }
    }

    /// Returns a grey screen showing, at `PROBE`, the probe for `time`, with
    /// the marker lit if `lit`.
    fn screen(time: u64, lit: bool) -> Screen {
        let chroma = WIDTH / 2 * (HEIGHT / 2);
        let mut screen = Screen {
            luma: vec![80; WIDTH * HEIGHT],
            blue: vec![128; chroma],
            red: vec![128; chroma],
        };
        for y in PROBE.y / 2..(PROBE.y + PROBE.height) / 2 {
            for x in PROBE.x / 2..(PROBE.x + PROBE.width) / 2 {
                screen.blue[y * WIDTH / 2 + x] = 42;
                screen.red[y * WIDTH / 2 + x] = 26;
            }
        }
        let mut fill = |area: Area, luma: u8| {
            let area = area.within(PROBE);
            for y in area.y..area.y + area.height {
                screen.luma[y * WIDTH + area.x..y * WIDTH + area.x + area.width].fill(luma);
            }
        };
        fill(
            Area {
                left: 0.0,
                top: 0.0,
                width: 1.0,
                height: 1.0,
            },
            173,
        );
        for (cell, white) in cells(time) {
            fill(cell, if white { 235 } else { 16 });
        }
        fill(MARKER, if lit { 235 } else { 16 });
        screen
    }

    /// The probe is found where the screen shows it.
    #[test]
    fn locates_the_probe() {
        let screen = screen(7, false);
        assert_eq!(locate(screen.picture()), Some(PROBE));
    }

    /// A screen without the probe's green has no probe.
    #[test]
    fn finds_no_probe_without_green() {
        let chroma = WIDTH / 2 * (HEIGHT / 2);
        let blank = Screen {
            luma: vec![80; WIDTH * HEIGHT],
            blue: vec![128; chroma],
            red: vec![128; chroma],
        };
        assert_eq!(locate(blank.picture()), None);
    }

    /// A drawn time reads back from where the probe is.
    #[test]
    fn reads_the_time_back() {
        for time in [1, 0x1234_5678_9abc, (1 << TIME_BITS) - 1] {
            let screen = screen(time, false);
            let probe = locate(screen.picture()).unwrap();
            assert_eq!(read_time(screen.picture(), probe), Some(time));
        }
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
        for lit in [true, false] {
            let screen = screen(7, lit);
            assert_eq!(marker_lit(screen.picture(), PROBE), Some(lit));
        }
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
