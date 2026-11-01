//! What the app is told about a session, as lines of text it reads one at a
//! time from a thread of its own: the session's events that concern it, and
//! the stream window's.
//!
//! Besides the lines of [`qdp_app::events`], except `cursor`, `warp` and
//! `rumble`, which the stream window handles itself, the app reads:
//!
//! | Line | Meaning |
//! |---|---|
//! | `window shown` | The stream window shows the video; the app may step back. |
//! | `microphone on\|off` | The user turned the microphone on or off. |
//!
//! A session's last line is `ended` or `failed <error>`.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use qdp_app::events::{Ending, Event};
use qdp_runtime::Microphone;

/// Something the stream window tells the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowLine {
    /// The window shows the video.
    Shown,
    /// The user turned the microphone on or off.
    Microphone(Microphone),
}

impl WindowLine {
    /// Returns the line the app reads.
    pub fn line(self) -> String {
        match self {
            Self::Shown => "window shown".to_owned(),
            Self::Microphone(Microphone::On) => "microphone on".to_owned(),
            Self::Microphone(Microphone::Off) => "microphone off".to_owned(),
        }
    }
}

/// Returns whether the app is told of `event`, rather than only the stream
/// window, which draws the cursor and rumbles the gamepads.
pub fn for_app(event: &Event) -> bool {
    !matches!(
        event,
        Event::Cursor | Event::Warp { .. } | Event::Rumble { .. }
    )
}

/// The lines waiting for the app, and whether the last has been queued.
#[derive(Debug, Default)]
struct Lines {
    /// The lines not yet read, oldest first.
    waiting: VecDeque<String>,
    /// Whether the session's last line has been queued, after which no
    /// other is.
    finished: bool,
}

/// The lines of one session waiting for the app.
#[derive(Debug, Default)]
pub struct EventQueue {
    /// The lines.
    lines: Mutex<Lines>,
    /// Signalled when a line is queued.
    queued: Condvar,
}

impl EventQueue {
    /// Locks the lines.
    fn lock(&self) -> MutexGuard<'_, Lines> {
        self.lines.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues a line, unless the session's last has been queued.
    pub fn push(&self, line: String) {
        let mut lines = self.lock();
        if lines.finished {
            return;
        }
        lines.waiting.push_back(line);
        drop(lines);
        self.queued.notify_all();
    }

    /// Queues the session's event, if the app is told of it, as its last
    /// line if it ended the session.
    pub fn push_event(&self, event: &Event) {
        match event {
            Event::Ended(ending) => self.finish(ending),
            event if for_app(event) => self.push(event.to_string()),
            _ => {}
        }
    }

    /// Queues the session's last line, saying how it ended, unless one was
    /// queued already.
    pub fn finish(&self, ending: &Ending) {
        let mut lines = self.lock();
        if lines.finished {
            return;
        }
        lines
            .waiting
            .push_back(Event::Ended(ending.clone()).to_string());
        lines.finished = true;
        drop(lines);
        self.queued.notify_all();
    }

    /// Returns whether the session's last line has been queued.
    pub fn finished(&self) -> bool {
        self.lock().finished
    }

    /// Returns the oldest line not yet read, waiting up to `timeout` for
    /// one, or `None` if none came. Once the last line has been read it
    /// waits out `timeout` too, so that a reader polling in a loop does not
    /// spin.
    pub fn next(&self, timeout: Duration) -> Option<String> {
        let deadline = Instant::now() + timeout;
        let mut lines = self.lock();
        loop {
            if let Some(line) = lines.waiting.pop_front() {
                return Some(line);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            lines = self
                .queued
                .wait_timeout(lines, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use qdp::message::{PointerKind, PointerMode};
    use qdp_runtime::WarpKind;

    use super::*;

    /// Lines come out in order; only the app's events are queued.
    #[test]
    fn queues_the_apps_lines() {
        let queue = EventQueue::default();
        queue.push_event(&Event::Connecting);
        queue.push_event(&Event::Cursor);
        queue.push_event(&Event::Warp {
            x: 1.0,
            y: 2.0,
            kind: WarpKind::Move,
        });
        queue.push_event(&Event::Rumble {
            slot: 0,
            low: 1,
            high: 2,
        });
        queue.push_event(&Event::PointerMode(PointerMode {
            mode: PointerKind::Relative,
            confinement: None,
        }));
        queue.push(WindowLine::Shown.line());
        queue.push(WindowLine::Microphone(Microphone::On).line());
        let read: Vec<_> = std::iter::from_fn(|| queue.next(Duration::ZERO)).collect();
        assert_eq!(
            read,
            [
                "connecting",
                "pointer relative",
                "window shown",
                "microphone on"
            ]
        );
    }

    /// The first ending is the last line; nothing is queued after it.
    #[test]
    fn ends_once() {
        let queue = EventQueue::default();
        queue.push_event(&Event::Connected);
        assert!(!queue.finished());
        queue.finish(&Ending::Failed("the host closed".to_owned()));
        queue.push_event(&Event::Ended(Ending::Normal));
        queue.push(WindowLine::Shown.line());
        assert!(queue.finished());
        assert_eq!(queue.next(Duration::ZERO).as_deref(), Some("connected"));
        assert_eq!(
            queue.next(Duration::ZERO).as_deref(),
            Some("failed the host closed")
        );
        assert_eq!(queue.next(Duration::ZERO), None);
    }

    /// A reader waiting for a line gets it as soon as it is queued, and one
    /// waiting with nothing to come waits out its timeout.
    #[test]
    fn waits_for_lines() {
        let queue = Arc::new(EventQueue::default());
        let reader = std::thread::spawn({
            let queue = Arc::clone(&queue);
            move || queue.next(Duration::from_secs(10))
        });
        std::thread::sleep(Duration::from_millis(20));
        queue.push_event(&Event::Connected);
        assert_eq!(reader.join().unwrap().as_deref(), Some("connected"));
        let started = Instant::now();
        assert_eq!(queue.next(Duration::from_millis(30)), None);
        assert!(started.elapsed() >= Duration::from_millis(30));
    }
}
