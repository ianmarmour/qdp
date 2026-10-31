//! Frame sources on the host, frame sinks on the client, and the comparison of
//! frames that fills block maps.

use std::sync::Arc;
use std::time::{Duration, Instant};

use qdp::message::{BLOCK_SIZE, BlockGrid, BlockSet, CursorImage, FrameNumber, PointerMode};
use qdp_codec::Yuv420Frame;

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

/// The pointer, as a capture source last saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CursorState {
    /// The hotspot's position in the frame, in pixels, or `None` while the
    /// pointer is hidden or on another monitor.
    pub position: Option<(i32, i32)>,
    /// The pointer's image, shared until it changes.
    pub shape: Option<Arc<CursorShape>>,
}

/// Produces the frames a host streams, such as a screen capture.
pub trait FrameSource: Send {
    /// Returns the width and height of the frames, in pixels.
    fn size(&self) -> (usize, usize);

    /// Returns the current frame.
    fn capture(&mut self) -> Result<Yuv420Frame, RuntimeError>;

    /// Returns the pointer as of the last capture, for sources that capture
    /// it apart from the frames, which then do not show it; the client draws
    /// it (Section 9.1). Sources that return `None` show the pointer in their
    /// frames, if at all.
    fn cursor(&mut self) -> Option<CursorState> {
        None
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

    fn capture(&mut self) -> Result<Yuv420Frame, RuntimeError> {
        let frame = Self::frame(self.width, self.height, self.next_index);
        self.next_index += 1;
        Ok(frame)
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
    /// The frame's number in its stream.
    pub frame_number: FrameNumber,
    /// The time from capture on the host to decoding on the client, once the
    /// clocks are synchronized.
    pub latency: Option<Duration>,
    /// When the frame finished decoding.
    pub decoded_at: Instant,
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
    /// Shows a decoded frame and says whether to keep streaming.
    fn present(&mut self, frame: &Yuv420Frame, info: FrameInfo) -> SinkControl;

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
        assert_eq!(source.capture().unwrap(), TestPattern::frame(128, 64, 0));
        assert_eq!(source.capture().unwrap(), TestPattern::frame(128, 64, 1));
    }
}
