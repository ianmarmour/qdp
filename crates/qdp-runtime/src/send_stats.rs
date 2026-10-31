//! The host's sending statistics, logged every second, and the rule that skips
//! frames when the network falls behind.
//!
//! A frame that cannot leave yet waits in the QUIC stack's datagram queue, and
//! every frame behind it waits longer. Skipping frames while the queue is long
//! trades smoothness for latency, which is the right trade for a remote
//! desktop: the next frame shows the current screen anyway.

use std::time::{Duration, Instant};

use qdp_quic::PathStats;
use tracing::{info, warn};

/// How often statistics are logged.
const STATS_INTERVAL: Duration = Duration::from_secs(1);

/// The bytes in a kilobyte, for logs.
const BYTES_PER_KILOBYTE: f64 = 1000.0;

/// The bits in a megabit, for logs.
const BITS_PER_MEGABIT: f64 = 1_000_000.0;

/// Returns how long the queued datagrams will take to leave at the rate the
/// congestion window allows: one window per round trip.
pub(crate) fn queue_delay(queued_bytes: usize, path: &PathStats) -> Duration {
    if queued_bytes == 0 {
        return Duration::ZERO;
    }
    if path.congestion_window == 0 {
        return Duration::MAX;
    }
    path.rtt
        .mul_f64(queued_bytes as f64 / path.congestion_window as f64)
}

/// Returns a duration in milliseconds with one decimal, for logs.
fn milliseconds(duration: Duration) -> String {
    format!("{:.1}", duration.as_secs_f64() * 1000.0)
}

/// What the host sent since statistics were last logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SendStats {
    /// When the interval started.
    started: Instant,
    /// The frames sent.
    frames: u64,
    /// The key frames among them.
    key_frames: u64,
    /// The frames skipped because the network was behind.
    skipped: u64,
    /// The bytes of encoded video sent, retransmissions excluded.
    bytes: u64,
    /// The fragments sent again.
    retransmissions: u64,
    /// The summed time from capture to encoded frame.
    encode_total: Duration,
    /// The longest time from capture to encoded frame.
    encode_max: Duration,
    /// The summed time spent capturing and scaling frames, a part of the
    /// time to encode them.
    scale_total: Duration,
    /// The path's statistics when the interval started.
    path_at_start: PathStats,
}

impl SendStats {
    /// Starts counting at `now`.
    pub(crate) fn new(now: Instant, path: PathStats) -> Self {
        Self {
            started: now,
            frames: 0,
            key_frames: 0,
            skipped: 0,
            bytes: 0,
            retransmissions: 0,
            encode_total: Duration::ZERO,
            encode_max: Duration::ZERO,
            scale_total: Duration::ZERO,
            path_at_start: path,
        }
    }

    /// Counts a frame of `bytes` that took `encode_time` to capture and
    /// encode, `scale_time` of it to capture and scale.
    pub(crate) fn frame_sent(
        &mut self,
        bytes: usize,
        is_key_frame: bool,
        encode_time: Duration,
        scale_time: Duration,
    ) {
        self.frames += 1;
        self.key_frames += u64::from(is_key_frame);
        self.bytes += bytes as u64;
        self.encode_total += encode_time;
        self.encode_max = self.encode_max.max(encode_time);
        self.scale_total += scale_time;
    }

    /// Counts a frame skipped because the network was behind.
    pub(crate) fn frame_skipped(&mut self) {
        self.skipped += 1;
    }

    /// Counts a retransmitted fragment.
    pub(crate) fn retransmitted(&mut self) {
        self.retransmissions += 1;
    }

    /// Returns when the statistics are next due.
    pub(crate) fn next_log_at(&self) -> Instant {
        self.started + STATS_INTERVAL
    }

    /// Logs the statistics and starts a new interval if this one has ended.
    /// `encoder_wait` is how long the encoder has been working on the frame
    /// it was last asked for, if any, and `quantizer` the one rate control
    /// chose.
    pub(crate) fn log_if_due(
        &mut self,
        now: Instant,
        path: PathStats,
        queued_bytes: usize,
        encoder_wait: Option<Duration>,
        quantizer: u8,
    ) {
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed < STATS_INTERVAL {
            return;
        }
        let seconds = elapsed.as_secs_f64();
        let frames = u32::try_from(self.frames).unwrap_or(u32::MAX);
        let encode_average = self.encode_total.checked_div(frames).unwrap_or_default();
        let scale_average = self.scale_total.checked_div(frames).unwrap_or_default();
        info!(
            fps = format!("{:.1}", self.frames as f64 / seconds),
            mbps = format!(
                "{:.2}",
                self.bytes as f64 * 8.0 / seconds / BITS_PER_MEGABIT
            ),
            key_frames = self.key_frames,
            skipped = self.skipped,
            quantizer,
            retransmissions = self.retransmissions,
            encode_ms = milliseconds(encode_average),
            encode_max_ms = milliseconds(self.encode_max),
            scale_ms = milliseconds(scale_average),
            rtt_ms = milliseconds(path.rtt),
            cwnd_kb = format!("{:.0}", path.congestion_window as f64 / BYTES_PER_KILOBYTE),
            queued_kb = format!("{:.1}", queued_bytes as f64 / BYTES_PER_KILOBYTE),
            lost_packets = path
                .lost_packets
                .saturating_sub(self.path_at_start.lost_packets),
            congestion_events = path
                .congestion_events
                .saturating_sub(self.path_at_start.congestion_events),
            "sending"
        );
        if let Some(wait) = encoder_wait.filter(|&wait| wait >= STATS_INTERVAL) {
            warn!(
                waiting_ms = milliseconds(wait),
                "the encoder has not returned a frame"
            );
        }
        *self = Self::new(now, path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A path with `window` bytes of congestion window and a 10 ms RTT.
    fn path(window: u64) -> PathStats {
        PathStats {
            rtt: Duration::from_millis(10),
            congestion_window: window,
            ..PathStats::default()
        }
    }

    /// The queue drains one congestion window per round trip.
    #[test]
    fn queue_delay_follows_the_window() {
        assert_eq!(queue_delay(0, &path(0)), Duration::ZERO);
        assert_eq!(
            queue_delay(30_000, &path(10_000)),
            Duration::from_millis(30)
        );
        assert_eq!(queue_delay(5_000, &path(10_000)), Duration::from_millis(5));
        assert_eq!(queue_delay(1, &path(0)), Duration::MAX);
    }

    /// Counts accumulate within an interval and reset when it is logged.
    #[test]
    fn counts_and_resets() {
        let start = Instant::now();
        let mut stats = SendStats::new(start, path(10_000));
        stats.frame_sent(
            1000,
            true,
            Duration::from_millis(4),
            Duration::from_millis(1),
        );
        stats.frame_sent(
            500,
            false,
            Duration::from_millis(2),
            Duration::from_millis(1),
        );
        stats.frame_skipped();
        stats.retransmitted();
        assert_eq!((stats.frames, stats.key_frames, stats.skipped), (2, 1, 1));
        assert_eq!(stats.bytes, 1500);
        assert_eq!(stats.encode_max, Duration::from_millis(4));
        assert_eq!(stats.scale_total, Duration::from_millis(2));
        stats.log_if_due(
            start + Duration::from_millis(500),
            path(10_000),
            0,
            None,
            100,
        );
        assert_eq!(stats.frames, 2);
        let later = start + STATS_INTERVAL;
        stats.log_if_due(later, path(10_000), 0, None, 100);
        assert_eq!(stats, SendStats::new(later, path(10_000)));
    }
}
