//! The host's rate control (Section 5.5): video must not persistently exceed
//! the rate the congestion controller allows. Frames are skipped while the
//! network is behind; on top of that, the encoder's quantizer rises when
//! frames take too long to cross the path, are skipped, or show a queue
//! building on the path, and falls back toward the configured quality while
//! frames cross it easily.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use qdp::message::{FrameNumber, FrameTimingRecord, StreamId, Timestamp};
use qdp_quic::PathStats;
use tracing::debug;

/// The share of a frame interval a frame may take to cross the path, at the
/// rate the congestion window allows, before the quality is lowered.
const HIGH_SHARE: f64 = 0.5;

/// The share of a frame interval below which a frame crosses the path
/// easily, so that the quality may be raised.
const LOW_SHARE: f64 = 0.125;

/// The queuing delay above which the quality is lowered (Section 5.5.3).
const QUEUE_LIMIT: Duration = Duration::from_millis(4);

/// How many frames in a row must show a queue before the quality is
/// lowered: a lone late arrival is more often the client being busy than a
/// queue on the path.
const QUEUED_FRAMES: u8 = 3;

/// How long the smallest transit time counts as the path's own, without a
/// queue (Section 5.5.3).
const TRANSIT_WINDOW: Duration = Duration::from_secs(5);

/// The frames whose sending times are kept for their `FRAME_TIMING`
/// records.
const KEPT_FRAMES: usize = 256;

/// The quantizer steps the quality is lowered by at once.
const STEP_UP: u8 = 12;

/// The quantizer steps the quality is raised by at once.
const STEP_DOWN: u8 = 2;

/// How long after the quality was lowered it may be raised again.
const RAISE_DELAY: Duration = Duration::from_millis(500);

/// The shortest time between two raises of the quality.
const RAISE_INTERVAL: Duration = Duration::from_millis(100);

/// The encoder's quantizer, chosen from how frames cross the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RateControl {
    /// The configured quantizer: the best quality the host sends.
    best: u8,
    /// The quantizer of the next frame.
    quantizer: u8,
    /// When the quality was last lowered.
    lowered_at: Option<Instant>,
    /// When the quality was last raised.
    raised_at: Option<Instant>,
    /// How many frames in a row showed a queue.
    queued_frames: u8,
}

impl RateControl {
    /// Starts at the configured quantizer, `best`.
    pub(crate) fn new(best: u8) -> Self {
        Self {
            best,
            quantizer: best,
            lowered_at: None,
            raised_at: None,
            queued_frames: 0,
        }
    }

    /// Returns the quantizer of the next frame.
    pub(crate) fn quantizer(&self) -> u8 {
        self.quantizer
    }

    /// Notes that a frame was skipped because the network was behind.
    pub(crate) fn frame_skipped(&mut self, now: Instant) {
        self.lower(now);
    }

    /// Notes the queuing delay a frame met on the path: a queue that
    /// persists lowers the quality.
    pub(crate) fn queue_measured(&mut self, delay: Duration, now: Instant) {
        if delay <= QUEUE_LIMIT {
            self.queued_frames = 0;
            return;
        }
        self.queued_frames += 1;
        if self.queued_frames == QUEUED_FRAMES {
            self.queued_frames = 0;
            self.lower(now);
        }
    }

    /// Notes that a frame of `bytes` bytes was sent on `path`, one frame
    /// interval after the last. Key frames are larger by nature and only
    /// ever lower the quality.
    pub(crate) fn frame_sent(
        &mut self,
        bytes: usize,
        key_frame: bool,
        path: &PathStats,
        frame_interval: Duration,
        now: Instant,
    ) {
        let Some(share) = transmission_share(bytes, path, frame_interval) else {
            return;
        };
        if share > HIGH_SHARE {
            self.lower(now);
        } else if share < LOW_SHARE && !key_frame {
            self.raise(now);
        }
    }

    /// Lowers the quality, down to the largest quantizer.
    fn lower(&mut self, now: Instant) {
        self.lowered_at = Some(now);
        let quantizer = self.quantizer.saturating_add(STEP_UP);
        if quantizer != self.quantizer {
            debug!(quantizer, "video quality lowered");
            self.quantizer = quantizer;
        }
    }

    /// Raises the quality toward the best, if it was not lowered recently.
    fn raise(&mut self, now: Instant) {
        let settled = |since: Option<Instant>, wait: Duration| {
            since.is_none_or(|since| now.saturating_duration_since(since) >= wait)
        };
        if self.quantizer == self.best
            || !settled(self.lowered_at, RAISE_DELAY)
            || !settled(self.raised_at, RAISE_INTERVAL)
        {
            return;
        }
        self.raised_at = Some(now);
        self.quantizer = self.quantizer.saturating_sub(STEP_DOWN).max(self.best);
        debug!(quantizer = self.quantizer, "video quality raised");
    }
}

/// Finds the queuing delay frames meet on the path from their `FRAME_TIMING`
/// records (Section 5.5.3). The client-clock time a frame's last fragment
/// arrived, less the host-clock time it was sent, is the clocks' constant
/// offset plus the frame's transit time; less the smallest such difference
/// of the last few seconds, it is the queuing delay.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct QueueDetector {
    /// When recent frames' last fragments were sent, on the host clock,
    /// oldest first. Key frames are left out: they are large by nature.
    sent: VecDeque<(StreamId, FrameNumber, Timestamp)>,
    /// The differences that may yet be the smallest of the window, with
    /// when they were measured, oldest and smallest first.
    smallest: VecDeque<(Instant, i64)>,
}

impl QueueDetector {
    /// Notes when a frame's last fragment was sent, on the host clock.
    pub(crate) fn frame_sent(
        &mut self,
        stream_id: StreamId,
        frame_number: FrameNumber,
        sent: Timestamp,
    ) {
        if self.sent.len() == KEPT_FRAMES {
            self.sent.pop_front();
        }
        self.sent.push_back((stream_id, frame_number, sent));
    }

    /// Returns the queuing delay a `FRAME_TIMING` record shows, measured
    /// at `now`, if its frame's sending time is known.
    pub(crate) fn frame_timing(
        &mut self,
        record: &FrameTimingRecord,
        now: Instant,
    ) -> Option<Duration> {
        let &(_, _, sent) = self.sent.iter().find(|(stream_id, frame_number, _)| {
            (*stream_id, *frame_number) == (record.stream_id, record.frame_number)
        })?;
        let arrived = i64::try_from(record.last_fragment_time.as_micros()).ok()?;
        let difference = arrived - i64::try_from(sent.as_micros()).ok()?;
        while self
            .smallest
            .back()
            .is_some_and(|&(_, smaller)| smaller >= difference)
        {
            self.smallest.pop_back();
        }
        self.smallest.push_back((now, difference));
        while self
            .smallest
            .front()
            .is_some_and(|&(at, _)| now.saturating_duration_since(at) > TRANSIT_WINDOW)
        {
            self.smallest.pop_front();
        }
        let &(_, minimum) = self.smallest.front()?;
        Some(Duration::from_micros(
            u64::try_from(difference - minimum).unwrap_or(0),
        ))
    }
}

/// Returns the share of `frame_interval` that `bytes` take to cross `path`
/// at the rate its congestion window allows, one window per round trip, or
/// `None` if the path's rate is not known yet.
fn transmission_share(bytes: usize, path: &PathStats, frame_interval: Duration) -> Option<f64> {
    if path.congestion_window == 0 || path.rtt.is_zero() || frame_interval.is_zero() {
        return None;
    }
    let rate = path.congestion_window as f64 / path.rtt.as_secs_f64();
    Some(bytes as f64 / rate / frame_interval.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::wire::VarInt;

    /// A frame interval of 60 frames per second.
    const INTERVAL: Duration = Duration::from_micros(16_667);

    /// Returns a path that carries `bytes_per_rtt` bytes every 10 ms.
    fn path(bytes_per_rtt: u64) -> PathStats {
        PathStats {
            rtt: Duration::from_millis(10),
            congestion_window: bytes_per_rtt,
            ..PathStats::default()
        }
    }

    /// Frames too large for the path, and skipped frames, lower the
    /// quality at once; small frames raise it slowly back to the best.
    #[test]
    fn follows_the_path() {
        let start = Instant::now();
        let mut rate = RateControl::new(100);
        // 100 kB per 10 ms: a 100 kB frame takes 10 ms, 60% of the interval.
        let path = path(100_000);
        rate.frame_sent(100_000, false, &path, INTERVAL, start);
        assert_eq!(rate.quantizer(), 100 + STEP_UP);
        rate.frame_skipped(start);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP);

        // Small frames raise it only once the lowering has settled.
        rate.frame_sent(1_000, false, &path, INTERVAL, start + RAISE_DELAY / 2);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP);
        let later = start + RAISE_DELAY;
        rate.frame_sent(1_000, false, &path, INTERVAL, later);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP - STEP_DOWN);
        rate.frame_sent(1_000, false, &path, INTERVAL, later);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP - STEP_DOWN);
        // Key frames never raise it.
        rate.frame_sent(1_000, true, &path, INTERVAL, later + RAISE_INTERVAL);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP - STEP_DOWN);

        let mut now = later;
        for _ in 0..100 {
            now += RAISE_INTERVAL;
            rate.frame_sent(1_000, false, &path, INTERVAL, now);
        }
        assert_eq!(rate.quantizer(), 100);
    }

    /// Returns the timing record of frame `number` of stream 1, whose last
    /// fragment arrived at `arrived` microseconds on the client clock.
    fn timing(number: u32, arrived: u32) -> FrameTimingRecord {
        let timestamp = |micros: u32| Timestamp(VarInt::from_u32(micros));
        FrameTimingRecord {
            stream_id: StreamId(VarInt::from_u32(1)),
            frame_number: FrameNumber(VarInt::from_u32(number)),
            first_fragment_time: timestamp(arrived),
            last_fragment_time: timestamp(arrived),
            decoded_time: timestamp(0),
            presented_time: timestamp(0),
        }
    }

    /// The queuing delay is each transit above the smallest of the window,
    /// whatever the clocks' offset; a queue above the limit for a few frames
    /// in a row lowers the quality.
    #[test]
    fn detects_queues() {
        let start = Instant::now();
        let mut queue = QueueDetector::default();
        let stream = StreamId(VarInt::from_u32(1));
        for number in 1..=4 {
            let sent = Timestamp(VarInt::from_u32(number * 16_000));
            queue.frame_sent(stream, FrameNumber(VarInt::from_u32(number)), sent);
        }
        // The client clock is 1 s ahead; transits of 5, 3, 3 and 9 ms.
        let offset = 1_000_000;
        assert_eq!(
            queue.frame_timing(&timing(1, offset + 16_000 + 5_000), start),
            Some(Duration::ZERO)
        );
        assert_eq!(
            queue.frame_timing(&timing(2, offset + 32_000 + 3_000), start),
            Some(Duration::ZERO)
        );
        assert_eq!(
            queue.frame_timing(&timing(3, offset + 48_000 + 3_000), start),
            Some(Duration::ZERO)
        );
        let delay = queue
            .frame_timing(&timing(4, offset + 64_000 + 9_000), start)
            .unwrap();
        assert_eq!(delay, Duration::from_millis(6));
        assert_eq!(queue.frame_timing(&timing(9, 0), start), None);
        // The smallest transit ages out of the window.
        queue.frame_sent(
            stream,
            FrameNumber(VarInt::from_u32(5)),
            Timestamp(VarInt::from_u32(80_000)),
        );
        let later = start + TRANSIT_WINDOW + Duration::from_millis(1);
        assert_eq!(
            queue.frame_timing(&timing(5, offset + 80_000 + 9_000), later),
            Some(Duration::ZERO)
        );

        let mut rate = RateControl::new(100);
        rate.queue_measured(delay, start);
        rate.queue_measured(QUEUE_LIMIT, start);
        for _ in 1..QUEUED_FRAMES {
            rate.queue_measured(delay, start);
        }
        assert_eq!(rate.quantizer(), 100);
        rate.queue_measured(delay, start);
        assert_eq!(rate.quantizer(), 100 + STEP_UP);
    }

    /// Without a rate estimate, frames change nothing, and the quantizer
    /// never passes its largest value.
    #[test]
    fn stays_in_range() {
        let start = Instant::now();
        let mut rate = RateControl::new(250);
        rate.frame_sent(10_000_000, false, &path(0), INTERVAL, start);
        assert_eq!(rate.quantizer(), 250);
        rate.frame_skipped(start);
        assert_eq!(rate.quantizer(), u8::MAX);
        rate.frame_skipped(start);
        assert_eq!(rate.quantizer(), u8::MAX);
    }
}
