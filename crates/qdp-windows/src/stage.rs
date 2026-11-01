//! What the render thread draws: the newest picture, placed in the stream
//! window, handed over from the session's thread and resized from the
//! window's; and when each picture reached the screen, for the host's timing
//! (Section 5.3).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use qdp::message::{FrameNumber, StreamId};
use qdp_app::placement::{Rect, fit};
use qdp_app::sink::Presenter;
use qdp_codec::DecodedPicture;
use qdp_desktop::{Fate, PixelRect, Scene, Submitter};
use qdp_runtime::{FrameInfo, Presentation, PresentationTiming};
use tracing::info;

/// How often the drawing's statistics are logged.
const DRAW_STATS_INTERVAL: Duration = Duration::from_secs(1);

/// Which frame a picture is, to report when it reached the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameId {
    /// The frame's stream.
    stream_id: StreamId,
    /// The frame's number in its stream.
    frame_number: FrameNumber,
}

/// When the pictures drawn reached the screen, until the session takes it.
pub(crate) type Presentations = Arc<Mutex<Vec<Presentation>>>;

/// The scene the render thread is given, shared by the session's thread,
/// which brings pictures, and the window's, which resizes it.
pub(crate) type SharedStage = Arc<Mutex<Stage>>;

/// What is drawn and where.
pub(crate) struct Stage {
    /// Hands the render thread what to draw.
    submitter: Submitter<FrameId>,
    /// The window's client area, in pixels.
    window: (u32, u32),
    /// The picture's size, in pixels, once one arrived.
    video: Option<(usize, usize)>,
}

/// Locks a stage.
pub(crate) fn lock(stage: &SharedStage) -> MutexGuard<'_, Stage> {
    stage.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Stage {
    /// Returns a stage drawing through `submitter` into a window of
    /// `window` pixels.
    pub(crate) fn new(submitter: Submitter<FrameId>, window: (u32, u32)) -> Self {
        Self {
            submitter,
            window,
            video: None,
        }
    }

    /// Hands the render thread `picture`, if there is one, as the picture
    /// from now on, and the scene around it.
    pub(crate) fn submit(&mut self, picture: Option<(DecodedPicture, FrameId)>) {
        if let Some((picture, _)) = &picture {
            self.video = Some(picture.size());
        }
        if picture.is_none() && self.video.is_none() {
            return;
        }
        let scene = Scene {
            size: self.window,
            picture: self.placement().map(|(rect, _)| PixelRect {
                left: rect.left,
                top: rect.top,
                width: rect.width,
                height: rect.height,
            }),
            overlay: None,
        };
        self.submitter.submit(picture, scene);
    }

    /// Takes the window's new size, in pixels, and redraws.
    pub(crate) fn resize(&mut self, window: (u32, u32)) {
        if self.window != window {
            self.window = window;
            self.submit(None);
        }
    }

    /// Returns where the picture sits in the window, and its size, once one
    /// arrived.
    pub(crate) fn placement(&self) -> Option<(Rect, (usize, usize))> {
        let video = self.video?;
        Some((fit(video, self.window)?, video))
    }
}

/// Shows the session's pictures through the stage, and tells the session
/// when they reached the screen.
pub(crate) struct WindowsPresenter {
    /// What is drawn.
    stage: SharedStage,
    /// When the pictures drawn reached the screen.
    presentations: Presentations,
}

impl WindowsPresenter {
    /// Returns a presenter drawing on `stage`, whose render thread reports
    /// into `presentations`.
    pub(crate) fn new(stage: SharedStage, presentations: Presentations) -> Self {
        Self {
            stage,
            presentations,
        }
    }
}

impl Presenter for WindowsPresenter {
    fn show(&mut self, frame: DecodedPicture, info: &FrameInfo) -> Result<(), String> {
        let id = FrameId {
            stream_id: info.stream_id,
            frame_number: info.frame_number,
        };
        lock(&self.stage).submit(Some((frame, id)));
        Ok(())
    }

    fn presentation_timing(&self) -> PresentationTiming {
        PresentationTiming::Deferred
    }

    fn take_presentations(&mut self) -> Vec<Presentation> {
        std::mem::take(
            &mut *self
                .presentations
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }
}

/// Returns what the render thread does with each picture's fate: keeps when
/// it reached the screen in `presentations`, and logs the drawing's
/// statistics each second, as the desktop client does.
pub(crate) fn reporter(presentations: Presentations) -> impl FnMut(Fate<FrameId>) + Send + 'static {
    let mut stats = DrawStats::new(Instant::now());
    move |fate| {
        let (id, presented_at) = match fate {
            Fate::Shown {
                tag,
                at,
                drew_in,
                waited,
            } => {
                stats.drew(drew_in, waited, at);
                (tag, Some(at))
            }
            Fate::Unshown(tag) => (tag, None),
        };
        presentations
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Presentation {
                stream_id: id.stream_id,
                frame_number: id.frame_number,
                presented_at,
            });
    }
}

/// How fast frames are drawn, over each second: the render thread draws the
/// newest, so frames decoded faster than they can be drawn are passed over.
#[derive(Debug)]
struct DrawStats {
    /// When the interval started.
    started: Instant,
    /// The new frames drawn.
    frames: u32,
    /// The time drawing them took.
    busy: Duration,
    /// The longest draw.
    longest: Duration,
    /// The time spent waiting for the swap chain.
    waited: Duration,
}

impl DrawStats {
    /// Starts an interval at `now`.
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            frames: 0,
            busy: Duration::ZERO,
            longest: Duration::ZERO,
            waited: Duration::ZERO,
        }
    }

    /// Counts a frame drawn in `took`, after `waited` for the swap chain,
    /// and logs the interval once it has ended at `now`.
    fn drew(&mut self, took: Duration, waited: Duration, now: Instant) {
        self.frames += 1;
        self.busy += took;
        self.longest = self.longest.max(took);
        self.waited += waited;
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed < DRAW_STATS_INTERVAL {
            return;
        }
        let milliseconds = |time: Duration| format!("{:.1}", time.as_secs_f64() * 1000.0);
        info!(
            drawn_fps = format!("{:.1}", f64::from(self.frames) / elapsed.as_secs_f64()),
            draw_ms = milliseconds(self.busy / self.frames),
            draw_max_ms = milliseconds(self.longest),
            wait_ms = milliseconds(self.waited / self.frames),
            "drawing"
        );
        *self = Self::new(now);
    }
}
