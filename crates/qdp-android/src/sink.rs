//! The sink that shows the session's video in the app's surface, and passes
//! the host's cursor and pointer mode to the app.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use qdp::message::{CursorImage, PointerKind, PointerMode};
use qdp_codec::DecodedPicture;
use qdp_runtime::{CursorWarp, FrameInfo, FrameSink, LatencyStages, SinkControl};
use tracing::{info, warn};

use crate::cursor::CursorBitmap;
use crate::events::{Event, SlowestFrame, VideoStats};

/// How often the video's statistics are logged and sent to the app.
const STATS_INTERVAL: Duration = Duration::from_secs(1);

/// The host's cursor as the app last heard of it, or `None` while hidden.
pub type SharedCursor = Arc<Mutex<Option<CursorBitmap>>>;

/// Shows decoded frames in the app's surface.
pub struct AndroidSink {
    /// Where the app's events go.
    events: Sender<Event>,
    /// The host's cursor, which the app reads when told it changed.
    cursor: SharedCursor,
    /// The size of the last frame, in pixels.
    video: Option<(usize, usize)>,
    /// The host's pointer mode, as last passed on.
    pointer: Option<PointerKind>,
    /// Draws pictures in memory, from the software decoder.
    painter: Painter,
    /// The video since the statistics were last sent.
    stats: StatsWindow,
}

impl AndroidSink {
    /// Returns a sink that sends the app's events to `events` and keeps the
    /// host's cursor in `cursor`.
    pub fn new(events: Sender<Event>, cursor: SharedCursor) -> Self {
        Self {
            events,
            cursor,
            video: None,
            pointer: None,
            painter: Painter::new(),
            stats: StatsWindow::new(Instant::now()),
        }
    }

    /// Tells the app something; an app that stopped listening is not told.
    fn send(&self, event: Event) {
        let _ = self.events.send(event);
    }

    /// Shows `frame`: MediaCodec's in the surface it decoded for, and the
    /// software decoder's by drawing it there.
    fn show(&mut self, frame: DecodedPicture) -> Result<(), String> {
        match frame {
            #[cfg(target_os = "android")]
            DecodedPicture::Surface(picture) => picture.render().map_err(|error| error.to_string()),
            frame => frame
                .with_memory(|memory| self.painter.draw(memory))
                .map_err(|error| error.to_string())
                .and_then(|drawn| drawn),
        }
    }
}

impl FrameSink for AndroidSink {
    fn present(&mut self, frame: DecodedPicture, info: FrameInfo) -> SinkControl {
        let (width, height) = frame.size();
        if self.video != Some((width, height)) {
            info!(width, height, "video size");
            self.video = Some((width, height));
            self.send(Event::Video { width, height });
        }
        match self.show(frame) {
            Ok(()) => self
                .stats
                .shown(info.latency, info.decode_time, info.stages()),
            Err(error) => self.stats.unshown(error),
        }
        if let Some(stats) = self.stats.finish(Instant::now()) {
            self.send(Event::Stats(stats));
        }
        SinkControl::Continue
    }

    fn set_cursor(&mut self, cursor: Option<&CursorImage>) {
        *self.cursor.lock().unwrap_or_else(PoisonError::into_inner) = cursor.map(CursorBitmap::new);
        self.send(Event::Cursor);
    }

    fn warp_cursor(&mut self, warp: CursorWarp) {
        let (x, y) = warp.position;
        self.send(Event::Warp {
            x,
            y,
            kind: warp.kind,
        });
    }

    fn set_pointer_mode(&mut self, mode: &PointerMode) {
        if self.pointer != Some(mode.mode) {
            self.pointer = Some(mode.mode);
            self.send(Event::PointerMode(mode.mode));
        }
    }
}

/// Draws pictures in memory into the app's surface.
#[cfg(target_os = "android")]
use crate::software::SoftwareWindow as Painter;

/// Draws pictures in memory: only the app's surface can show them.
#[cfg(not(target_os = "android"))]
struct Painter;

#[cfg(not(target_os = "android"))]
impl Painter {
    /// Returns the painter.
    fn new() -> Self {
        Self
    }

    /// Refuses to draw `frame`, having nowhere to.
    fn draw(&mut self, frame: &qdp_codec::Yuv420Frame) -> Result<(), String> {
        let _ = frame;
        Err("pictures are drawn only into an Android surface".to_owned())
    }
}

/// What the video did since the statistics were last sent.
#[derive(Debug)]
struct StatsWindow {
    /// When the window started.
    started: Instant,
    /// The frames shown.
    frames: u32,
    /// Their total latency, of those whose latency is known.
    latency: Duration,
    /// The frames whose latency is known.
    timed: u32,
    /// Their total decode time.
    decode: Duration,
    /// The slowest frame, of those whose latency is known.
    worst: Option<SlowestFrame>,
    /// The frames that could not be shown.
    unshown: u32,
    /// Why the last of them could not be.
    last_error: Option<String>,
}

impl StatsWindow {
    /// Returns an empty window starting at `now`.
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            frames: 0,
            latency: Duration::ZERO,
            timed: 0,
            decode: Duration::ZERO,
            worst: None,
            unshown: 0,
            last_error: None,
        }
    }

    /// Counts a frame shown, `latency` after capture if known, through
    /// `stages`, that took `decode` to decode.
    fn shown(
        &mut self,
        latency: Option<Duration>,
        decode: Duration,
        stages: Option<LatencyStages>,
    ) {
        self.frames += 1;
        self.decode += decode;
        let Some(latency) = latency else {
            return;
        };
        self.latency += latency;
        self.timed += 1;
        let slower = self.worst.is_none_or(|worst| latency > worst.latency);
        if let (true, Some(stages)) = (slower, stages) {
            self.worst = Some(SlowestFrame { latency, stages });
        }
    }

    /// Counts a frame that could not be shown, for `error`.
    fn unshown(&mut self, error: String) {
        self.unshown += 1;
        self.last_error = Some(error);
    }

    /// Returns the window's statistics once it has lasted
    /// [`STATS_INTERVAL`] at `now`, logging them, and starts the next.
    fn finish(&mut self, now: Instant) -> Option<VideoStats> {
        if now.saturating_duration_since(self.started) < STATS_INTERVAL {
            return None;
        }
        let window = std::mem::replace(self, Self::new(now));
        let stats = VideoStats {
            frames: window.frames,
            latency: window.latency.checked_div(window.timed),
            decode: window.decode.checked_div(window.frames).unwrap_or_default(),
            worst: window.worst,
        };
        let milliseconds = |duration: Duration| duration.as_secs_f64() * 1000.0;
        let worst = stats.worst.map(|worst| worst.stages);
        info!(
            fps = stats.frames,
            latency_ms = stats.latency.map(milliseconds),
            decode_ms = milliseconds(stats.decode),
            worst_ms = stats.worst.map(|worst| milliseconds(worst.latency)),
            worst_to_first_fragment_ms = worst.map(|stages| milliseconds(stages.to_first_fragment)),
            worst_transfer_ms = worst.map(|stages| milliseconds(stages.transfer)),
            worst_queued_ms = worst.map(|stages| milliseconds(stages.queued)),
            worst_decode_ms = worst.map(|stages| milliseconds(stages.decode)),
            "video"
        );
        if let Some(error) = window.last_error {
            warn!(frames = window.unshown, %error, "frames could not be shown");
        }
        Some(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window averages the frames it counted once it has lasted its
    /// interval, keeps the slowest, and the next starts empty.
    #[test]
    fn averages_each_interval() {
        let start = Instant::now();
        let mut window = StatsWindow::new(start);
        let stages = |to_first_fragment_ms| LatencyStages {
            to_first_fragment: Duration::from_millis(to_first_fragment_ms),
            transfer: Duration::from_millis(1),
            queued: Duration::ZERO,
            decode: Duration::from_millis(1),
        };
        window.shown(
            Some(Duration::from_millis(4)),
            Duration::from_millis(1),
            Some(stages(2)),
        );
        window.shown(
            Some(Duration::from_millis(6)),
            Duration::from_millis(3),
            Some(stages(4)),
        );
        window.shown(None, Duration::from_millis(2), None);
        window.unshown("abandoned".to_owned());
        assert_eq!(window.finish(start + STATS_INTERVAL / 2), None);
        let stats = window.finish(start + STATS_INTERVAL).unwrap();
        assert_eq!(
            stats,
            VideoStats {
                frames: 3,
                latency: Some(Duration::from_millis(5)),
                decode: Duration::from_millis(2),
                worst: Some(SlowestFrame {
                    latency: Duration::from_millis(6),
                    stages: stages(4),
                }),
            }
        );
        let empty = window.finish(start + STATS_INTERVAL * 2).unwrap();
        assert_eq!(
            empty,
            VideoStats {
                frames: 0,
                latency: None,
                decode: Duration::ZERO,
                worst: None,
            }
        );
    }
}
