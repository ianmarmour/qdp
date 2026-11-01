//! Congestion control: BBR, with a floor under its window on a short path,
//! such as a local network's.
//!
//! While a desktop changes little, the video stream sends only what its few
//! changes need, and BBR's bandwidth estimate, and its congestion window
//! with it, falls to that: on a local network to some 40 KB. A large frame
//! that follows, as when a game's scene changes, then leaves 40 KB a round
//! trip until BBR measures more, and the frames behind it wait in the
//! sender's queue, which showed as tens of milliseconds of latency. A short
//! path carries far more than that floor in a round trip, so the floor only
//! lets a large frame leave at once; on longer paths, which may well carry
//! less, BBR's window is kept as it is.

use std::any::Any;
use std::sync::Arc;
use std::time::{Duration, Instant};

use quinn::congestion::{BbrConfig, Controller, ControllerFactory, ControllerMetrics};
use quinn_proto::RttEstimator;

/// The least congestion window on a short path, in bytes: enough for a large
/// frame, such as a scene change's, to leave in one round trip.
const SHORT_PATH_WINDOW_FLOOR: u64 = 256 * 1024;

/// The longest minimum round trip of a path counted as short, as a local
/// network's.
const SHORT_PATH_RTT: Duration = Duration::from_millis(10);

/// Returns the congestion window for BBR's `window` on a path whose least
/// round trip so far is `min_rtt`, if one was measured.
fn floored_window(window: u64, min_rtt: Option<Duration>) -> u64 {
    match min_rtt {
        Some(rtt) if rtt <= SHORT_PATH_RTT => window.max(SHORT_PATH_WINDOW_FLOOR),
        _ => window,
    }
}

/// Builds [`FlooredBbr`] controllers.
#[derive(Debug, Default)]
pub(crate) struct FlooredBbrConfig {
    /// The configuration of the BBR controllers underneath.
    bbr: Arc<BbrConfig>,
}

impl ControllerFactory for FlooredBbrConfig {
    fn build(self: Arc<Self>, now: Instant, current_mtu: u16) -> Box<dyn Controller> {
        Box::new(FlooredBbr {
            bbr: Arc::clone(&self.bbr).build(now, current_mtu),
            min_rtt: None,
        })
    }
}

/// BBR, whose window is kept at least [`SHORT_PATH_WINDOW_FLOOR`] on a short
/// path.
struct FlooredBbr {
    /// The BBR controller.
    bbr: Box<dyn Controller>,
    /// The path's least round trip so far, once one was measured.
    min_rtt: Option<Duration>,
}

impl Controller for FlooredBbr {
    fn on_sent(&mut self, now: Instant, bytes: u64, last_packet_number: u64) {
        self.bbr.on_sent(now, bytes, last_packet_number);
    }

    fn on_ack(
        &mut self,
        now: Instant,
        sent: Instant,
        bytes: u64,
        app_limited: bool,
        rtt: &RttEstimator,
    ) {
        self.min_rtt = Some(rtt.min());
        self.bbr.on_ack(now, sent, bytes, app_limited, rtt);
    }

    fn on_end_acks(
        &mut self,
        now: Instant,
        in_flight: u64,
        app_limited: bool,
        largest_packet_num_acked: Option<u64>,
    ) {
        self.bbr
            .on_end_acks(now, in_flight, app_limited, largest_packet_num_acked);
    }

    fn on_congestion_event(
        &mut self,
        now: Instant,
        sent: Instant,
        is_persistent_congestion: bool,
        lost_bytes: u64,
    ) {
        self.bbr
            .on_congestion_event(now, sent, is_persistent_congestion, lost_bytes);
    }

    fn on_mtu_update(&mut self, new_mtu: u16) {
        self.bbr.on_mtu_update(new_mtu);
    }

    fn window(&self) -> u64 {
        floored_window(self.bbr.window(), self.min_rtt)
    }

    fn metrics(&self) -> ControllerMetrics {
        let mut metrics = self.bbr.metrics();
        metrics.congestion_window = self.window();
        metrics
    }

    fn clone_box(&self) -> Box<dyn Controller> {
        Box::new(Self {
            bbr: self.bbr.clone_box(),
            min_rtt: self.min_rtt,
        })
    }

    fn initial_window(&self) -> u64 {
        self.bbr.initial_window()
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A short path keeps at least the floor; a long one, or one not yet
    /// measured, keeps BBR's window, and a window above the floor stays.
    #[test]
    fn floors_windows_on_short_paths() {
        let small = 40 * 1024;
        let lan = Some(Duration::from_millis(3));
        let internet = Some(Duration::from_millis(45));
        assert_eq!(floored_window(small, lan), SHORT_PATH_WINDOW_FLOOR);
        assert_eq!(floored_window(small, internet), small);
        assert_eq!(floored_window(small, None), small);
        assert_eq!(floored_window(4 << 20, lan), 4 << 20);
    }
}
