//! The client's side of input: retaining reports and sending them until the host
//! acknowledges them (Section 8.6).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use crate::message::{
    DatagramPayload, DeviceId, DisplayConfigId, Input, InputAck, InputReport, InputSequence, WarpId,
};
use crate::registry::DatagramType;
use crate::time::SessionClock;
use crate::wire::VarInt;

/// How long a report is retained without acknowledgement (Section 8.6).
pub const RETENTION: Duration = Duration::from_secs(1);

/// The shortest interval between repeats of an unacknowledged report
/// (Section 8.6).
const MIN_REPEAT_INTERVAL: Duration = Duration::from_millis(2);

/// The divisor of the smoothed RTT that gives the repeat interval (Section 8.6).
const REPEAT_RTT_DIVISOR: u32 = 4;

/// How often an `INPUT` datagram is sent while a device is away from rest
/// (Section 8.6).
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);

/// The largest encoding of the varint fields of an `INPUT` header and of one
/// report's Sequence, Device ID, Time Offset and Report Length, used to bound
/// datagram sizes.
const MAX_VARINT_LENGTH: usize = 8;

/// The number of varint fields in the `INPUT` header after the Datagram Type.
const HEADER_FIELDS: usize = 4;

/// Whether a report changes discrete state, which decides whether a later report
/// of the same device may supersede it (Section 8.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportKind {
    /// A text report, or one that changes a key, button, contact set, contact or
    /// pen flag, or gamepad button. Never superseded.
    Transition,
    /// A report that only moves positions, axes or totals.
    Motion,
}

/// Whether a device is at rest after a report (Section 8.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceRest {
    /// Every key, button, contact and control is at rest.
    AtRest,
    /// Something is held or away from rest; the client sends a heartbeat.
    Active,
}

/// The kind of report a device produces, which decides how expiry and
/// superseding treat it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceClass {
    /// A standard device other than text, whose expired reports are replaced by a
    /// fresh report of its current state.
    Standard,
    /// A text device, whose reports are actions rather than state.
    Text,
    /// A passthrough device, whose reports supersede earlier ones only with the
    /// same report ID byte.
    Passthrough,
}

/// A report waiting for acknowledgement.
#[derive(Debug, Clone)]
struct RetainedReport {
    /// The device that produced it.
    device_id: DeviceId,
    /// The report data.
    data: Vec<u8>,
    /// Whether later reports may supersede it.
    kind: ReportKind,
    /// When it was produced.
    produced_at: Instant,
    /// When it was last sent, if it has been.
    last_sent: Option<Instant>,
    /// The layout and warp its positions were computed with, which the
    /// datagram carrying it names (Section 8.3).
    labels: Labels,
}

/// The `DISPLAY_CONFIG` whose layout a report's positions use, and the
/// latest `CURSOR_WARP` processed when it was produced (Section 8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Labels {
    /// The Display Config ID.
    display_config_id: DisplayConfigId,
    /// The Warp ID.
    warp_id: WarpId,
}

/// The newest report of one device.
#[derive(Debug, Clone)]
struct DeviceState {
    /// The device's class.
    class: DeviceClass,
    /// The newest report's sequence number and data, if any.
    newest: Option<(u64, Vec<u8>)>,
    /// Whether the device is away from rest.
    rest: DeviceRest,
}

/// The client's input state for one session.
#[derive(Debug)]
pub struct InputSender {
    /// The client's session clock.
    clock: SessionClock,
    /// The next sequence number.
    next_sequence: u64,
    /// Oldest Retained: every report at or above it is retained.
    oldest_retained: u64,
    /// Reports at or above Oldest Retained, by sequence number.
    retained: BTreeMap<u64, RetainedReport>,
    /// The highest sequence number the host acknowledged.
    acknowledged: u64,
    /// The attached devices the host accepted.
    devices: HashMap<DeviceId, DeviceState>,
    /// The `DISPLAY_CONFIG` whose layout positions use.
    display_config_id: DisplayConfigId,
    /// The latest `CURSOR_WARP` processed.
    warp_id: WarpId,
    /// When an `INPUT` datagram was last sent.
    last_datagram_at: Option<Instant>,
}

impl InputSender {
    /// Creates the input state of a session.
    pub fn new(clock: SessionClock) -> Self {
        Self {
            clock,
            next_sequence: 1,
            oldest_retained: 1,
            retained: BTreeMap::new(),
            acknowledged: 0,
            devices: HashMap::new(),
            display_config_id: DisplayConfigId::default(),
            warp_id: WarpId::NONE,
            last_datagram_at: None,
        }
    }

    /// Registers a device the host accepted. Reports may be sent for it only after
    /// this (Section 8.2.4).
    pub fn add_device(&mut self, device_id: DeviceId, class: DeviceClass) {
        self.devices.insert(
            device_id,
            DeviceState {
                class,
                newest: None,
                rest: DeviceRest::AtRest,
            },
        );
    }

    /// Forgets a detached device and discards its retained reports
    /// (Section 8.2.2).
    pub fn remove_device(&mut self, device_id: DeviceId) {
        self.devices.remove(&device_id);
        self.retained
            .retain(|_, report| report.device_id != device_id);
        self.advance_oldest_retained(None);
    }

    /// Sets the `DISPLAY_CONFIG` whose layout new positions use.
    pub fn set_display_config(&mut self, display_config_id: DisplayConfigId) {
        self.display_config_id = display_config_id;
    }

    /// Records the latest `CURSOR_WARP` the client processed.
    pub fn set_warp(&mut self, warp_id: WarpId) {
        self.warp_id = self.warp_id.max(warp_id);
    }

    /// Returns the labels of reports produced now.
    fn labels(&self) -> Labels {
        Labels {
            display_config_id: self.display_config_id,
            warp_id: self.warp_id,
        }
    }

    /// Returns the highest sequence number the host acknowledged, or 0.
    pub fn acknowledged(&self) -> InputSequence {
        InputSequence(VarInt::new(self.acknowledged).unwrap_or(VarInt::MAX))
    }

    /// Returns Oldest Retained.
    pub fn oldest_retained(&self) -> InputSequence {
        InputSequence(VarInt::new(self.oldest_retained).unwrap_or(VarInt::MAX))
    }

    /// Adds a report of an accepted device and returns its sequence number, or
    /// `None` if the device has not been accepted.
    ///
    /// A motion report replaces the device's newest report if that one is also a
    /// motion report that has not been sent yet, so motion is merged rather than
    /// queued (Section 8.6).
    pub fn push(
        &mut self,
        device_id: DeviceId,
        data: Vec<u8>,
        kind: ReportKind,
        rest: DeviceRest,
        now: Instant,
    ) -> Option<InputSequence> {
        let labels = self.labels();
        let device = self.devices.get_mut(&device_id)?;
        device.rest = rest;
        let mergeable = device.newest.as_ref().and_then(|(sequence, _)| {
            let report = self.retained.get(sequence)?;
            (kind == ReportKind::Motion
                && report.kind == ReportKind::Motion
                && report.last_sent.is_none())
            .then_some(*sequence)
        });
        let sequence = match mergeable {
            Some(sequence) => {
                if let Some(report) = self.retained.get_mut(&sequence) {
                    report.data.clone_from(&data);
                    report.labels = labels;
                }
                sequence
            }
            None => {
                let sequence = self.next_sequence;
                self.next_sequence += 1;
                self.retained.insert(
                    sequence,
                    RetainedReport {
                        device_id,
                        data: data.clone(),
                        kind,
                        produced_at: now,
                        last_sent: None,
                        labels,
                    },
                );
                sequence
            }
        };
        device.newest = Some((sequence, data));
        self.advance_oldest_retained(None);
        VarInt::new(sequence).ok().map(InputSequence)
    }

    /// Processes an `INPUT_ACK`.
    pub fn handle_ack(&mut self, ack: &InputAck, now: Instant) {
        self.acknowledged = self.acknowledged.max(ack.largest_sequence.0.into_inner());
        self.advance_oldest_retained(Some(now));
    }

    /// Returns when [`InputSender::poll_transmit`] must next be called: when a
    /// retained report is due to be repeated or to expire, or a heartbeat is due.
    pub fn poll_timeout(&self, smoothed_rtt: Duration) -> Option<Instant> {
        let interval = repeat_interval(smoothed_rtt);
        let reports = self.retained.values().map(|report| match report.last_sent {
            Some(sent_at) => (sent_at + interval).min(report.produced_at + RETENTION),
            None => report.produced_at,
        });
        let heartbeat = self.needs_heartbeat().then(|| {
            self.last_datagram_at
                .map_or(self.clock.epoch(), |sent_at| sent_at + HEARTBEAT_INTERVAL)
        });
        reports.chain(heartbeat).min()
    }

    /// Returns the next `INPUT` datagram to send, of at most `max_datagram_size`
    /// bytes, or `None` if nothing is due. Call it repeatedly until it returns
    /// `None`: new reports are sent first, then due repeats, oldest first.
    pub fn poll_transmit(
        &mut self,
        smoothed_rtt: Duration,
        max_datagram_size: usize,
        now: Instant,
    ) -> Option<Vec<u8>> {
        self.advance_oldest_retained(Some(now));
        let interval = repeat_interval(smoothed_rtt);
        let is_due = |report: &RetainedReport| {
            report
                .last_sent
                .is_none_or(|sent_at| now >= sent_at + interval)
        };
        // New reports first, then reports due to be repeated, then any other
        // retained reports as filler, each group oldest first (Section 8.6).
        let new = self
            .retained
            .iter()
            .filter(|(_, report)| report.last_sent.is_none())
            .map(|(&sequence, _)| sequence);
        let due = self
            .retained
            .iter()
            .filter(|(_, report)| report.last_sent.is_some() && is_due(report))
            .map(|(&sequence, _)| sequence);
        let filler = self
            .retained
            .iter()
            .filter(|(_, report)| !is_due(report))
            .map(|(&sequence, _)| sequence);
        let anything_due = self.retained.values().any(is_due);
        let candidates: Vec<u64> = new.chain(due).chain(filler).collect();
        // One datagram names one layout and warp, so it carries only the
        // reports computed with the first candidate's; the next carries
        // the others.
        let labels = candidates
            .first()
            .map_or(self.labels(), |sequence| self.retained[sequence].labels);
        let candidates: Vec<u64> = candidates
            .into_iter()
            .filter(|sequence| self.retained[sequence].labels == labels)
            .collect();

        let header_budget = DatagramType::INPUT.0.encoded_len() + HEADER_FIELDS * MAX_VARINT_LENGTH;
        let mut budget = max_datagram_size.saturating_sub(header_budget);
        let mut chosen = Vec::new();
        for sequence in candidates {
            let size = self.retained[&sequence].data.len() + 4 * MAX_VARINT_LENGTH;
            if size > budget {
                break;
            }
            budget -= size;
            chosen.push(sequence);
        }
        let heartbeat_due = self.needs_heartbeat()
            && self
                .last_datagram_at
                .is_none_or(|sent_at| now >= sent_at + HEARTBEAT_INTERVAL);
        if !(anything_due || heartbeat_due) {
            return None;
        }
        chosen.sort_unstable();

        let timestamp = self.clock.timestamp(now);
        let mut reports: Vec<InputReport<'_>> = Vec::new();
        for &sequence in &chosen {
            let report = &self.retained[&sequence];
            reports.push(self.report(
                sequence,
                report.device_id,
                &report.data,
                report.produced_at,
                now,
            ));
        }
        if reports.is_empty() {
            // A heartbeat repeats the newest report of an active device.
            if let Some((device_id, (sequence, data))) = self
                .devices
                .iter()
                .filter(|(_, device)| device.rest == DeviceRest::Active)
                .find_map(|(id, device)| device.newest.as_ref().map(|newest| (*id, newest)))
            {
                reports.push(self.report(*sequence, device_id, data, now, now));
            }
        }
        let datagram = Input {
            timestamp,
            oldest_retained: self.oldest_retained(),
            display_config_id: labels.display_config_id,
            warp_id: labels.warp_id,
            reports,
        }
        .to_bytes();

        for sequence in chosen {
            if let Some(report) = self.retained.get_mut(&sequence) {
                report.last_sent = Some(now);
            }
        }
        self.last_datagram_at = Some(now);
        Some(datagram)
    }

    /// Builds one report entry of an `INPUT` datagram.
    fn report<'a>(
        &self,
        sequence: u64,
        device_id: DeviceId,
        data: &'a [u8],
        produced_at: Instant,
        now: Instant,
    ) -> InputReport<'a> {
        let offset = now.saturating_duration_since(produced_at).as_micros();
        InputReport {
            sequence: InputSequence(VarInt::new(sequence).unwrap_or(VarInt::MAX)),
            device_id,
            time_offset: VarInt::new(u64::try_from(offset).unwrap_or(u64::MAX))
                .unwrap_or(VarInt::MAX),
            data,
        }
    }

    /// Returns whether any device is away from rest.
    fn needs_heartbeat(&self) -> bool {
        self.devices
            .values()
            .any(|device| device.rest == DeviceRest::Active && device.newest.is_some())
    }

    /// Returns whether the report `sequence` may leave the retained set: it was
    /// acknowledged, is a second old at `now`, or is superseded.
    fn is_releasable(&self, sequence: u64, report: &RetainedReport, now: Option<Instant>) -> bool {
        let acknowledged = sequence <= self.acknowledged;
        let expired = now.is_some_and(|now| now >= report.produced_at + RETENTION);
        let superseded = report.kind == ReportKind::Motion
            && self.retained.range(sequence + 1..).any(|(_, later)| {
                later.device_id == report.device_id
                    && match self
                        .devices
                        .get(&report.device_id)
                        .map(|device| device.class)
                    {
                        Some(DeviceClass::Passthrough) => later.data.first() == report.data.first(),
                        _ => true,
                    }
            });
        acknowledged || expired || superseded
    }

    /// Advances Oldest Retained past releasable reports at the front, replacing
    /// expired standard reports with a fresh report of the device's state
    /// (Section 8.6).
    fn advance_oldest_retained(&mut self, now: Option<Instant>) {
        let mut refresh = Vec::new();
        while let Some((&sequence, report)) = self.retained.first_key_value() {
            if !self.is_releasable(sequence, report, now) {
                break;
            }
            let expired_unacknowledged = sequence > self.acknowledged
                && now.is_some_and(|now| now >= report.produced_at + RETENTION);
            if expired_unacknowledged && !refresh.contains(&report.device_id) {
                refresh.push(report.device_id);
            }
            self.retained.pop_first();
            self.oldest_retained = sequence + 1;
        }
        if self.retained.is_empty() {
            self.oldest_retained = self.next_sequence;
        }
        for device_id in refresh {
            let fresh = self.devices.get(&device_id).and_then(|device| {
                (device.class == DeviceClass::Standard)
                    .then(|| {
                        device
                            .newest
                            .as_ref()
                            .map(|(_, data)| (data.clone(), device.rest))
                    })
                    .flatten()
            });
            if let (Some((data, rest)), Some(now)) = (fresh, now) {
                self.push(device_id, data, ReportKind::Transition, rest, now);
            }
        }
    }
}

/// Returns how often an unacknowledged report is repeated.
fn repeat_interval(smoothed_rtt: Duration) -> Duration {
    (smoothed_rtt / REPEAT_RTT_DIVISOR).max(MIN_REPEAT_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Reader;

    /// A smoothed RTT for the tests.
    const RTT: Duration = Duration::from_millis(20);

    /// The datagram size for the tests.
    const DATAGRAM_SIZE: usize = 1200;

    /// Returns a device ID.
    fn device(id: u32) -> DeviceId {
        DeviceId(VarInt::from_u32(id))
    }

    /// Returns the sequence numbers of the reports in an `INPUT` datagram and its
    /// Oldest Retained.
    fn contents(datagram: &[u8]) -> (Vec<u64>, u64) {
        let input = Input::decode(&mut Reader::new(&datagram[1..])).unwrap();
        let sequences = input
            .reports
            .iter()
            .map(|report| report.sequence.0.into_inner())
            .collect();
        (sequences, input.oldest_retained.0.into_inner())
    }

    /// Returns a sender with one standard device.
    fn sender(now: Instant) -> InputSender {
        let mut sender = InputSender::new(SessionClock::new(now));
        sender.add_device(device(1), DeviceClass::Standard);
        sender
    }

    /// Reports are sent once, repeated each interval until acknowledged, and then
    /// released.
    #[test]
    fn repeat_until_acknowledged() {
        let now = Instant::now();
        let mut sender = sender(now);
        sender.push(
            device(1),
            vec![1],
            ReportKind::Transition,
            DeviceRest::Active,
            now,
        );
        let (sequences, oldest) = contents(&sender.poll_transmit(RTT, DATAGRAM_SIZE, now).unwrap());
        assert_eq!((sequences, oldest), (vec![1], 1));

        let repeat_at = now + repeat_interval(RTT);
        assert_eq!(sender.poll_timeout(RTT), Some(repeat_at));
        let (sequences, _) =
            contents(&sender.poll_transmit(RTT, DATAGRAM_SIZE, repeat_at).unwrap());
        assert_eq!(sequences, vec![1]);

        sender.handle_ack(
            &InputAck {
                largest_sequence: InputSequence(VarInt::from_u32(1)),
            },
            repeat_at,
        );
        assert_eq!(sender.oldest_retained().0.into_inner(), 2);
    }

    /// Each datagram names the layout and warp its reports were computed
    /// with: reports from before and after a new layout go in separate
    /// datagrams.
    #[test]
    fn labels_reports() {
        let now = Instant::now();
        let mut sender = sender(now);
        let push = |sender: &mut InputSender, data| {
            sender.push(
                device(1),
                vec![data],
                ReportKind::Transition,
                DeviceRest::Active,
                now,
            );
        };
        sender.set_display_config(DisplayConfigId(VarInt::from_u32(1)));
        push(&mut sender, 1);
        sender.set_display_config(DisplayConfigId(VarInt::from_u32(2)));
        sender.set_warp(WarpId(VarInt::from_u32(4)));
        push(&mut sender, 2);
        let labels = |datagram: &[u8]| {
            let input = Input::decode(&mut Reader::new(&datagram[1..])).unwrap();
            (
                input.display_config_id.0.into_inner(),
                input.warp_id.0.into_inner(),
                input.reports.len(),
            )
        };
        let first = sender.poll_transmit(RTT, DATAGRAM_SIZE, now).unwrap();
        assert_eq!(labels(&first), (1, 0, 1));
        let second = sender.poll_transmit(RTT, DATAGRAM_SIZE, now).unwrap();
        assert_eq!(labels(&second), (2, 4, 1));
        assert!(sender.poll_transmit(RTT, DATAGRAM_SIZE, now).is_none());
    }

    /// Unsent motion reports are merged, and sent motion reports are superseded
    /// by later ones, while transitions are kept.
    #[test]
    fn merging_and_superseding() {
        let now = Instant::now();
        let mut sender = sender(now);
        let first = sender.push(
            device(1),
            vec![1],
            ReportKind::Motion,
            DeviceRest::AtRest,
            now,
        );
        let merged = sender.push(
            device(1),
            vec![2],
            ReportKind::Motion,
            DeviceRest::AtRest,
            now,
        );
        assert_eq!(first, merged);
        sender.poll_transmit(RTT, DATAGRAM_SIZE, now).unwrap();

        sender.push(
            device(1),
            vec![3],
            ReportKind::Motion,
            DeviceRest::AtRest,
            now,
        );
        assert_eq!(sender.oldest_retained().0.into_inner(), 2);

        sender.push(
            device(1),
            vec![4],
            ReportKind::Transition,
            DeviceRest::Active,
            now,
        );
        sender.push(
            device(1),
            vec![5],
            ReportKind::Transition,
            DeviceRest::Active,
            now,
        );
        // Report 2 is superseded by the transition 3, which is not superseded.
        assert_eq!(sender.oldest_retained().0.into_inner(), 3);
    }

    /// Datagrams carry new reports and then retained ones, even before the
    /// retained ones are due, and nothing is sent when nothing is due.
    #[test]
    fn retained_reports_ride_along() {
        let now = Instant::now();
        let mut sender = sender(now);
        sender.push(
            device(1),
            vec![1],
            ReportKind::Transition,
            DeviceRest::Active,
            now,
        );
        sender.poll_transmit(RTT, DATAGRAM_SIZE, now).unwrap();
        let soon = now + Duration::from_micros(500);
        assert!(sender.poll_transmit(RTT, DATAGRAM_SIZE, soon).is_none());
        sender.push(
            device(1),
            vec![2],
            ReportKind::Transition,
            DeviceRest::Active,
            soon,
        );
        let (sequences, _) = contents(&sender.poll_transmit(RTT, DATAGRAM_SIZE, soon).unwrap());
        assert_eq!(sequences, vec![1, 2]);
        assert!(sender.poll_transmit(RTT, DATAGRAM_SIZE, soon).is_none());
    }

    /// Reports that do not fit one datagram are sent in the next, with retained
    /// reports filling the remaining space.
    #[test]
    fn datagram_size_limit() {
        let now = Instant::now();
        let mut sender = sender(now);
        for _ in 0..3 {
            sender.push(
                device(1),
                vec![0; 500],
                ReportKind::Transition,
                DeviceRest::Active,
                now,
            );
        }
        let (first, _) = contents(&sender.poll_transmit(RTT, DATAGRAM_SIZE, now).unwrap());
        let (second, _) = contents(&sender.poll_transmit(RTT, DATAGRAM_SIZE, now).unwrap());
        assert_eq!(first, vec![1, 2]);
        // The new report comes first, and the oldest retained one fills the rest.
        assert_eq!(second, vec![1, 3]);
    }

    /// A report retained for a second is released and replaced by a fresh report
    /// of the device's state.
    #[test]
    fn expiry_refreshes_state() {
        let now = Instant::now();
        let mut sender = sender(now);
        sender.push(
            device(1),
            vec![9],
            ReportKind::Transition,
            DeviceRest::Active,
            now,
        );
        sender.poll_transmit(RTT, DATAGRAM_SIZE, now).unwrap();
        let expired = now + RETENTION;
        let (sequences, oldest) =
            contents(&sender.poll_transmit(RTT, DATAGRAM_SIZE, expired).unwrap());
        assert_eq!((sequences, oldest), (vec![2], 2));
    }

    /// While a device is active, a heartbeat is sent at least every 100 ms.
    #[test]
    fn heartbeat() {
        let now = Instant::now();
        let mut sender = sender(now);
        sender.push(
            device(1),
            vec![1],
            ReportKind::Transition,
            DeviceRest::Active,
            now,
        );
        sender.poll_transmit(RTT, DATAGRAM_SIZE, now).unwrap();
        sender.handle_ack(
            &InputAck {
                largest_sequence: InputSequence(VarInt::from_u32(1)),
            },
            now,
        );
        assert_eq!(sender.poll_timeout(RTT), Some(now + HEARTBEAT_INTERVAL));
        let beat = sender
            .poll_transmit(RTT, DATAGRAM_SIZE, now + HEARTBEAT_INTERVAL)
            .unwrap();
        assert_eq!(contents(&beat).0, vec![1]);

        sender.push(
            device(1),
            vec![0],
            ReportKind::Transition,
            DeviceRest::AtRest,
            now,
        );
        sender.handle_ack(
            &InputAck {
                largest_sequence: InputSequence(VarInt::from_u32(2)),
            },
            now,
        );
        assert_eq!(sender.poll_timeout(RTT), None);
    }

    /// Reports of devices that were not accepted are refused, and detaching a
    /// device discards its reports.
    #[test]
    fn devices() {
        let now = Instant::now();
        let mut sender = sender(now);
        assert_eq!(
            sender.push(
                device(2),
                vec![1],
                ReportKind::Transition,
                DeviceRest::AtRest,
                now
            ),
            None
        );
        sender.push(
            device(1),
            vec![1],
            ReportKind::Transition,
            DeviceRest::AtRest,
            now,
        );
        sender.remove_device(device(1));
        assert!(sender.poll_transmit(RTT, DATAGRAM_SIZE, now).is_none());
    }
}
