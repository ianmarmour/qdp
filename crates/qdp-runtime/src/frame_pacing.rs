//! Pacing frames that are captured as the source delivers them, rather than
//! on a timer: a frame goes out as soon as it arrives, so it does not wait
//! up to a frame interval for the next tick, and the stream keeps to its
//! frame rate even when the source delivers faster.
//!
//! Credit for one frame accrues over each frame interval, and a frame that
//! arrives goes out once there is enough credit, spending one frame's
//! worth. A source whose frames arrive at the stream's rate is sent every
//! frame as it arrives; one that delivers faster is sent the newest frame
//! at the stream's rate on average. When nothing arrives, a frame goes out
//! anyway after a while, as a repeat.

use std::time::{Duration, Instant};

/// The credit a frame that arrived needs to go out: three quarters of a
/// frame, so that a frame arriving a little early is not held back for a
/// whole frame.
const ARRIVAL_CREDIT: f64 = 0.75;

/// The most credit kept: one frame, so that frames never go out in bursts.
const MAX_CREDIT: f64 = 1.0;

/// The least credit kept, so that frames sent out of turn, as recovery
/// frames are, delay later ones by no more than a frame.
const MIN_CREDIT: f64 = -1.0;

/// How many frame intervals after the last frame one goes out anyway, as a
/// repeat, if none arrived.
const REPEAT_INTERVALS: f64 = 1.5;

/// How far short of the credit needed a frame may be and still go out: the
/// rounding of the credit's sums.
const CREDIT_TOLERANCE: f64 = 1e-6;

/// What [`FramePacer::seen`] starts at: no count of arrivals, so that the
/// first frame goes out at once.
const NONE_SEEN: u64 = u64::MAX;

/// Paces frames captured as they arrive.
#[derive(Debug, Clone)]
pub(crate) struct FramePacer {
    /// The frames that may go out, in frames.
    credit: f64,
    /// When the credit was last brought up to date.
    updated_at: Instant,
    /// When the last frame went out.
    last_at: Instant,
    /// How many frames had arrived when the last frame went out.
    seen: u64,
}

impl FramePacer {
    /// Starts pacing at `now`, with the first frame due at once.
    pub(crate) fn new(now: Instant) -> Self {
        Self {
            credit: MAX_CREDIT,
            updated_at: now,
            last_at: now,
            seen: NONE_SEEN,
        }
    }

    /// Returns whether a frame should go out at `now`, the stream having a
    /// frame each `interval` and `arrivals` frames having arrived: one that
    /// arrived since the last frame and has credit, or a repeat after a
    /// while without one.
    pub(crate) fn due(&mut self, now: Instant, interval: Duration, arrivals: u64) -> bool {
        self.accrue(now, interval);
        let arrived = arrivals != self.seen;
        (arrived && self.credit + CREDIT_TOLERANCE >= ARRIVAL_CREDIT)
            || now >= self.repeat_at(interval)
    }

    /// Notes that a frame went out at `now`, or was passed over, with
    /// `arrivals` frames having arrived.
    pub(crate) fn taken(&mut self, now: Instant, arrivals: u64) {
        self.credit = (self.credit - 1.0).max(MIN_CREDIT);
        self.last_at = now;
        self.seen = arrivals;
    }

    /// Returns when the next frame is due, the stream having a frame each
    /// `interval` and `arrivals` frames having arrived, unless another
    /// arrives first.
    pub(crate) fn next_due(&self, interval: Duration, arrivals: u64) -> Instant {
        let repeat = self.repeat_at(interval);
        if arrivals == self.seen {
            return repeat;
        }
        let needed = (ARRIVAL_CREDIT - self.credit).max(0.0);
        (self.updated_at + interval.mul_f64(needed)).min(repeat)
    }

    /// Adds the credit accrued up to `now`.
    fn accrue(&mut self, now: Instant, interval: Duration) {
        let elapsed = now.saturating_duration_since(self.updated_at);
        if !interval.is_zero() {
            self.credit =
                (self.credit + elapsed.as_secs_f64() / interval.as_secs_f64()).min(MAX_CREDIT);
        }
        self.updated_at = self.updated_at.max(now);
    }

    /// Returns when a repeat goes out if no frame arrives.
    fn repeat_at(&self, interval: Duration) -> Instant {
        self.last_at + interval.mul_f64(REPEAT_INTERVALS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame at 120 fps.
    const INTERVAL: Duration = Duration::from_nanos(8_333_333);

    /// Runs a pacer over `length` from `start`, with frames arriving at
    /// `arrivals` (offsets from the start), checking it whenever a frame
    /// arrives and whenever it said a frame was next due, as the host does,
    /// and returns when frames went out and how old each was when it did.
    fn run(arrivals: &[Duration], length: Duration) -> Vec<(Duration, Duration)> {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        let mut sent = Vec::new();
        let mut count = 0;
        let mut newest: Option<Duration> = None;
        let mut next_arrival = arrivals.iter().copied().peekable();
        let mut now = Duration::ZERO;
        while now < length {
            while next_arrival.peek().is_some_and(|&arrival| arrival <= now) {
                newest = next_arrival.next();
                count += 1;
            }
            if pacer.due(start + now, INTERVAL, count) {
                let age = newest.map_or(Duration::ZERO, |arrived| now - arrived);
                sent.push((now, age));
                pacer.taken(start + now, count);
            }
            let due = pacer.next_due(INTERVAL, count).duration_since(start);
            let arrival = next_arrival.peek().copied().unwrap_or(length);
            now = due
                .max(now + Duration::from_micros(1))
                .min(arrival)
                .min(length);
        }
        sent
    }

    /// Returns frame times every `spacing`, each `jitter` early or late in
    /// turn, from the first spacing on.
    fn every(spacing: Duration, jitter: Duration, length: Duration) -> Vec<Duration> {
        (1..)
            .map(|index| {
                let base = spacing * index;
                if index % 2 == 0 {
                    base.saturating_sub(jitter)
                } else {
                    base + jitter
                }
            })
            .take_while(|&time| time < length)
            .collect()
    }

    /// Frames arriving at the stream's rate, even a little early or late,
    /// each go out as they arrive.
    #[test]
    fn sends_frames_as_they_arrive() {
        let length = Duration::from_secs(1);
        let arrivals = every(
            Duration::from_nanos(8_338_000),
            Duration::from_micros(600),
            length,
        );
        let sent = run(&arrivals, length);
        // The first frame goes out at once, then each arrival.
        assert_eq!(sent.len(), arrivals.len() + 1, "{sent:?}");
        assert!(sent.iter().all(|&(_, age)| age.is_zero()), "{sent:?}");
    }

    /// A source faster than the stream is sent at the stream's rate, its
    /// newest frame each time.
    #[test]
    fn keeps_to_the_frame_rate() {
        let length = Duration::from_secs(1);
        let arrivals = every(Duration::from_nanos(6_060_606), Duration::ZERO, length);
        let sent = run(&arrivals, length);
        assert!((118..=122).contains(&sent.len()), "{}", sent.len());
        assert!(sent.iter().all(|&(_, age)| age < INTERVAL), "{sent:?}");
    }

    /// Without new frames, repeats go out every one and a half frames.
    #[test]
    fn repeats_when_nothing_arrives() {
        let sent = run(&[], Duration::from_millis(100));
        let gaps: Vec<Duration> = sent.windows(2).map(|pair| pair[1].0 - pair[0].0).collect();
        assert!(!gaps.is_empty());
        assert!(
            gaps.iter()
                .all(|&gap| gap == INTERVAL.mul_f64(REPEAT_INTERVALS)),
            "{gaps:?}"
        );
    }

    /// A frame sent out of turn spends credit, and the next arrival waits
    /// for it.
    #[test]
    fn waits_for_credit() {
        let start = Instant::now();
        let mut pacer = FramePacer::new(start);
        assert!(pacer.due(start, INTERVAL, 1));
        pacer.taken(start, 1);
        // A frame half an interval later waits until three quarters.
        let early = start + INTERVAL / 2;
        assert!(!pacer.due(early, INTERVAL, 2));
        let due = pacer.next_due(INTERVAL, 2);
        assert_eq!(due, start + INTERVAL.mul_f64(ARRIVAL_CREDIT));
        assert!(pacer.due(due, INTERVAL, 2));
    }
}
