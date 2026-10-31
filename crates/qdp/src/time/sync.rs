//! Clock synchronization with `TIME_PING` and `TIME_PONG` (Section 5.2).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::message::{PingId, TimePing, TimePong, Timestamp};
use crate::time::SessionClock;
use crate::wire::VarInt;

/// The number of samples kept to choose the offset from (Section 5.2.2).
pub const SAMPLE_COUNT: usize = 8;

/// The ping interval during the first seconds of the `ACTIVE` phase
/// (Section 5.2.1).
const INITIAL_PING_INTERVAL: Duration = Duration::from_secs(1);

/// How long the initial ping interval applies after the session becomes active
/// (Section 5.2.1).
const INITIAL_PERIOD: Duration = Duration::from_secs(5);

/// The ping interval after the initial period (Section 5.2.1).
const STEADY_PING_INTERVAL: Duration = Duration::from_secs(10);

/// The ping interval until the first pong arrives: the clocks are compared
/// as soon as they can be, even if the first ping is lost. Pinging more
/// often than the intervals above is allowed (Section 5.2.1).
const UNANSWERED_PING_INTERVAL: Duration = Duration::from_millis(100);

/// One measurement of the peer's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSample {
    /// The peer's clock minus the local clock, in microseconds: peer clock ≈ local
    /// clock + offset.
    pub offset_micros: i64,
    /// The round-trip time excluding the peer's processing.
    pub delay: Duration,
    /// When the sample was taken.
    pub taken_at: Instant,
}

/// Measures the offset between the local session clock and the peer's.
///
/// The client must ping on the schedule of Section 5.2.1, which
/// [`ClockSync::poll_ping`] follows; the host may use the same type to ping too.
/// Every endpoint answers pings with [`answer_ping`].
#[derive(Debug)]
pub struct ClockSync {
    /// The local session clock.
    clock: SessionClock,
    /// When the session became active.
    active_since: Instant,
    /// The Ping ID of the next ping.
    next_ping_id: u64,
    /// Pings sent and not yet answered, as Ping ID and Origin Timestamp.
    outstanding: VecDeque<(PingId, Timestamp)>,
    /// The latest samples, newest last.
    samples: VecDeque<ClockSample>,
    /// When the last ping was sent.
    last_ping_at: Option<Instant>,
}

impl ClockSync {
    /// Creates the estimator for a session that became active at `active_since`.
    pub fn new(clock: SessionClock, active_since: Instant) -> Self {
        Self {
            clock,
            active_since,
            next_ping_id: 1,
            outstanding: VecDeque::new(),
            samples: VecDeque::new(),
            last_ping_at: None,
        }
    }

    /// Returns when the next ping is due.
    pub fn next_ping_at(&self) -> Instant {
        let Some(last) = self.last_ping_at else {
            return self.active_since;
        };
        let interval = if self.samples.is_empty() {
            UNANSWERED_PING_INTERVAL
        } else if last < self.active_since + INITIAL_PERIOD {
            INITIAL_PING_INTERVAL
        } else {
            STEADY_PING_INTERVAL
        };
        last + interval
    }

    /// Returns a `TIME_PING` to send if one is due at `now`.
    pub fn poll_ping(&mut self, now: Instant) -> Option<TimePing> {
        if now < self.next_ping_at() {
            return None;
        }
        let ping = TimePing {
            ping_id: PingId(VarInt::new(self.next_ping_id).unwrap_or(VarInt::MAX)),
            origin_timestamp: self.clock.timestamp(now),
        };
        self.next_ping_id += 1;
        self.last_ping_at = Some(now);
        if self.outstanding.len() == SAMPLE_COUNT {
            self.outstanding.pop_front();
        }
        self.outstanding
            .push_back((ping.ping_id, ping.origin_timestamp));
        Some(ping)
    }

    /// Processes a `TIME_PONG` that arrived at `now`, returning the new sample.
    /// Pongs for pings this endpoint did not send, or already answered, are
    /// ignored.
    pub fn handle_pong(&mut self, pong: &TimePong, now: Instant) -> Option<ClockSample> {
        let position = self
            .outstanding
            .iter()
            .position(|&(id, origin)| id == pong.ping_id && origin == pong.origin_timestamp)?;
        self.outstanding.remove(position);

        let t1 = i128::from(pong.origin_timestamp.as_micros());
        let t2 = i128::from(pong.receive_timestamp.as_micros());
        let t3 = i128::from(pong.transmit_timestamp.as_micros());
        let t4 = i128::from(self.clock.timestamp(now).as_micros());
        let offset = ((t2 - t1) + (t3 - t4)) / 2;
        let delay = ((t4 - t1) - (t3 - t2)).max(0);
        let sample = ClockSample {
            offset_micros: i64::try_from(offset).ok()?,
            delay: Duration::from_micros(u64::try_from(delay).ok()?),
            taken_at: now,
        };
        if self.samples.len() == SAMPLE_COUNT {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
        Some(sample)
    }

    /// Returns the sample with the smallest delay among the latest eight, whose
    /// offset is the best estimate (Section 5.2.2).
    pub fn best_sample(&self) -> Option<ClockSample> {
        self.samples
            .iter()
            .min_by_key(|sample| sample.delay)
            .copied()
    }

    /// Returns the estimated offset in microseconds: peer clock ≈ local clock +
    /// offset.
    pub fn offset_micros(&self) -> Option<i64> {
        self.best_sample().map(|sample| sample.offset_micros)
    }

    /// Maps a Timestamp on the peer's clock to the local instant it corresponds to,
    /// using the best offset.
    pub fn peer_to_local(&self, peer: Timestamp) -> Option<Instant> {
        let offset = self.offset_micros()?;
        let local_micros = i128::from(peer.as_micros()) - i128::from(offset);
        let local_micros = u64::try_from(local_micros.max(0)).ok()?;
        self.clock
            .epoch()
            .checked_add(Duration::from_micros(local_micros))
    }
}

/// Returns the `TIME_PONG` answering `ping`, which arrived at `received_at` and is
/// answered at `now`, both on the local session clock (Section 5.2.1).
pub fn answer_ping(
    ping: &TimePing,
    clock: &SessionClock,
    received_at: Instant,
    now: Instant,
) -> TimePong {
    TimePong {
        ping_id: ping.ping_id,
        origin_timestamp: ping.origin_timestamp,
        receive_timestamp: clock.timestamp(received_at),
        transmit_timestamp: clock.timestamp(now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The client's clock epoch is later than the host's by this much.
    const EPOCH_DIFFERENCE: Duration = Duration::from_millis(250);

    /// The one-way delay of the simulated path.
    const ONE_WAY: Duration = Duration::from_millis(5);

    /// A ping answered over a symmetric path measures the true offset and delay.
    #[test]
    fn symmetric_path() {
        let host_epoch = Instant::now();
        let client_epoch = host_epoch + EPOCH_DIFFERENCE;
        let host_clock = SessionClock::new(host_epoch);
        let mut sync = ClockSync::new(SessionClock::new(client_epoch), client_epoch);

        let ping = sync.poll_ping(client_epoch).unwrap();
        assert!(sync.poll_ping(client_epoch).is_none());
        let at_host = client_epoch + ONE_WAY;
        let pong = answer_ping(
            &ping,
            &host_clock,
            at_host,
            at_host + Duration::from_micros(50),
        );
        let sample = sync
            .handle_pong(&pong, at_host + Duration::from_micros(50) + ONE_WAY)
            .unwrap();
        assert_eq!(sample.offset_micros, EPOCH_DIFFERENCE.as_micros() as i64);
        assert_eq!(sample.delay, 2 * ONE_WAY);

        // A host Timestamp maps back to the matching client instant.
        let host_time = host_clock.timestamp(at_host);
        assert_eq!(sync.peer_to_local(host_time), Some(at_host));
    }

    /// Unknown and repeated pongs are ignored.
    #[test]
    fn ignores_unknown_pongs() {
        let epoch = Instant::now();
        let clock = SessionClock::new(epoch);
        let mut sync = ClockSync::new(clock, epoch);
        let ping = sync.poll_ping(epoch).unwrap();
        let pong = answer_ping(&ping, &clock, epoch, epoch);
        assert!(sync.handle_pong(&pong, epoch).is_some());
        assert!(sync.handle_pong(&pong, epoch).is_none());
        let forged = TimePong {
            ping_id: PingId(VarInt::from_u32(99)),
            ..pong
        };
        assert!(sync.handle_pong(&forged, epoch).is_none());
    }

    /// The offset comes from the sample with the smallest delay.
    #[test]
    fn best_sample_has_smallest_delay() {
        let epoch = Instant::now();
        let clock = SessionClock::new(epoch);
        let mut sync = ClockSync::new(clock, epoch);
        let mut now = epoch;
        for delay_ms in [9u64, 2, 7] {
            now = sync.next_ping_at().max(now);
            let ping = sync.poll_ping(now).unwrap();
            // An asymmetric path: all the delay is on the way back.
            let pong = answer_ping(&ping, &clock, now, now);
            now += Duration::from_millis(delay_ms);
            sync.handle_pong(&pong, now).unwrap();
        }
        assert_eq!(sync.best_sample().unwrap().delay, Duration::from_millis(2));
        assert_eq!(sync.offset_micros(), Some(-1000));
    }

    /// Unanswered pings are repeated every 100 ms; once one is answered,
    /// pings are sent every second for the first five seconds, then every
    /// ten.
    #[test]
    fn ping_schedule() {
        let epoch = Instant::now();
        let clock = SessionClock::new(epoch);
        let mut sync = ClockSync::new(clock, epoch);
        sync.poll_ping(epoch).unwrap();
        let retry_at = sync.next_ping_at();
        assert_eq!(retry_at, epoch + UNANSWERED_PING_INTERVAL);
        let retry = sync.poll_ping(retry_at).unwrap();
        let pong = answer_ping(&retry, &clock, retry_at, retry_at);
        assert!(sync.handle_pong(&pong, retry_at).is_some());
        let mut times = Vec::new();
        while times.len() < 7 {
            let now = sync.next_ping_at();
            sync.poll_ping(now).unwrap();
            times.push(now.duration_since(retry_at).as_secs());
        }
        assert_eq!(times, [1, 2, 3, 4, 5, 15, 25]);
    }
}
