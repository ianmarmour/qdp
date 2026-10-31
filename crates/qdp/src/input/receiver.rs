//! The host's side of input: applying reports exactly once, in sequence order
//! (Section 8.7).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::message::{
    DeviceId, DisplayConfigId, Input, InputAck, InputSequence, Timestamp, WarpId,
};
use crate::wire::VarInt;

/// The shortest time the host holds later reports while waiting for a missing one
/// (Section 8.7).
const MIN_GAP_WAIT: Duration = Duration::from_millis(20);

/// The shortest interval between `INPUT_ACK` datagrams (Section 8.5).
const ACK_INTERVAL: Duration = Duration::from_millis(1);

/// A report ready to be applied, with the context it was sent in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputEvent {
    /// The report's sequence number.
    pub sequence: InputSequence,
    /// The device that produced it.
    pub device_id: DeviceId,
    /// When the report was produced, on the client's clock: the datagram's
    /// Timestamp minus the report's Time Offset.
    pub produced_at: Timestamp,
    /// The `DISPLAY_CONFIG` whose layout its positions use.
    pub display_config_id: DisplayConfigId,
    /// The latest `CURSOR_WARP` the client had processed when it was sent.
    pub warp_id: WarpId,
    /// The report data.
    pub data: Vec<u8>,
}

/// A report that arrived ahead of a missing one.
#[derive(Debug, Clone)]
enum Held {
    /// A report of an accepted device.
    Report(InputEvent),
    /// A report of a device the host has not accepted, which is skipped.
    Skip,
}

/// The host's input state for one session.
#[derive(Debug, Default)]
pub struct InputReceiver {
    /// The highest sequence number applied or skipped.
    applied: u64,
    /// The highest Oldest Retained received.
    oldest_retained: u64,
    /// Reports above `applied` that arrived out of order.
    held: BTreeMap<u64, Held>,
    /// When the host started waiting for the report after `applied`.
    gap_since: Option<Instant>,
    /// Whether an `INPUT` datagram has arrived since the last `INPUT_ACK`.
    ack_pending: bool,
    /// When the last `INPUT_ACK` was sent.
    ack_sent_at: Option<Instant>,
    /// When the last `INPUT` datagram arrived.
    last_input_at: Option<Instant>,
}

impl InputReceiver {
    /// Creates the input state of a session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the highest sequence number applied or skipped.
    pub fn applied(&self) -> InputSequence {
        InputSequence(VarInt::new(self.applied).unwrap_or(VarInt::MAX))
    }

    /// Returns when the last `INPUT` datagram arrived, so the host can return
    /// devices to rest after a second of silence (Section 8.7).
    pub fn last_input_at(&self) -> Option<Instant> {
        self.last_input_at
    }

    /// Processes an `INPUT` datagram and returns the reports that can now be
    /// applied, in sequence order. `is_accepted` says whether a device has been
    /// accepted and not detached; reports of other devices are skipped without
    /// being parsed.
    pub fn handle_input(
        &mut self,
        input: &Input<'_>,
        is_accepted: impl Fn(DeviceId) -> bool,
        smoothed_rtt: Duration,
        now: Instant,
    ) -> Vec<InputEvent> {
        self.last_input_at = Some(now);
        self.ack_pending = true;
        self.oldest_retained = self
            .oldest_retained
            .max(input.oldest_retained.0.into_inner());
        for report in &input.reports {
            let sequence = report.sequence.0.into_inner();
            if sequence <= self.applied || self.held.contains_key(&sequence) {
                continue;
            }
            let held = if is_accepted(report.device_id) {
                let produced_at = input
                    .timestamp
                    .as_micros()
                    .saturating_sub(report.time_offset.into_inner());
                Held::Report(InputEvent {
                    sequence: report.sequence,
                    device_id: report.device_id,
                    produced_at: Timestamp(VarInt::new(produced_at).unwrap_or(VarInt::ZERO)),
                    display_config_id: input.display_config_id,
                    warp_id: input.warp_id,
                    data: report.data.to_vec(),
                })
            } else {
                Held::Skip
            };
            self.held.insert(sequence, held);
        }
        self.drain(smoothed_rtt, now)
    }

    /// Returns when [`InputReceiver::handle_timeout`] or
    /// [`InputReceiver::poll_ack`] must next be called.
    pub fn poll_timeout(&self, smoothed_rtt: Duration) -> Option<Instant> {
        let gap = self.gap_since.map(|since| since + gap_wait(smoothed_rtt));
        let ack = self
            .ack_pending
            .then(|| {
                self.ack_sent_at
                    .map(|sent_at| sent_at + ACK_INTERVAL)
                    .or(self.last_input_at)
            })
            .flatten();
        gap.into_iter().chain(ack).min()
    }

    /// Skips a missing report the host has waited for long enough, returning the
    /// reports that can then be applied.
    pub fn handle_timeout(&mut self, smoothed_rtt: Duration, now: Instant) -> Vec<InputEvent> {
        self.drain(smoothed_rtt, now)
    }

    /// Returns the `INPUT_ACK` to send now, if one is owed and the rate limit
    /// allows it.
    pub fn poll_ack(&mut self, now: Instant) -> Option<InputAck> {
        let allowed = self
            .ack_sent_at
            .is_none_or(|sent_at| now >= sent_at + ACK_INTERVAL);
        if !(self.ack_pending && allowed) {
            return None;
        }
        self.ack_pending = false;
        self.ack_sent_at = Some(now);
        Some(InputAck {
            largest_sequence: self.applied(),
        })
    }

    /// Applies or skips held reports in order, skipping missing ones that will
    /// never arrive or have been waited for long enough (Section 8.7).
    fn drain(&mut self, smoothed_rtt: Duration, now: Instant) -> Vec<InputEvent> {
        let mut events = Vec::new();
        loop {
            let next = self.applied + 1;
            if let Some(held) = self.held.remove(&next) {
                self.applied = next;
                self.gap_since = None;
                if let Held::Report(event) = held {
                    events.push(event);
                }
                continue;
            }
            let Some((&first_held, _)) = self.held.first_key_value() else {
                self.gap_since = None;
                break;
            };
            // `next` is missing and later reports are waiting.
            let below_retained = next < self.oldest_retained;
            let waited_enough = self
                .gap_since
                .is_some_and(|since| now >= since + gap_wait(smoothed_rtt));
            if below_retained || waited_enough {
                let skip_to = if below_retained {
                    self.oldest_retained.min(first_held) - 1
                } else {
                    next
                };
                self.applied = skip_to.max(next);
                self.gap_since = None;
                continue;
            }
            self.gap_since.get_or_insert(now);
            break;
        }
        events
    }
}

/// Returns how long the host holds later reports for a missing one.
fn gap_wait(smoothed_rtt: Duration) -> Duration {
    smoothed_rtt.max(MIN_GAP_WAIT)
}

/// Returns the change of a running total since its previous value, modulo 2^32
/// (Section 8.4).
pub fn total_delta(previous: i32, current: i32) -> i32 {
    current.wrapping_sub(previous)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::InputReport;

    /// A smoothed RTT for the tests.
    const RTT: Duration = Duration::from_millis(10);

    /// The device the tests accept.
    const ACCEPTED: DeviceId = DeviceId(VarInt::from_u32(1));

    /// Returns an `INPUT` datagram with Oldest Retained `oldest` and reports with
    /// `sequences` of `device`.
    fn input(oldest: u32, device: DeviceId, sequences: &[u32]) -> Input<'static> {
        Input {
            timestamp: Timestamp(VarInt::from_u32(10_000)),
            oldest_retained: InputSequence(VarInt::from_u32(oldest)),
            display_config_id: DisplayConfigId(VarInt::from_u32(1)),
            warp_id: WarpId::NONE,
            reports: sequences
                .iter()
                .map(|&sequence| InputReport {
                    sequence: InputSequence(VarInt::from_u32(sequence)),
                    device_id: device,
                    time_offset: VarInt::from_u32(500),
                    data: &[0xaa],
                })
                .collect(),
        }
    }

    /// Returns the sequence numbers of `events`.
    fn sequences(events: &[InputEvent]) -> Vec<u64> {
        events
            .iter()
            .map(|event| event.sequence.0.into_inner())
            .collect()
    }

    /// Reports are applied once, in order, with their production time.
    #[test]
    fn applies_in_order_once() {
        let mut receiver = InputReceiver::new();
        let now = Instant::now();
        let events = receiver.handle_input(&input(1, ACCEPTED, &[1, 2]), |_| true, RTT, now);
        assert_eq!(sequences(&events), [1, 2]);
        assert_eq!(events[0].produced_at.as_micros(), 9_500);
        let repeated = receiver.handle_input(&input(1, ACCEPTED, &[1, 2, 3]), |_| true, RTT, now);
        assert_eq!(sequences(&repeated), [3]);
    }

    /// A missing report at or above Oldest Retained holds later ones until it
    /// arrives.
    #[test]
    fn holds_for_missing_report() {
        let mut receiver = InputReceiver::new();
        let now = Instant::now();
        assert!(
            receiver
                .handle_input(&input(1, ACCEPTED, &[2, 3]), |_| true, RTT, now)
                .is_empty()
        );
        let events = receiver.handle_input(&input(1, ACCEPTED, &[1]), |_| true, RTT, now);
        assert_eq!(sequences(&events), [1, 2, 3]);
    }

    /// A missing report below Oldest Retained is skipped at once.
    #[test]
    fn skips_below_oldest_retained() {
        let mut receiver = InputReceiver::new();
        let now = Instant::now();
        let events = receiver.handle_input(&input(3, ACCEPTED, &[3, 4]), |_| true, RTT, now);
        assert_eq!(sequences(&events), [3, 4]);
    }

    /// A missing report is skipped after max(20 ms, smoothed RTT).
    #[test]
    fn skips_after_waiting() {
        let mut receiver = InputReceiver::new();
        let now = Instant::now();
        receiver.handle_input(&input(1, ACCEPTED, &[2]), |_| true, RTT, now);
        let deadline = now + MIN_GAP_WAIT;
        assert!(receiver.poll_timeout(RTT).unwrap() <= deadline);
        assert!(
            receiver
                .handle_timeout(RTT, deadline - Duration::from_millis(1))
                .is_empty()
        );
        assert_eq!(sequences(&receiver.handle_timeout(RTT, deadline)), [2]);
    }

    /// Reports of devices that were not accepted are skipped in sequence.
    #[test]
    fn skips_unaccepted_devices() {
        let mut receiver = InputReceiver::new();
        let other = DeviceId(VarInt::from_u32(9));
        let now = Instant::now();
        receiver.handle_input(
            &input(1, other, &[1]),
            |device| device == ACCEPTED,
            RTT,
            now,
        );
        let events = receiver.handle_input(
            &input(1, ACCEPTED, &[2]),
            |device| device == ACCEPTED,
            RTT,
            now,
        );
        assert_eq!(sequences(&events), [2]);
        assert_eq!(receiver.applied().0.into_inner(), 2);
    }

    /// Every datagram is acknowledged, at most once per millisecond.
    #[test]
    fn acknowledgements() {
        let mut receiver = InputReceiver::new();
        let now = Instant::now();
        assert!(receiver.poll_ack(now).is_none());
        receiver.handle_input(&input(1, ACCEPTED, &[1]), |_| true, RTT, now);
        let ack = receiver.poll_ack(now).unwrap();
        assert_eq!(ack.largest_sequence.0.into_inner(), 1);
        receiver.handle_input(&input(1, ACCEPTED, &[1]), |_| true, RTT, now);
        assert!(receiver.poll_ack(now).is_none());
        assert!(receiver.poll_ack(now + ACK_INTERVAL).is_some());
    }

    /// Running totals wrap around modulo 2^32.
    #[test]
    fn running_totals_wrap() {
        assert_eq!(total_delta(10, 25), 15);
        assert_eq!(total_delta(i32::MAX, i32::MIN), 1);
        assert_eq!(total_delta(i32::MIN, i32::MAX), -1);
    }
}
