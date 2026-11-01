//! What the render thread draws: the newest picture, placed in the layer,
//! and the host's cursor over it where the app asks for it, handed over
//! from the session's thread and the app's input; and when each picture
//! reached the screen, for the host's timing (Section 5.3).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use qdp::message::{FrameNumber, ImageSerial, StreamId};
use qdp_app::cursor::ScaledCursor;
use qdp_app::sink::{Presenter, SharedCursor};
use qdp_codec::DecodedPicture;
use qdp_desktop::{Fate, Overlay, PixelRect, Scene, Submitter};
use qdp_runtime::{FrameInfo, Presentation, PresentationTiming};
use tracing::info;

use crate::placement::fit;

/// An interface scale of one, in percent.
const UNSCALED_PERCENT: f64 = 100.0;

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
/// which brings pictures, and the app's, which resizes the layer and moves
/// the pointer.
pub(crate) type SharedStage = Arc<Mutex<Stage>>;

/// What is drawn and where.
pub(crate) struct Stage {
    /// Hands the render thread what to draw.
    submitter: Submitter<FrameId>,
    /// The layer's size, in pixels.
    layer: (u32, u32),
    /// The picture's size, in pixels, once one arrived.
    video: Option<(usize, usize)>,
    /// The pointer, in video pixels.
    pointer: (f64, f64),
    /// Whether the host's cursor is drawn over the picture.
    draw_cursor: bool,
    /// The host's cursor, as the session keeps it.
    cursor: SharedCursor,
    /// The cursor scaled for the picture, and what it was scaled from.
    scaled: Option<(CursorKey, ScaledCursor)>,
}

/// What a scaled cursor was scaled from.
#[derive(Debug, Clone, Copy, PartialEq)]
struct CursorKey {
    /// The image's serial.
    serial: ImageSerial,
    /// The factor it was scaled by.
    factor: f64,
}

/// Locks a stage.
pub(crate) fn lock(stage: &SharedStage) -> MutexGuard<'_, Stage> {
    stage.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Stage {
    /// Returns a stage drawing through `submitter` into a layer of `layer`
    /// pixels, with the host's cursor kept in `cursor`.
    pub(crate) fn new(
        submitter: Submitter<FrameId>,
        layer: (u32, u32),
        cursor: SharedCursor,
    ) -> Self {
        Self {
            submitter,
            layer,
            video: None,
            pointer: (0.0, 0.0),
            draw_cursor: false,
            cursor,
            scaled: None,
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
        let scene = self.scene();
        self.submitter.submit(picture, scene);
    }

    /// Takes the layer's new size, in pixels, and redraws.
    pub(crate) fn resize(&mut self, layer: (u32, u32)) {
        if self.layer != layer {
            self.layer = layer;
            self.submit(None);
        }
    }

    /// Moves the cursor drawn to `x, y` in video pixels, and redraws if it
    /// is drawn.
    pub(crate) fn move_pointer(&mut self, x: f64, y: f64) {
        self.pointer = (x, y);
        if self.draw_cursor {
            self.submit(None);
        }
    }

    /// Takes the host's cursor as the session keeps it.
    pub(crate) fn set_cursor(&mut self, cursor: SharedCursor) {
        self.cursor = cursor;
        self.scaled = None;
    }

    /// Draws the host's cursor over the picture, or stops.
    pub(crate) fn set_draw_cursor(&mut self, draw: bool) {
        if self.draw_cursor != draw {
            self.draw_cursor = draw;
            self.submit(None);
        }
    }

    /// Redraws with the host's cursor as it now is, if it is drawn.
    pub(crate) fn cursor_changed(&mut self) {
        if self.draw_cursor {
            self.submit(None);
        }
    }

    /// Returns the scene: the picture placed in the layer, and the cursor
    /// over it if it is drawn.
    fn scene(&mut self) -> Scene {
        let placed = self
            .video
            .and_then(|video| fit(video, self.layer).zip(Some(video)));
        let overlay = match placed {
            Some((rect, video)) if self.draw_cursor => self.overlay(rect, video),
            _ => None,
        };
        Scene {
            size: self.layer,
            picture: placed.map(|(rect, _)| PixelRect {
                left: rect.left,
                top: rect.top,
                width: rect.width,
                height: rect.height,
            }),
            overlay,
        }
    }

    /// Returns the host's cursor at the pointer, scaled as the picture is
    /// (Section 9.2), with the picture at `rect` in the layer showing a
    /// `video` picture.
    fn overlay(&mut self, rect: crate::placement::Rect, video: (usize, usize)) -> Option<Overlay> {
        let picture = rect.width / video.0.max(1) as f64;
        {
            // The image is scaled again only when it or the picture's scale
            // changed, not for every frame.
            let image = self.cursor.lock().unwrap_or_else(PoisonError::into_inner);
            let image = image.as_ref()?;
            let factor = picture * UNSCALED_PERCENT / (image.scale.into_inner().max(1) as f64);
            let key = CursorKey {
                serial: image.image_serial,
                factor,
            };
            if self
                .scaled
                .as_ref()
                .is_none_or(|(scaled, _)| *scaled != key)
            {
                self.scaled = Some((key, ScaledCursor::new(image, factor)));
            }
        }
        let (_, cursor) = self.scaled.as_ref()?;
        let (x, y) = self.pointer;
        Some(Overlay {
            width: cursor.width,
            height: cursor.height,
            rgba: Arc::clone(&cursor.rgba),
            left: (rect.left + x * picture).round() - cursor.hotspot.0 as f64,
            top: (rect.top + y * picture).round() - cursor.hotspot.1 as f64,
        })
    }
}

/// Shows the session's pictures through the stage, and tells the session
/// when they reached the screen.
pub(crate) struct ApplePresenter {
    /// What is drawn.
    stage: SharedStage,
    /// When the pictures drawn reached the screen.
    presentations: Presentations,
}

impl ApplePresenter {
    /// Returns a presenter drawing on `stage`, whose render thread reports
    /// into `presentations`.
    pub(crate) fn new(stage: SharedStage, presentations: Presentations) -> Self {
        Self {
            stage,
            presentations,
        }
    }
}

impl Presenter for ApplePresenter {
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
pub(crate) fn reporter(presentations: Presentations) -> Box<dyn FnMut(Fate<FrameId>) + Send> {
    let mut stats = DrawStats::new(Instant::now());
    Box::new(move |fate| {
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
    })
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
    /// The time spent waiting for a drawable.
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

    /// Counts a frame drawn in `took`, after `waited` for a drawable, and
    /// logs the interval once it has ended at `now`.
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
