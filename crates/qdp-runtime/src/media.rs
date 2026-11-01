//! Frame sources on the host, frame sinks on the client, and the comparison of
//! frames that fills block maps.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use qdp::message::{
    BLOCK_SIZE, BlockGrid, BlockSet, CursorImage, FrameNumber, PointerMode, StreamId,
};
#[cfg(target_os = "linux")]
use qdp_codec::DmaBufFrame;
use qdp_codec::{DecodedPicture, Yuv420Frame};

use tokio::sync::Notify;

use crate::RuntimeError;

/// A pointer image, as a capture source reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CursorShape {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The pixel that marks the pointer's position: its column and row.
    pub hotspot: (u32, u32),
    /// Red, green, blue and alpha bytes per pixel, sRGB with straight alpha,
    /// row by row from the top.
    pub pixels: Vec<u8>,
}

/// The bytes of each pixel of a [`CursorShape`]: red, green, blue, alpha.
const RGBA_BYTES: usize = 4;

/// The index of the alpha byte within a pixel of a [`CursorShape`].
const ALPHA_INDEX: usize = 3;

/// The rows of the stock arrow: `B` is its black outline, `W` its white
/// fill, and `.` transparent.
const ARROW: [&str; 19] = [
    "B...........",
    "BB..........",
    "BWB.........",
    "BWWB........",
    "BWWWB.......",
    "BWWWWB......",
    "BWWWWWB.....",
    "BWWWWWWB....",
    "BWWWWWWWB...",
    "BWWWWWWWWB..",
    "BWWWWWWWWWB.",
    "BWWWWWWBBBBB",
    "BWWWBWWB....",
    "BWWBBWWB....",
    "BWB..BWWB...",
    "BB...BWWB...",
    "B.....BWWB..",
    "......BWWB..",
    ".......BB...",
];

impl CursorShape {
    /// Returns whether no pixel of the image shows: how applications,
    /// games especially, often hide the pointer while drawing their own.
    pub fn is_invisible(&self) -> bool {
        self.pixels
            .chunks_exact(RGBA_BYTES)
            .all(|pixel| pixel[ALPHA_INDEX] == 0)
    }

    /// Returns a stock arrow pointing up and left from its hotspot, for
    /// sources that cannot read the pointer's image: the host never draws
    /// the pointer into its video (Section 9.1).
    pub fn arrow() -> Self {
        let pixels = ARROW
            .iter()
            .flat_map(|row| row.bytes())
            .flat_map(|pixel| match pixel {
                b'B' => [0, 0, 0, 255],
                b'W' => [255, 255, 255, 255],
                _ => [0, 0, 0, 0],
            })
            .collect();
        Self {
            width: ARROW[0].len() as u32,
            height: ARROW.len() as u32,
            hotspot: (0, 0),
            pixels,
        }
    }
}

/// Where a capture source saw the pointer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PointerPosition {
    /// Hidden, or on another monitor.
    #[default]
    Hidden,
    /// Its hotspot is at this position in the frame, in pixels.
    At(i32, i32),
    /// Shown, but the source cannot tell where: the client draws it at its
    /// own pointer, and the host cannot tell it about moves.
    Unknown,
}

/// The pointer, as a capture source last saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorState {
    /// Where the pointer is.
    pub position: PointerPosition,
    /// The pointer's image, shared until it changes.
    pub shape: Option<Arc<CursorShape>>,
}

/// The interface scale of a captured monitor: how many of its pixels make one
/// unit of the desktop's own coordinates, in which the desktop lays out its
/// windows and measures the pointer's motion, in thousandths: 1500 at 150 %.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DesktopScale(u32);

impl DesktopScale {
    /// The thousandths in a scale of one.
    pub const UNIT: u32 = 1000;

    /// No scaling: a pixel to each unit.
    pub const UNSCALED: Self = Self(Self::UNIT);

    /// The smallest scale taken as real, in thousandths.
    const MIN: u32 = 500;

    /// The largest scale taken as real, in thousandths.
    const MAX: u32 = 8000;

    /// Returns the scale of a monitor `pixels` wide whose desktop coordinates
    /// make it `units` wide, or `None` if either is zero or the scale is
    /// outside what desktops use.
    pub fn from_widths(pixels: usize, units: usize) -> Option<Self> {
        let (pixels, units) = (u64::try_from(pixels).ok()?, u64::try_from(units).ok()?);
        if pixels == 0 || units == 0 {
            return None;
        }
        let thousandths = (pixels * u64::from(Self::UNIT) + units / 2) / units;
        u32::try_from(thousandths)
            .ok()
            .filter(|thousandths| (Self::MIN..=Self::MAX).contains(thousandths))
            .map(Self)
    }

    /// Returns the scale in thousandths.
    pub const fn thousandths(self) -> u32 {
        self.0
    }

    /// Returns the scale in percent, rounded, as `DISPLAY_CONFIG` reports
    /// it (Section 6.3).
    pub const fn percent(self) -> u32 {
        (self.0 + 5) / 10
    }
}

impl Default for DesktopScale {
    fn default() -> Self {
        Self::UNSCALED
    }
}

/// A rectangle of the host's desktop, in the desktop's own units, which the
/// interface scale makes larger than the monitors' pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutRect {
    /// The left edge.
    pub x: i32,
    /// The top edge.
    pub y: i32,
    /// The width.
    pub width: u32,
    /// The height.
    pub height: u32,
}

impl LayoutRect {
    /// Returns the smallest rectangle holding all of `rects`, or `None` if
    /// there are none.
    pub fn union(rects: impl IntoIterator<Item = Self>) -> Option<Self> {
        let edges = |rect: Self| {
            (
                i64::from(rect.x),
                i64::from(rect.y),
                i64::from(rect.x) + i64::from(rect.width),
                i64::from(rect.y) + i64::from(rect.height),
            )
        };
        let (left, top, right, bottom) = rects
            .into_iter()
            .map(edges)
            .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))?;
        Some(Self {
            x: i32::try_from(left).ok()?,
            y: i32::try_from(top).ok()?,
            width: u32::try_from(right - left).ok()?,
            height: u32::try_from(bottom - top).ok()?,
        })
    }
}

/// Where the captured monitor is in the host's desktop. Absolute pointing
/// devices cover the whole desktop, every monitor together, while the
/// client's positions are on the captured monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MonitorPlacement {
    /// The captured monitor.
    pub monitor: LayoutRect,
    /// The whole desktop.
    pub desktop: LayoutRect,
}

impl MonitorPlacement {
    /// Returns where a position on a `capture`-sized monitor, in its pixels,
    /// falls in the whole desktop, as a share of the desktop's width and
    /// height, from 0 to 1.
    pub fn desktop_share(&self, (x, y): (f64, f64), capture: (usize, usize)) -> (f64, f64) {
        let share =
            |position: f64, pixels: usize, start: i32, length: u32, from: i32, across: u32| {
                let within = position / pixels.max(1) as f64 * f64::from(length);
                ((f64::from(start) - f64::from(from) + within) / f64::from(across.max(1)))
                    .clamp(0.0, 1.0)
            };
        (
            share(
                x,
                capture.0,
                self.monitor.x,
                self.monitor.width,
                self.desktop.x,
                self.desktop.width,
            ),
            share(
                y,
                capture.1,
                self.monitor.y,
                self.monitor.height,
                self.desktop.y,
                self.desktop.height,
            ),
        )
    }
}

/// The display a client asks the host's monitor to match (Section 6.2): its
/// size, refresh rate and interface scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WantedDisplay {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The refresh rate in millihertz, or 0 if the client did not say.
    pub refresh_millihertz: u32,
    /// The interface scale in percent, or 0 if the client did not say.
    pub scale_percent: u32,
}

/// A frame a source captured: in memory, or held by the GPU.
#[derive(Debug, Clone)]
pub enum CapturedFrame {
    /// A frame in memory.
    Memory(Arc<Yuv420Frame>),
    /// A frame the GPU holds, which hardware encoders read where it is.
    #[cfg(target_os = "linux")]
    DmaBuf(Arc<DmaBufFrame>),
}

impl CapturedFrame {
    /// Returns the width and height in pixels.
    pub fn size(&self) -> (usize, usize) {
        match self {
            Self::Memory(frame) => (frame.width(), frame.height()),
            #[cfg(target_os = "linux")]
            Self::DmaBuf(frame) => (frame.width(), frame.height()),
        }
    }

    /// Returns the frame in memory, if it is in memory.
    pub fn in_memory(&self) -> Option<&Arc<Yuv420Frame>> {
        match self {
            Self::Memory(frame) => Some(frame),
            #[cfg(target_os = "linux")]
            Self::DmaBuf(_) => None,
        }
    }

    /// Returns whether `other` is this very frame, shared, as a source that
    /// holds an unchanged screen returns it.
    pub fn is_same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Memory(frame), Self::Memory(other)) => Arc::ptr_eq(frame, other),
            #[cfg(target_os = "linux")]
            (Self::DmaBuf(frame), Self::DmaBuf(other)) => Arc::ptr_eq(frame, other),
            #[cfg(target_os = "linux")]
            _ => false,
        }
    }
}

impl From<Arc<Yuv420Frame>> for CapturedFrame {
    fn from(frame: Arc<Yuv420Frame>) -> Self {
        Self::Memory(frame)
    }
}

impl From<Yuv420Frame> for CapturedFrame {
    fn from(frame: Yuv420Frame) -> Self {
        Self::Memory(Arc::new(frame))
    }
}

/// Produces the frames a host streams, such as a screen capture.
pub trait FrameSource: Send {
    /// Returns the width and height of the frames, in pixels.
    fn size(&self) -> (usize, usize);

    /// Returns the current frame. When the monitor changes size, the frames
    /// take the new size, and [`FrameSource::size`] returns it from then on.
    /// A source that holds the newest frame returns it again, shared rather
    /// than copied, while the screen does not change.
    fn capture(&mut self) -> Result<CapturedFrame, RuntimeError>;

    /// Asks the source for frames in memory from now on, as the encoder
    /// cannot read the frames the GPU holds that it captured. The next
    /// capture returns a frame in memory, waiting for one if it must.
    /// Sources whose frames are always in memory need do nothing.
    fn use_memory_frames(&mut self) {}

    /// Returns the pointer as of the last capture, for sources that capture
    /// it apart from the frames, which then do not show it; the client draws
    /// it (Section 9.1). Sources that return `None` show the pointer in their
    /// frames, if at all.
    fn cursor(&mut self) -> Option<CursorState> {
        None
    }

    /// Returns the captured monitor's interface scale, if the source knows
    /// it.
    fn desktop_scale(&self) -> Option<DesktopScale> {
        None
    }

    /// Asks the source to make the monitor it captures `display`, as a
    /// virtual monitor can, so that the host streams it without scaling
    /// (Section 6.3). Returns whether it did, once it did: its frames are of
    /// the new size from then on. A source that cannot leaves its monitor as
    /// it is, and the host scales its frames to the client's display.
    fn match_display(&mut self, _display: &WantedDisplay) -> bool {
        false
    }

    /// Returns where the captured monitor is in the host's desktop, if the
    /// source knows.
    fn monitor_placement(&self) -> Option<MonitorPlacement> {
        None
    }

    /// Has the source tell `arrivals` each time a new frame arrives, if it
    /// knows when frames do, and returns whether it will. The host then
    /// captures frames as they arrive, at most at the stream's frame rate,
    /// rather than on a timer that each frame waits for.
    fn watch_frames(&mut self, _arrivals: Arc<FrameArrivals>) -> bool {
        false
    }
}

/// Counts the frames a source delivers and wakes the host for each, for
/// sources that know when frames arrive.
#[derive(Debug)]
pub struct FrameArrivals {
    /// The frames that arrived so far.
    count: AtomicU64,
    /// Wakes the host.
    wake: Arc<Notify>,
}

impl FrameArrivals {
    /// Returns arrivals that wake `wake`.
    pub fn new(wake: Arc<Notify>) -> Self {
        Self {
            count: AtomicU64::new(0),
            wake,
        }
    }

    /// Records that a new frame arrived, and wakes the host.
    pub fn arrived(&self) {
        self.count.fetch_add(1, Ordering::Release);
        self.wake.notify_one();
    }

    /// Returns how many frames arrived so far.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Acquire)
    }
}

/// The luma value of the test pattern's background at the top-left corner.
const PATTERN_BASE_LUMA: usize = 32;

/// The range of the test pattern's background gradient.
const PATTERN_GRADIENT: usize = 160;

/// The luma value of the test pattern's moving square.
const PATTERN_SQUARE_LUMA: u8 = 235;

/// The side of the test pattern's moving square, in pixels.
const PATTERN_SQUARE_SIZE: usize = 48;

/// How far the square moves per frame, in pixels.
const PATTERN_SQUARE_STEP: usize = 6;

/// A synthetic source: a fixed gradient with a square that moves each frame, so
/// that consecutive frames differ in a few composition blocks only, as a desktop
/// does.
#[derive(Debug, Clone)]
pub struct TestPattern {
    /// The frame width.
    width: usize,
    /// The frame height.
    height: usize,
    /// The index of the next frame.
    next_index: u64,
}

impl TestPattern {
    /// Creates a test pattern of `width × height` pixels.
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            next_index: 0,
        }
    }

    /// Returns frame `index` of a `width × height` pattern, so that a receiver
    /// can compare what it decodes with what was sent.
    pub fn frame(width: usize, height: usize, index: u64) -> Yuv420Frame {
        let mut frame = Yuv420Frame::black(width, height);
        let travel = width.saturating_sub(PATTERN_SQUARE_SIZE).max(1);
        let square_x = (index as usize * PATTERN_SQUARE_STEP) % travel;
        let square_y = height / 3;
        let luma = frame.y_mut();
        for row in 0..height {
            for column in 0..width {
                let in_square = (square_x..square_x + PATTERN_SQUARE_SIZE).contains(&column)
                    && (square_y..square_y + PATTERN_SQUARE_SIZE).contains(&row);
                luma[row * width + column] = if in_square {
                    PATTERN_SQUARE_LUMA
                } else {
                    (PATTERN_BASE_LUMA + (row + column) * PATTERN_GRADIENT / (width + height)) as u8
                };
            }
        }
        frame
    }
}

impl FrameSource for TestPattern {
    fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    fn capture(&mut self) -> Result<CapturedFrame, RuntimeError> {
        let frame = Self::frame(self.width, self.height, self.next_index);
        self.next_index += 1;
        Ok(frame.into())
    }
}

/// Returns the composition blocks in which `current` differs from `previous`,
/// for the Blocks field of a motion frame (Section 6.5).
pub fn changed_blocks(previous: &Yuv420Frame, current: &Yuv420Frame, grid: BlockGrid) -> BlockSet {
    let mut changed = BlockSet::empty(grid.len());
    let block = BLOCK_SIZE as usize;
    let width = current.width();
    let chroma_width = current.chroma_width();
    for row in 0..grid.rows() as usize {
        for column in 0..grid.columns() as usize {
            let differs = rows_differ(
                previous.y(),
                current.y(),
                width,
                column * block..((column + 1) * block).min(width),
                row * block..((row + 1) * block).min(current.height()),
            ) || [(previous.u(), current.u()), (previous.v(), current.v())]
                .into_iter()
                .any(|(before, after)| {
                    rows_differ(
                        before,
                        after,
                        chroma_width,
                        column * block / 2..((column + 1) * block / 2).min(chroma_width),
                        row * block / 2..((row + 1) * block / 2).min(current.chroma_height()),
                    )
                });
            if differs {
                changed.insert(grid.index(column as u32, row as u32));
            }
        }
    }
    changed
}

/// Returns whether the rectangle of `columns × rows` differs between two planes
/// with `stride` samples per row.
fn rows_differ(
    before: &[u8],
    after: &[u8],
    stride: usize,
    columns: std::ops::Range<usize>,
    rows: std::ops::Range<usize>,
) -> bool {
    rows.into_iter().any(|row| {
        let span = row * stride + columns.start..row * stride + columns.end;
        before.get(span.clone()) != after.get(span)
    })
}

/// What a frame sink wants after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SinkControl {
    /// Keep streaming.
    Continue,
    /// End the session.
    Stop,
}

/// What the client knows about a presented frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// The frame's stream.
    pub stream_id: StreamId,
    /// The frame's number in its stream.
    pub frame_number: FrameNumber,
    /// The time from capture on the host to decoding on the client, once the
    /// clocks are synchronized.
    pub latency: Option<Duration>,
    /// When the frame finished decoding.
    pub decoded_at: Instant,
    /// How long the decoder took to decode it.
    pub decode_time: Duration,
}

/// When a sink shows the frames it is given.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PresentationTiming {
    /// Each frame is shown as [`FrameSink::present`] returns.
    #[default]
    Immediate,
    /// Frames are shown later, and [`FrameSink::take_presentations`] says
    /// when, or that a newer frame replaced one first.
    Deferred,
}

/// When a frame a sink was given reached the screen, for its `FRAME_TIMING`
/// record (Section 6.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Presentation {
    /// The frame's stream.
    pub stream_id: StreamId,
    /// The frame's number in its stream.
    pub frame_number: FrameNumber,
    /// When it was shown, or `None` if a newer frame replaced it first.
    pub presented_at: Option<Instant>,
}

/// Whether a display shows frames at a fixed refresh rate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum RefreshMode {
    /// The display refreshes at its fixed period.
    #[default]
    Fixed,
    /// The display is in variable refresh mode, and shows a frame as soon as
    /// it is presented.
    Variable,
}

/// The refresh timing of the display a sink shows frames on (Section 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisplayTiming {
    /// The time between refreshes.
    pub refresh_period: Duration,
    /// A recent start of vertical blanking.
    pub vsync: Instant,
    /// Whether the display is in variable refresh mode.
    pub mode: RefreshMode,
}

/// How a warp moves the client's pointer (Section 9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WarpKind {
    /// An application moved the pointer: the client moves its own pointer to
    /// match, when it has focus and the platform allows.
    Move,
    /// A device the client does not know moves the pointer: the client draws
    /// the cursor there but leaves its own pointer alone.
    Follow,
}

/// The host's pointer moved without the client's input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CursorWarp {
    /// The pointer's new position on the host's monitor, in pixels.
    pub position: (f64, f64),
    /// How the client's pointer follows.
    pub kind: WarpKind,
}

/// Receives the frames a client decodes, such as a window that shows them.
pub trait FrameSink {
    /// Shows a decoded picture and says whether to keep streaming. The sink
    /// takes the picture, so that one that keeps it to draw later need not
    /// copy it; a hardware decoder's picture may be in GPU memory, which a
    /// sink that needs it in memory copies from with
    /// [`DecodedPicture::with_memory`].
    fn present(&mut self, frame: DecodedPicture, info: FrameInfo) -> SinkControl;

    /// Returns when the sink shows the frames it is given.
    fn presentation_timing(&self) -> PresentationTiming {
        PresentationTiming::Immediate
    }

    /// Returns when the frames given since the last call reached the
    /// screen, oldest first, for a sink whose timing is
    /// [`PresentationTiming::Deferred`].
    fn take_presentations(&mut self) -> Vec<Presentation> {
        Vec::new()
    }

    /// Returns the refresh timing of the display the frames are shown on,
    /// if the sink knows it.
    fn display_timing(&mut self) -> Option<DisplayTiming> {
        None
    }

    /// Shows the host's cursor image at the user's pointer, or hides the
    /// pointer when `None` (Section 9.3). The image is the size it has on the
    /// host's monitor.
    fn set_cursor(&mut self, cursor: Option<&CursorImage>) {
        let _ = cursor;
    }

    /// Moves the cursor where the host's pointer went (Section 9.4).
    fn warp_cursor(&mut self, warp: CursorWarp) {
        let _ = warp;
    }

    /// Follows the host's pointer mode (Section 8.9): in relative mode the
    /// sink captures the pointer and reports raw motion, and in absolute mode
    /// it releases it, keeping it within any confinement.
    fn set_pointer_mode(&mut self, mode: &PointerMode) {
        let _ = mode;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A position on the captured monitor is placed within the whole
    /// desktop, and the union of monitors is the desktop.
    #[test]
    fn places_monitors_in_the_desktop() {
        let left = LayoutRect {
            x: 0,
            y: 0,
            width: 1000,
            height: 500,
        };
        let right = LayoutRect {
            x: 1000,
            y: 100,
            width: 500,
            height: 300,
        };
        let desktop = LayoutRect::union([left, right]).unwrap();
        assert_eq!(
            desktop,
            LayoutRect {
                x: 0,
                y: 0,
                width: 1500,
                height: 500
            }
        );
        assert_eq!(LayoutRect::union([]), None);
        let placement = MonitorPlacement {
            monitor: right,
            desktop,
        };
        // The middle of a 1000 × 600 pixel capture of the right monitor, at
        // 200 %.
        let (x, y) = placement.desktop_share((500.0, 300.0), (1000, 600));
        assert!((x - 1250.0 / 1500.0).abs() < 1e-9);
        assert!((y - 250.0 / 500.0).abs() < 1e-9);
        // A lone monitor maps one to one.
        let alone = MonitorPlacement {
            monitor: left,
            desktop: left,
        };
        assert_eq!(
            alone.desktop_share((250.0, 125.0), (1000, 500)),
            (0.25, 0.25)
        );
    }

    /// A monitor's scale is its pixel width over its desktop width, within
    /// the scales desktops use.
    #[test]
    fn measures_desktop_scales() {
        let scale = DesktopScale::from_widths(5120, 3413).unwrap();
        assert_eq!(scale.thousandths(), 1500);
        assert_eq!(scale.percent(), 150);
        assert_eq!(
            DesktopScale::from_widths(2560, 2560),
            Some(DesktopScale::UNSCALED)
        );
        assert_eq!(DesktopScale::from_widths(5120, 0), None);
        assert_eq!(DesktopScale::from_widths(5120, 100), None);
        assert_eq!(DesktopScale::default().percent(), 100);
    }

    /// Consecutive test pattern frames differ only where the square moves.
    #[test]
    fn test_pattern_changes_few_blocks() {
        let (width, height) = (640, 384);
        let grid = BlockGrid::for_monitor(width as u32, height as u32);
        let first = TestPattern::frame(width, height, 0);
        let second = TestPattern::frame(width, height, 1);
        let changed = changed_blocks(&first, &second, grid);
        assert!(changed.count() > 0);
        assert!(changed.count() <= 4, "{} blocks changed", changed.count());
        assert!(changed_blocks(&first, &first, grid).is_empty());
    }

    /// A source produces successive pattern frames.
    #[test]
    fn source_advances() {
        let mut source = TestPattern::new(128, 64);
        assert_eq!(source.size(), (128, 64));
        for index in 0..2 {
            let captured = source.capture().unwrap();
            let frame = captured.in_memory().unwrap();
            assert_eq!(**frame, TestPattern::frame(128, 64, index));
        }
    }
}
