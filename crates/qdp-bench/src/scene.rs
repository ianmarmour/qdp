//! The scenes the probe shows around its grid and marker: how much of the
//! host's screen moves, from almost nothing to all of it. What moves is part
//! of a texture that repeats seamlessly, moved on as time passes, in
//! colours that are never the probe's green.

use crate::layout::{Area, INSIDE_FRAME, Pixels, WINDOW};

/// How fast the full motion scene pans, in screen widths and heights a
/// second.
const PAN_SPEED: (f64, f64) = (0.25, 0.15);

/// How fast the scrolling scene scrolls, in screen heights a second.
const SCROLL_SPEED: f64 = 0.5;

/// The cell sizes of the landscape's noise, in pixels, largest first, each
/// with its weight.
const OCTAVES: [(usize, f64); 3] = [(256, 0.6), (64, 0.3), (16, 0.1)];

/// The landscape's colours, from its lowest ground to its highest, as red,
/// green and blue.
const LANDSCAPE: [[f64; 3]; 4] = [
    [24.0, 28.0, 64.0],
    [90.0, 50.0, 120.0],
    [200.0, 90.0, 60.0],
    [250.0, 200.0, 120.0],
];

/// The page's background.
const PAPER: u32 = 0xEC_EC_F0;

/// The page's text.
const INK: u32 = 0x3C_40_50;

/// The page's headings.
const HEADING: u32 = 0x1E_3C_8C;

/// The page's lines in a window's height.
const LINES_PER_WINDOW: usize = 24;

/// The page's windows of content before it repeats.
const PAGE_WINDOWS: usize = 4;

/// What the random numbers that lay out the page start from, the same each
/// run.
const PAGE_SEED: u64 = 0x5EED;

/// What the probe shows around its grid and marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scene {
    /// Only the grid and the marker change, as on an idle desktop with a
    /// clock.
    #[default]
    Low,
    /// A page scrolls in a window, as in a browser.
    Scroll,
    /// The whole screen pans across a landscape, as a game's camera does.
    Full,
}

impl Scene {
    /// Every scene, from least to most motion.
    pub const ALL: [Self; 3] = [Self::Low, Self::Scroll, Self::Full];

    /// Returns the scene named `name`.
    pub fn from_name(name: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|scene| scene.name() == name)
            .ok_or_else(|| format!("unknown scene {name}; the scenes are low, scroll and full"))
    }

    /// Returns the scene's name, as options and results give it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Scroll => "scroll",
            Self::Full => "full",
        }
    }

    /// Returns the scene's title, as the report gives it.
    pub fn title(self) -> &'static str {
        match self {
            Self::Low => "Low motion",
            Self::Scroll => "Scrolling",
            Self::Full => "Full motion",
        }
    }

    /// Returns the area of the probe the scene moves, if it moves any.
    pub fn moving_area(self) -> Option<Area> {
        match self {
            Self::Low => None,
            Self::Scroll => Some(WINDOW),
            Self::Full => Some(INSIDE_FRAME),
        }
    }

    /// Returns where in its texture the scene is at `time` microseconds, on
    /// a screen `screen` pixels in size.
    pub fn offset(self, time: u64, screen: (usize, usize)) -> (usize, usize) {
        let seconds = time as f64 / 1_000_000.0;
        let (width, height) = (screen.0 as f64, screen.1 as f64);
        match self {
            Self::Low => (0, 0),
            Self::Scroll => (0, (seconds * SCROLL_SPEED * height) as usize),
            Self::Full => (
                (seconds * PAN_SPEED.0 * width) as usize,
                (seconds * PAN_SPEED.1 * height) as usize,
            ),
        }
    }
}

/// A picture that repeats seamlessly both ways, part of which a scene
/// shows.
#[derive(Debug)]
pub struct Texture {
    /// The width, in pixels.
    width: usize,
    /// The height, in pixels.
    height: usize,
    /// The pixels, row by row, as `0x00RRGGBB` words.
    pixels: Vec<u32>,
}

impl Texture {
    /// Returns what `scene` moves across an area `width` by `height`
    /// pixels, if it moves anything.
    pub fn for_scene(scene: Scene, width: usize, height: usize) -> Option<Self> {
        match scene {
            Scene::Low => None,
            Scene::Scroll => Some(Self::page(width, height)),
            Scene::Full => Some(Self::landscape(width, height)),
        }
    }

    /// Draws the texture into `area` of `buffer`, whose rows are `stride`
    /// pixels apart, from `offset` in the texture.
    pub fn draw(&self, buffer: &mut [u32], stride: usize, area: Pixels, offset: (usize, usize)) {
        for row in 0..area.height {
            let source_row = (offset.1 + row) % self.height;
            let source = &self.pixels[source_row * self.width..(source_row + 1) * self.width];
            let start = (area.y + row) * stride + area.x;
            let target = &mut buffer[start..start + area.width];
            let mut column = offset.0 % self.width;
            let mut filled = 0;
            while filled < area.width {
                let run = (self.width - column).min(area.width - filled);
                target[filled..filled + run].copy_from_slice(&source[column..column + run]);
                filled += run;
                column = 0;
            }
        }
    }

    /// Returns a landscape of smooth noise, twice an area of `width` by
    /// `height` pixels each way, in whole cells of its largest noise, so
    /// that it repeats seamlessly and seldom.
    fn landscape(width: usize, height: usize) -> Self {
        let cell = OCTAVES[0].0;
        let width = (2 * width).max(1).next_multiple_of(cell);
        let height = (2 * height).max(1).next_multiple_of(cell);
        let mut pixels = Vec::with_capacity(width * height);
        for y in 0..height {
            for x in 0..width {
                let ground: f64 = OCTAVES
                    .iter()
                    .zip(0..)
                    .map(|(&(size, weight), seed)| {
                        weight * noise((x, y), size, (width / size, height / size), seed)
                    })
                    .sum();
                pixels.push(landscape_colour(ground));
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// Returns a page of paragraphs, headings and pictures, as wide as an
    /// area of `width` by `height` pixels and some windows of it long.
    fn page(width: usize, height: usize) -> Self {
        let line = (height / LINES_PER_WINDOW).max(4);
        let page_height = line * LINES_PER_WINDOW * PAGE_WINDOWS;
        let mut page = Self {
            width: width.max(1),
            height: page_height,
            pixels: vec![PAPER; width.max(1) * page_height],
        };
        let margin = width / 20;
        let text = (line * 11 / 20).max(1);
        let mut random = Random(PAGE_SEED);
        let mut y = line;
        let mut paragraph = 0;
        while y + line * 8 < page_height {
            if paragraph % 4 == 3 {
                // A picture, six lines high.
                page.picture(margin, y, width / 2, line * 6);
                y += line * 7;
            } else {
                if paragraph % 3 == 0 {
                    page.fill(margin, y, width * 2 / 5, line * 4 / 5, HEADING);
                    y += line * 2;
                }
                for _ in 0..3 + random.below(5) {
                    page.text_line(margin, y, width - 2 * margin, text, &mut random);
                    y += line;
                }
                y += line;
            }
            paragraph += 1;
        }
        page
    }

    /// Draws a line of words `height` pixels high from `x`, `y`, filling
    /// most of `width`.
    fn text_line(&mut self, x: usize, y: usize, width: usize, height: usize, random: &mut Random) {
        let space = height;
        let end = x + width * (7 + random.below(4) as usize) / 10;
        let mut left = x;
        while left < end {
            let word = (height * (2 + random.below(8) as usize) / 2).min(end - left);
            self.fill(left, y, word, height, INK);
            left += word + space;
        }
    }

    /// Draws a picture: a landscape's colours across `width` by `height`
    /// pixels from `x`, `y`.
    fn picture(&mut self, x: usize, y: usize, width: usize, height: usize) {
        for row in y..(y + height).min(self.height) {
            for column in x..(x + width).min(self.width) {
                let ground = ((column - x) as f64 / width.max(1) as f64
                    + (row - y) as f64 / height.max(1) as f64)
                    / 2.0;
                self.pixels[row * self.width + column] = landscape_colour(ground);
            }
        }
    }

    /// Fills `width` by `height` pixels from `x`, `y` with `colour`, as far
    /// as the texture goes.
    fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, colour: u32) {
        for row in y..(y + height).min(self.height) {
            let start = row * self.width + x.min(self.width);
            let end = row * self.width + (x + width).min(self.width);
            self.pixels[start..end].fill(colour);
        }
    }
}

/// Returns smooth noise between 0 and 1 at `x`, `y`, from random values on
/// a lattice `size` pixels apart that repeats every `period` points each
/// way, chosen by `seed`.
fn noise((x, y): (usize, usize), size: usize, period: (usize, usize), seed: u64) -> f64 {
    let (column, row) = (x / size, y / size);
    let smooth = |offset: usize| {
        let fraction = offset as f64 / size as f64;
        fraction * fraction * (3.0 - 2.0 * fraction)
    };
    let (across, down) = (smooth(x % size), smooth(y % size));
    let at = |column: usize, row: usize| {
        let point = ((row % period.1) * period.0 + column % period.0) as u64;
        (mix(point ^ seed << 56) >> 11) as f64 / (1_u64 << 53) as f64
    };
    let top = at(column, row) + (at(column + 1, row) - at(column, row)) * across;
    let bottom = at(column, row + 1) + (at(column + 1, row + 1) - at(column, row + 1)) * across;
    top + (bottom - top) * down
}

/// Returns the landscape's colour for `ground`, between 0 and 1.
fn landscape_colour(ground: f64) -> u32 {
    let position = ground.clamp(0.0, 1.0) * (LANDSCAPE.len() - 1) as f64;
    let low = (position as usize).min(LANDSCAPE.len() - 2);
    let fraction = position - low as f64;
    let [red, green, blue] = [0, 1, 2].map(|channel| {
        let (from, to) = (LANDSCAPE[low][channel], LANDSCAPE[low + 1][channel]);
        (from + (to - from) * fraction).round() as u32
    });
    red << 16 | green << 8 | blue
}

/// Returns `value` thoroughly mixed: SplitMix64's finaliser.
fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ value >> 30).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ value >> 27).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ value >> 31
}

/// Random numbers, the same from the same start.
#[derive(Debug)]
struct Random(u64);

impl Random {
    /// Returns a number below `bound`.
    fn below(&mut self, bound: u64) -> u64 {
        self.0 = self.0.wrapping_add(1);
        mix(self.0) % bound.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::GREEN_CHROMA;

    /// Returns whether a `0x00RRGGBB` colour is green as the measurer sees
    /// it.
    fn looks_green(colour: u32) -> bool {
        let rgb = [16, 8, 0].map(|shift| i32::from((colour >> shift) as u8));
        let (_, blue, red) = crate::picture::to_yuv(rgb);
        blue < GREEN_CHROMA && red < GREEN_CHROMA
    }

    /// Each scene's name reads back as the scene.
    #[test]
    fn names_scenes() {
        for scene in Scene::ALL {
            assert_eq!(Scene::from_name(scene.name()), Ok(scene));
        }
        assert!(Scene::from_name("busy").is_err());
    }

    /// Neither moving texture has the probe's green, which would move the
    /// probe where the measurer finds it.
    #[test]
    fn never_draws_green() {
        for scene in [Scene::Scroll, Scene::Full] {
            let texture = Texture::for_scene(scene, 64, 48).unwrap();
            assert!(!texture.pixels.iter().any(|&colour| looks_green(colour)));
        }
    }

    /// The landscape repeats seamlessly: its edges continue each other.
    #[test]
    fn repeats_seamlessly() {
        let size = 256;
        let period = (2, 2);
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(
            noise((0, 5), size, period, 1),
            noise((2 * size, 5), size, period, 1)
        ));
        assert!(close(
            noise((7, 0), size, period, 1),
            noise((7, 2 * size), size, period, 1)
        ));
    }

    /// Drawing wraps around the texture's edges and fills the whole area.
    #[test]
    fn draws_across_the_edges() {
        let texture = Texture {
            width: 3,
            height: 2,
            pixels: vec![1, 2, 3, 4, 5, 6],
        };
        let mut buffer = vec![0; 5 * 3];
        let area = Pixels {
            x: 1,
            y: 1,
            width: 4,
            height: 2,
        };
        texture.draw(&mut buffer, 5, area, (2, 1));
        assert_eq!(buffer[5..10], [0, 6, 4, 5, 6]);
        assert_eq!(buffer[10..15], [0, 3, 1, 2, 3]);
    }

    /// The scenes move at their speeds, and the low scene not at all.
    #[test]
    fn moves_with_time() {
        let screen = (1920, 1080);
        assert_eq!(Scene::Low.offset(5_000_000, screen), (0, 0));
        assert_eq!(Scene::Scroll.offset(2_000_000, screen), (0, 1080));
        assert_eq!(Scene::Full.offset(4_000_000, screen), (1920, 648));
    }
}
