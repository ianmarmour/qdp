//! The host's rate control (Section 5.5): video must not persistently exceed
//! the rate the congestion controller allows. Frames are skipped while the
//! network is behind; on top of that, the encoder's quantizer rises when
//! frames take too long to cross the path, are skipped, show a queue
//! building on the path, or many packets are lost, and falls back toward the
//! configured quality while frames cross it easily. Past the highest
//! quantizer, the frame rate is lowered instead.
//!
//! Packet loss counts because the congestion controller, BBR, does not
//! treat loss as congestion, and a wireless link can lose much of what is
//! sent long before the window shrinks. Each loss also costs a recovery
//! frame, so a stream that keeps sending more than the link carries loses
//! more and more.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use qdp::message::{FrameNumber, FrameTimingRecord, StreamId, Timestamp};
use qdp_quic::PathStats;
use tracing::{debug, info};

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

/// How long after the quality was last lowered, or a reason to lower it
/// last appeared, it may be raised again.
const RAISE_DELAY: Duration = Duration::from_millis(500);

/// The shortest time between two lowerings of the quality. The reasons to
/// lower it arrive for several frames before a lower quality can take
/// effect, and lower it once rather than once each.
const LOWER_INTERVAL: Duration = Duration::from_millis(250);

/// How far above the configured quantizer the quality may be lowered.
/// Beyond it the host skips frames rather than send pictures too degraded
/// to read.
const MAX_RISE: u8 = 60;

/// The shortest time between two raises of the quality.
const RAISE_INTERVAL: Duration = Duration::from_millis(100);

/// How long the packets counted for each loss rate are sent over: as long
/// as a lowering of the quality takes to show.
const LOSS_WINDOW: Duration = LOWER_INTERVAL;

/// The fewest packets a loss rate is measured over, so that a few losses
/// while little is sent do not count.
const MIN_LOSS_PACKETS: u64 = 50;

/// The share of packets lost above which the quality is lowered.
const LOSS_LIMIT: f64 = 0.05;

/// The most frame intervals between frames sent, when rate control has
/// lowered the frame rate as far as it goes: a quarter of the stream's.
const MAX_INTERVAL_MULTIPLE: u32 = 4;

/// How long after a reason to lower the quality last appeared the frame
/// rate may be raised again. Longer than for the quantizer, since a frame
/// rate that is raised too soon doubles the rate sent at once.
const FRAME_RATE_RAISE_DELAY: Duration = Duration::from_secs(2);

/// The encoder's quantizer, chosen from how frames cross the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RateControl {
    /// The configured quantizer: the best quality the host sends.
    best: u8,
    /// The quantizer of the next frame.
    quantizer: u8,
    /// When the quality was last lowered.
    lowered_at: Option<Instant>,
    /// When a reason to lower the quality last appeared, whether or not it
    /// was lowered.
    pressed_at: Option<Instant>,
    /// When the quality was last raised.
    raised_at: Option<Instant>,
    /// How many frames in a row showed a queue.
    queued_frames: u8,
    /// How many frame intervals pass between frames sent: 1 at the
    /// stream's frame rate, more when the frame rate is lowered.
    interval_multiple: u32,
    /// The packets sent and lost when the current loss window started, and
    /// when it started.
    loss_window: Option<LossWindow>,
}

/// The packet counts at the start of a loss window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LossWindow {
    /// When the window started.
    started: Instant,
    /// The packets sent by then.
    sent_packets: u64,
    /// The packets declared lost by then.
    lost_packets: u64,
}

impl RateControl {
    /// Starts at the configured quantizer, `best`.
    pub(crate) fn new(best: u8) -> Self {
        Self {
            best,
            quantizer: best,
            lowered_at: None,
            pressed_at: None,
            raised_at: None,
            queued_frames: 0,
            interval_multiple: 1,
            loss_window: None,
        }
    }

    /// Returns the quantizer of the next frame.
    pub(crate) fn quantizer(&self) -> u8 {
        self.quantizer
    }

    /// Returns the time between frames sent for a stream whose frames are
    /// `stream_interval` apart: longer while the frame rate is lowered.
    pub(crate) fn frame_interval(&self, stream_interval: Duration) -> Duration {
        stream_interval * self.interval_multiple
    }

    /// Notes the packets `path` has sent and lost by `now`: a loss window
    /// that ends with too many of its packets lost lowers the quality.
    fn path_measured(&mut self, path: &PathStats, now: Instant) {
        let start = LossWindow {
            started: now,
            sent_packets: path.sent_packets,
            lost_packets: path.lost_packets,
        };
        let Some(window) = self.loss_window else {
            self.loss_window = Some(start);
            return;
        };
        let sent = path.sent_packets.saturating_sub(window.sent_packets);
        if now.saturating_duration_since(window.started) < LOSS_WINDOW || sent < MIN_LOSS_PACKETS {
            return;
        }
        self.loss_window = Some(start);
        let lost = path.lost_packets.saturating_sub(window.lost_packets);
        if lost as f64 > sent as f64 * LOSS_LIMIT {
            debug!(lost, sent, "packets lost");
            self.lower(now);
        }
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
    /// interval after the last, and the packets the path has lost. Key
    /// frames are larger by nature, and their size neither lowers nor
    /// raises the quality.
    pub(crate) fn frame_sent(
        &mut self,
        bytes: usize,
        key_frame: bool,
        path: &PathStats,
        frame_interval: Duration,
        now: Instant,
    ) {
        self.path_measured(path, now);
        if key_frame {
            return;
        }
        let Some(share) = transmission_share(bytes, path, frame_interval) else {
            return;
        };
        if share > HIGH_SHARE {
            self.lower(now);
        } else if share < LOW_SHARE {
            self.raise(now);
        }
    }

    /// Lowers the quality, at most once every [`LOWER_INTERVAL`]: the
    /// quantizer up to [`MAX_RISE`] above the best, then the frame rate by
    /// half, down to a [`MAX_INTERVAL_MULTIPLE`]th.
    fn lower(&mut self, now: Instant) {
        self.pressed_at = Some(now);
        if !settled(self.lowered_at, LOWER_INTERVAL, now) {
            return;
        }
        let worst = self.best.saturating_add(MAX_RISE);
        if self.quantizer < worst {
            self.lowered_at = Some(now);
            self.quantizer = self.quantizer.saturating_add(STEP_UP).min(worst);
            debug!(quantizer = self.quantizer, "video quality lowered");
        } else if self.interval_multiple < MAX_INTERVAL_MULTIPLE {
            self.lowered_at = Some(now);
            self.interval_multiple *= 2;
            info!(
                share = format!("1/{}", self.interval_multiple),
                "video frame rate lowered"
            );
        }
    }

    /// Raises the quality toward the best, if there was no reason to lower
    /// it recently: the frame rate first, then the quantizer.
    fn raise(&mut self, now: Instant) {
        if !settled(self.lowered_at, RAISE_DELAY, now)
            || !settled(self.pressed_at, RAISE_DELAY, now)
            || !settled(self.raised_at, RAISE_INTERVAL, now)
        {
            return;
        }
        if self.interval_multiple > 1 {
            if settled(self.pressed_at, FRAME_RATE_RAISE_DELAY, now) {
                self.raised_at = Some(now);
                self.interval_multiple /= 2;
                info!(
                    share = format!("1/{}", self.interval_multiple),
                    "video frame rate raised"
                );
            }
            return;
        }
        if self.quantizer == self.best {
            return;
        }
        self.raised_at = Some(now);
        self.quantizer = self.quantizer.saturating_sub(STEP_DOWN).max(self.best);
        debug!(quantizer = self.quantizer, "video quality raised");
    }
}

/// Returns whether at least `wait` has passed since `since`, at `now`, or
/// nothing happened at all.
fn settled(since: Option<Instant>, wait: Duration, now: Instant) -> bool {
    since.is_none_or(|since| now.saturating_duration_since(since) >= wait)
}

/// Finds the queuing delay frames meet on the path from their `FRAME_TIMING`
/// records (Section 5.5.3). The client-clock time a frame's first fragment
/// arrived, less the host-clock time it was sent, is the clocks' constant
/// offset plus the fragment's transit time; less the smallest such
/// difference of the last few seconds, it is the queuing delay. The first
/// fragment is the one measured because the others wait behind the
/// frame's own earlier fragments as they are paced out, which a large frame
/// would otherwise show as a queue.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct QueueDetector {
    /// When recent frames' first fragments were sent, on the host clock,
    /// oldest first. Key frames are left out: they are large by nature.
    sent: VecDeque<(StreamId, FrameNumber, Timestamp)>,
    /// The differences that may yet be the smallest of the window, with
    /// when they were measured, oldest and smallest first.
    smallest: VecDeque<(Instant, i64)>,
}

impl QueueDetector {
    /// Notes when a frame's first fragment was sent, on the host clock.
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
        let arrived = i64::try_from(record.first_fragment_time.as_micros()).ok()?;
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
    /// quality, once for a burst of them; small frames raise it slowly back
    /// to the best, once there has been no reason to lower it for a while.
    #[test]
    fn follows_the_path() {
        let start = Instant::now();
        let mut rate = RateControl::new(100);
        // 100 kB per 10 ms: a 100 kB frame takes 10 ms, 60% of the interval.
        let path = path(100_000);
        rate.frame_sent(100_000, false, &path, INTERVAL, start);
        assert_eq!(rate.quantizer(), 100 + STEP_UP);
        // The frames that follow before the lower quality takes effect
        // lower it no further.
        rate.frame_skipped(start + INTERVAL);
        rate.frame_sent(100_000, false, &path, INTERVAL, start + 2 * INTERVAL);
        assert_eq!(rate.quantizer(), 100 + STEP_UP);
        let pressed = start + LOWER_INTERVAL;
        rate.frame_skipped(pressed);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP);
        // Key frames are large by nature, and change nothing.
        rate.frame_sent(1_000_000, true, &path, INTERVAL, pressed + LOWER_INTERVAL);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP);

        // Small frames raise it only once the reasons to lower it stopped.
        rate.frame_sent(1_000, false, &path, INTERVAL, pressed + RAISE_DELAY / 2);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP);
        let later = pressed + RAISE_DELAY;
        rate.frame_sent(1_000, false, &path, INTERVAL, later);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP - STEP_DOWN);
        rate.frame_sent(1_000, false, &path, INTERVAL, later);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP - STEP_DOWN);
        rate.frame_sent(1_000, true, &path, INTERVAL, later + RAISE_INTERVAL);
        assert_eq!(rate.quantizer(), 100 + 2 * STEP_UP - STEP_DOWN);

        let mut now = later;
        for _ in 0..100 {
            now += RAISE_INTERVAL;
            rate.frame_sent(1_000, false, &path, INTERVAL, now);
        }
        assert_eq!(rate.quantizer(), 100);
    }

    /// The quantizer rises no further than a fixed distance from the best,
    /// and past it the frame rate is halved, down to a quarter; the frame
    /// rate comes back first, and only once the reasons to lower the
    /// quality have long stopped.
    #[test]
    fn lowers_the_frame_rate_past_the_worst_quantizer() {
        let start = Instant::now();
        let mut rate = RateControl::new(100);
        let steps = u32::from(MAX_RISE / STEP_UP);
        for step in 0..steps {
            rate.frame_skipped(start + LOWER_INTERVAL * step);
        }
        assert_eq!(rate.quantizer(), 100 + MAX_RISE);
        assert_eq!(rate.frame_interval(INTERVAL), INTERVAL);
        rate.frame_skipped(start + LOWER_INTERVAL * steps);
        assert_eq!(rate.frame_interval(INTERVAL), INTERVAL * 2);
        let mut now = start + LOWER_INTERVAL * steps;
        for _ in 0..10 {
            now += LOWER_INTERVAL;
            rate.frame_skipped(now);
        }
        assert_eq!(rate.quantizer(), 100 + MAX_RISE);
        assert_eq!(
            rate.frame_interval(INTERVAL),
            INTERVAL * MAX_INTERVAL_MULTIPLE
        );

        // Small frames soon after the last reason leave the frame rate.
        let path = path(100_000);
        rate.frame_sent(1_000, false, &path, INTERVAL, now + RAISE_DELAY);
        assert_eq!(
            rate.frame_interval(INTERVAL),
            INTERVAL * MAX_INTERVAL_MULTIPLE
        );
        // Long after it, they raise the frame rate before the quantizer.
        now += FRAME_RATE_RAISE_DELAY;
        rate.frame_sent(1_000, false, &path, INTERVAL, now);
        assert_eq!(rate.frame_interval(INTERVAL), INTERVAL * 2);
        assert_eq!(rate.quantizer(), 100 + MAX_RISE);
        now += RAISE_INTERVAL;
        rate.frame_sent(1_000, false, &path, INTERVAL, now);
        assert_eq!(rate.frame_interval(INTERVAL), INTERVAL);
        now += RAISE_INTERVAL;
        rate.frame_sent(1_000, false, &path, INTERVAL, now);
        assert_eq!(rate.quantizer(), 100 + MAX_RISE - STEP_DOWN);
    }

    /// A loss window that ends with more than its share of packets lost
    /// lowers the quality, whether the frame sent is a key frame or not;
    /// light loss, and loss among too few packets, do not.
    #[test]
    fn lowers_on_packet_loss() {
        let start = Instant::now();
        let mut rate = RateControl::new(100);
        let path = |sent_packets, lost_packets| PathStats {
            sent_packets,
            lost_packets,
            ..path(100_000)
        };
        rate.frame_sent(1_000, false, &path(1_000, 0), INTERVAL, start);
        // 2% lost: the network is fine.
        let first = start + LOSS_WINDOW;
        rate.frame_sent(1_000, false, &path(2_000, 20), INTERVAL, first);
        assert_eq!(rate.quantizer(), 100);
        // A window that has not ended yet is not judged.
        let early = first + LOSS_WINDOW / 2;
        rate.frame_sent(1_000, true, &path(3_000, 500), INTERVAL, early);
        assert_eq!(rate.quantizer(), 100);
        // Once it ends, 40% lost lowers the quality, even on a key frame.
        let second = first + LOSS_WINDOW;
        rate.frame_sent(1_000, true, &path(3_000, 420), INTERVAL, second);
        assert_eq!(rate.quantizer(), 100 + STEP_UP);
        // Too few packets are not judged, however many of them are lost.
        let third = second + LOSS_WINDOW;
        rate.frame_sent(1_000, false, &path(3_010, 430), INTERVAL, third);
        assert_eq!(rate.quantizer(), 100 + STEP_UP);
    }

    /// Returns the timing record of frame `number` of stream 1, whose first
    /// fragment arrived at `arrived` microseconds on the client clock, and
    /// its last 10 ms later, as a large frame's does.
    fn timing(number: u32, arrived: u32) -> FrameTimingRecord {
        let timestamp = |micros: u32| Timestamp(VarInt::from_u32(micros));
        FrameTimingRecord {
            stream_id: StreamId(VarInt::from_u32(1)),
            frame_number: FrameNumber(VarInt::from_u32(number)),
            first_fragment_time: timestamp(arrived),
            last_fragment_time: timestamp(arrived + 10_000),
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
        rate.frame_skipped(start + LOWER_INTERVAL);
        assert_eq!(rate.quantizer(), u8::MAX);
    }
}
