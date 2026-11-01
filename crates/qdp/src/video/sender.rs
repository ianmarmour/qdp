//! The host's side of one video stream: fragmentation, retransmission and loss
//! recovery (Sections 6.6 and 6.8).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::message::{
    DatagramPayload, FragmentFlags, FrameHeader, FrameNumber, FramePayload, LossRecord,
    MAX_FRAGMENTS_PER_FRAME, MAX_FRAME_PAYLOAD_LENGTH, NackRecord, StreamId, VideoFragment,
};
use crate::registry::DatagramType;
use crate::wire::VarInt;

/// The size of the Flags field of a fragment.
const FLAGS_LENGTH: usize = 1;

/// The extra time, beyond two smoothed RTTs, before the host repeats a recovery
/// frame for the same loss (Section 6.8.4).
const RECOVERY_REPEAT_MARGIN: Duration = Duration::from_millis(20);

/// The longest the host waits, after a recovery frame that failed, before
/// sending the next (Section 6.8.4).
const MAX_RECOVERY_WAIT: Duration = Duration::from_secs(1);

/// The extra time, beyond two smoothed RTTs, before the host repairs a possibly
/// lost final frame (Section 6.8.4).
const TAIL_LOSS_MARGIN: Duration = Duration::from_millis(10);

/// The shortest time the host keeps a frame's fragments for retransmission.
/// Spec Section 6.8.1 asks for at least two smoothed RTTs; keeping them longer
/// lets a large frame, such as a key frame, recover over several rounds of
/// `NACK`s instead of being declared lost and replaced by another large frame.
const MIN_RETENTION: Duration = Duration::from_millis(100);

/// The smoothed RTTs the host keeps a frame's fragments for, if longer than
/// [`MIN_RETENTION`].
const RETENTION_RTTS: u32 = 4;

/// How often a host that reports its datagram queue is asked to look at it
/// again while fragments wait in it: how precisely it knows when they were
/// sent.
const QUEUE_CHECK_INTERVAL: Duration = Duration::from_millis(1);

/// When a fragment counts as sent, which decides when it may be sent again
/// and when the newest frame's tail-loss timer starts (Sections 6.8.1 and
/// 6.8.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SentWhen {
    /// When the sender returns it, for callers that cannot see their queue.
    Returned,
    /// When it has left the host's datagram queue, as
    /// [`VideoSender::queue_drained`] reports. A fragment waiting behind the
    /// congestion window will still arrive: sending it again would only
    /// lengthen the queue that delays it.
    LeftQueue,
}

/// How far a host's datagram queue has drained, in bytes of datagrams
/// counted from the start of the connection.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SendQueue {
    /// The bytes of datagrams handed to the queue so far.
    pub offered: u64,
    /// The bytes of them that have left it, for the network or dropped.
    pub departed: u64,
}

/// The latest send of one fragment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FragmentSend {
    /// Waiting in the host's datagram queue since the time.
    Queued(Instant),
    /// Sent at the time.
    Sent(Instant),
}

/// What a frame is, which decides its fragment flags and its Recovery Base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    /// An ordinary frame that depends on the previous one.
    Delta,
    /// A key frame that does not end a loss state.
    Key,
    /// A key frame that ends a loss state, flagged `KEY_FRAME` and `RECOVERY`.
    KeyRecovery,
    /// A frame decodable after the client's Last Decoded Frame `base`, flagged
    /// `RECOVERY`.
    Recovery {
        /// The Last Decoded Frame of the `LOSS` this frame answers.
        base: FrameNumber,
    },
}

impl FrameKind {
    /// Returns the flags every fragment of the frame carries.
    fn flags(self) -> FragmentFlags {
        match self {
            Self::Delta => FragmentFlags::default(),
            Self::Key => FragmentFlags::KEY_FRAME,
            Self::KeyRecovery => FragmentFlags::KEY_FRAME.union(FragmentFlags::RECOVERY),
            Self::Recovery { .. } => FragmentFlags::RECOVERY,
        }
    }

    /// Returns whether the frame is a key frame.
    fn is_key(self) -> bool {
        matches!(self, Self::Key | Self::KeyRecovery)
    }
}

/// An error returned when a frame cannot be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SendFrameError {
    /// The frame payload is larger than 64 MiB.
    #[error("frame payload exceeds 64 MiB")]
    PayloadTooLarge,
    /// The frame needs more than 65535 fragments at this datagram size, or the
    /// datagram size cannot hold a fragment header.
    #[error("the frame does not fit in 65535 fragments of this datagram size")]
    TooManyFragments,
    /// The first frame of a stream must be a key frame.
    #[error("the first frame of a stream must be a key frame")]
    FirstFrameNotKey,
}

/// The fragments of one sent frame, kept for retransmission.
#[derive(Debug)]
struct SentFrame {
    /// The frame's number.
    frame_number: FrameNumber,
    /// The encoded datagrams, without the `RETRANSMISSION` flag.
    datagrams: Vec<Vec<u8>>,
    /// The offset of the Flags byte in every datagram of this frame.
    flags_offset: usize,
    /// Each fragment's latest send.
    sends: Vec<FragmentSend>,
    /// Where each fragment's latest send ends in the host's queue: the
    /// queue's offered bytes once it was handed to it.
    ends: Vec<u64>,
    /// Where each fragment's first send ended in the queue.
    first_ends: Vec<u64>,
    /// How many of the fragments still wait in the host's queue.
    queued: usize,
    /// When the frame was first sent whole, which its retention counts
    /// from.
    sent_at: Option<Instant>,
}

impl SentFrame {
    /// Records that fragment `index` is handed to the queue at `now`, as
    /// `sent_when` counts it.
    fn hand_over(
        &mut self,
        index: usize,
        sent_when: SentWhen,
        queue: &mut SendQueue,
        now: Instant,
    ) {
        queue.offered += self.datagrams[index].len() as u64;
        self.ends[index] = queue.offered;
        let send = match sent_when {
            SentWhen::Returned => FragmentSend::Sent(now),
            SentWhen::LeftQueue => FragmentSend::Queued(now),
        };
        if matches!(send, FragmentSend::Queued(_))
            && !matches!(self.sends[index], FragmentSend::Queued(_))
        {
            self.queued += 1;
        }
        self.sends[index] = send;
    }

    /// Returns fragment `index` as a retransmission, handed to the queue at
    /// `now`.
    fn retransmit(
        &mut self,
        index: usize,
        sent_when: SentWhen,
        queue: &mut SendQueue,
        now: Instant,
    ) -> Vec<u8> {
        let mut datagram = self.datagrams[index].clone();
        datagram[self.flags_offset] |= FragmentFlags::RETRANSMISSION.0;
        self.hand_over(index, sent_when, queue, now);
        datagram
    }

    /// Records the fragments that have left the queue by `departed` bytes,
    /// found at `now`, as sent as early as they can have been: when the
    /// queue was last looked at, `since`, or when they were queued, if
    /// later. A host too busy to look often must not count fragments that
    /// left long ago as just sent, or it would refuse the `NACK`s that ask
    /// for them while the client gives them up as lost.
    fn drained(&mut self, departed: u64, since: Instant, now: Instant) {
        if self.queued == 0 {
            return;
        }
        let mut latest = since;
        for (send, &end) in self.sends.iter_mut().zip(&self.ends) {
            if let FragmentSend::Queued(queued_at) = *send {
                if end <= departed {
                    let sent_at = since.max(queued_at).min(now);
                    latest = latest.max(sent_at);
                    *send = FragmentSend::Sent(sent_at);
                    self.queued -= 1;
                }
            }
        }
        if self.queued == 0 && self.sent_at.is_none() {
            self.sent_at = Some(latest);
        }
    }

    /// Returns whether fragment `index` was sent at least `interval` before
    /// `now`.
    fn sent_before(&self, index: usize, interval: Duration, now: Instant) -> bool {
        match self.sends[index] {
            FragmentSend::Sent(at) => now.saturating_duration_since(at) >= interval,
            FragmentSend::Queued(_) => false,
        }
    }

    /// Returns whether fragment `index` must be lost, the client holding
    /// fragment `held`: every copy of `held` left the queue after the latest
    /// of `index`, and the queue and the network keep their order.
    fn lost_before(&self, index: usize, held: usize) -> bool {
        self.ends[index] < self.first_ends[held]
    }
}

/// A recovery frame that has been sent, remembered so that it is not repeated
/// while it may still be arriving (Section 6.8.4).
#[derive(Debug, Clone, Copy)]
struct SentRecovery {
    /// The Last Decoded Frame the recovery frame answered.
    base: FrameNumber,
    /// The recovery frame's number.
    frame_number: FrameNumber,
    /// When its last fragment was sent.
    sent_at: Instant,
}

/// One frame split into datagrams, ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentFrameInfo {
    /// The number assigned to the frame.
    pub frame_number: FrameNumber,
    /// The datagrams, in fragment order.
    pub datagrams: Vec<Vec<u8>>,
}

/// The host's state for one video stream.
#[derive(Debug)]
pub struct VideoSender {
    /// The stream's identifier.
    stream_id: StreamId,
    /// The number of the next frame.
    next_frame_number: u64,
    /// Recently sent frames, oldest first.
    sent: VecDeque<SentFrame>,
    /// The highest frame the client reported decoded.
    last_decoded: FrameNumber,
    /// The Last Decoded Frame the next frame must recover from, if the client
    /// reported a loss or asked for a key frame.
    recovery_needed: Option<FrameNumber>,
    /// When the needed recovery frame may be sent, if earlier ones failed.
    recovery_not_before: Option<Instant>,
    /// The last recovery frame sent.
    last_recovery: Option<SentRecovery>,
    /// How many recovery frames in a row did not reach the client.
    recovery_failures: u32,
    /// When fragments count as sent.
    sent_when: SentWhen,
    /// How far the host's datagram queue had drained when last reported,
    /// with the fragments handed to it since counted as offered.
    queue: SendQueue,
    /// When the queue was last reported.
    queue_checked_at: Option<Instant>,
}

impl VideoSender {
    /// Creates the sender of a new stream, whose first frame is number 1 and
    /// whose fragments count as sent once they are returned.
    pub fn new(stream_id: StreamId) -> Self {
        Self::counting_sent(stream_id, SentWhen::Returned)
    }

    /// Creates the sender of a new stream whose fragments count as sent as
    /// `sent_when` says. With [`SentWhen::LeftQueue`], the caller reports
    /// its datagram queue with [`VideoSender::queue_drained`] before each
    /// call that returns datagrams, and queues those datagrams at once and
    /// in order.
    pub fn counting_sent(stream_id: StreamId, sent_when: SentWhen) -> Self {
        Self {
            stream_id,
            next_frame_number: FrameNumber::FIRST.0.into_inner(),
            sent: VecDeque::new(),
            last_decoded: FrameNumber::NONE,
            recovery_needed: None,
            recovery_not_before: None,
            last_recovery: None,
            recovery_failures: 0,
            sent_when,
            queue: SendQueue::default(),
            queue_checked_at: None,
        }
    }

    /// Notes how far the host's datagram queue has drained at `now`: the
    /// fragments that have left it since it was last reported count as sent
    /// from then, the earliest they can have left.
    pub fn queue_drained(&mut self, queue: SendQueue, now: Instant) {
        let since = self.queue_checked_at.unwrap_or(now);
        self.queue = queue;
        self.queue_checked_at = Some(now);
        for frame in &mut self.sent {
            frame.drained(queue.departed, since, now);
        }
    }

    /// Returns the stream's identifier.
    pub fn stream_id(&self) -> StreamId {
        self.stream_id
    }

    /// Returns the highest frame the client has reported decoded.
    pub fn last_decoded(&self) -> FrameNumber {
        self.last_decoded
    }

    /// Returns how many frames have been sent beyond the highest one the
    /// client has reported decoded.
    pub fn undecoded_frames(&self) -> u64 {
        (self.next_frame_number - 1).saturating_sub(self.last_decoded.0.into_inner())
    }

    /// Returns the Last Decoded Frame the next frame must recover from, if a
    /// recovery frame is due at `now`: [`FrameNumber::NONE`] means a key
    /// frame flagged `RECOVERY` is required; any other value allows a
    /// [`FrameKind::Recovery`] frame from that base.
    pub fn recovery_needed(&self, now: Instant) -> Option<FrameNumber> {
        self.recovery_needed.filter(|_| {
            self.recovery_not_before
                .is_none_or(|not_before| now >= not_before)
        })
    }

    /// Returns when the recovery frame the client waits for may be sent, if
    /// earlier ones failed and it must wait (Section 6.8.4). Until then the
    /// host sends no other frames, which the client cannot decode: they
    /// would only add to the traffic that lost the recovery frames.
    pub fn recovery_wait(&self) -> Option<Instant> {
        self.recovery_needed.and(self.recovery_not_before)
    }

    /// Splits a frame into fragments that each fit `max_datagram_size` bytes and
    /// returns them for sending.
    ///
    /// The header's Recovery Base is set from `kind`; the caller sets its other
    /// fields, including Base Frame Number on refinement streams.
    pub fn send_frame(
        &mut self,
        mut header: FrameHeader,
        temporal_unit: &[u8],
        kind: FrameKind,
        max_datagram_size: usize,
        now: Instant,
    ) -> Result<SentFrameInfo, SendFrameError> {
        if self.next_frame_number == FrameNumber::FIRST.0.into_inner() && !kind.is_key() {
            return Err(SendFrameError::FirstFrameNotKey);
        }
        header.recovery_base = match kind {
            FrameKind::Recovery { base } => Some(base),
            _ => None,
        };
        let payload = FramePayload {
            header,
            temporal_unit,
        }
        .to_bytes();
        if payload.len() > MAX_FRAME_PAYLOAD_LENGTH {
            return Err(SendFrameError::PayloadTooLarge);
        }

        let frame_number = FrameNumber(
            VarInt::new(self.next_frame_number).map_err(|_| SendFrameError::TooManyFragments)?,
        );
        let fixed_overhead = DatagramType::VIDEO_FRAGMENT.0.encoded_len()
            + self.stream_id.0.encoded_len()
            + frame_number.0.encoded_len()
            + FLAGS_LENGTH;
        let piece_size = choose_piece_size(payload.len(), max_datagram_size, fixed_overhead)?;
        let flags = kind.flags();
        let pieces: Vec<&[u8]> = payload.chunks(piece_size).collect();
        let last_index = pieces.len() - 1;
        let datagrams: Vec<Vec<u8>> = pieces
            .iter()
            .enumerate()
            .map(|(index, data)| {
                let fragment_flags = if index == last_index {
                    flags.union(FragmentFlags::LAST)
                } else {
                    flags
                };
                VideoFragment {
                    stream_id: self.stream_id,
                    frame_number,
                    fragment_index: VarInt::try_from(index)
                        .expect("fragment indices are below 65535"),
                    flags: fragment_flags,
                    data,
                }
                .to_bytes()
            })
            .collect();

        self.next_frame_number += 1;
        // Only a frame flagged RECOVERY answers a loss or a key frame request.
        if matches!(kind, FrameKind::KeyRecovery | FrameKind::Recovery { .. }) {
            if let Some(base) = self.recovery_needed.take() {
                self.recovery_not_before = None;
                self.last_recovery = Some(SentRecovery {
                    base,
                    frame_number,
                    sent_at: now,
                });
            }
        }
        let mut frame = SentFrame {
            frame_number,
            flags_offset: datagrams[0].len() - pieces[0].len() - FLAGS_LENGTH,
            sends: vec![FragmentSend::Sent(now); datagrams.len()],
            ends: vec![0; datagrams.len()],
            first_ends: Vec::new(),
            queued: 0,
            sent_at: None,
            datagrams: datagrams.clone(),
        };
        for index in 0..datagrams.len() {
            frame.hand_over(index, self.sent_when, &mut self.queue, now);
        }
        frame.first_ends = frame.ends.clone();
        frame.sent_at = (frame.queued == 0).then_some(now);
        self.sent.push_back(frame);
        Ok(SentFrameInfo {
            frame_number,
            datagrams,
        })
    }

    /// Processes a `DECODED` record, releasing the fragments of every frame up to
    /// the one decoded.
    pub fn handle_decoded(&mut self, last_decoded: FrameNumber) {
        self.last_decoded = self.last_decoded.max(last_decoded);
        if self
            .last_recovery
            .is_some_and(|recovery| self.last_decoded >= recovery.frame_number)
        {
            // The last recovery frame arrived, so a recovery frame still
            // needed is no longer held back.
            self.recovery_failures = 0;
            self.recovery_not_before = None;
        }
        while self.sent.len() > 1 && self.sent[0].frame_number <= self.last_decoded {
            self.sent.pop_front();
        }
    }

    /// Processes a `NACK` record and returns the fragments to retransmit.
    ///
    /// Fragments not yet sent, among them those still waiting in the host's
    /// queue, fragments sent less than one smoothed RTT ago, which may be on
    /// their way, and frames no longer held are skipped (Section 6.8.1). A
    /// fragment the client shows lost is sent again at once: one sent before
    /// a fragment the client holds, as the one after a Missing Range that
    /// ends inside the frame is.
    pub fn handle_nack(
        &mut self,
        nack: &NackRecord,
        smoothed_rtt: Duration,
        now: Instant,
    ) -> Vec<Vec<u8>> {
        let sent_when = self.sent_when;
        let queue = &mut self.queue;
        let Some(frame) = self
            .sent
            .iter_mut()
            .find(|frame| frame.frame_number == nack.frame_number)
        else {
            return Vec::new();
        };
        let fragment_count = frame.datagrams.len();
        let mut retransmissions = Vec::new();
        for range in &nack.missing {
            let first = usize::try_from(range.first_index.into_inner()).unwrap_or(usize::MAX);
            let (end, held) = match range.count.into_inner() {
                0 => (fragment_count, None),
                count => {
                    let end = usize::try_from(count)
                        .map_or(fragment_count, |count| first.saturating_add(count));
                    (end, Some(end).filter(|&end| end < fragment_count))
                }
            };
            for index in first..end.min(fragment_count) {
                let lost = held.is_some_and(|held| frame.lost_before(index, held));
                if lost || frame.sent_before(index, smoothed_rtt, now) {
                    retransmissions.push(frame.retransmit(index, sent_when, queue, now));
                }
            }
        }
        retransmissions
    }

    /// Processes a `LOSS` record, deciding whether a new recovery frame is needed
    /// (Section 6.8.4). A loss reported after a recovery frame the client has
    /// not decoded means that frame failed: each failure in a row doubles
    /// how long after it the next may be sent, up to a second.
    pub fn handle_loss(&mut self, loss: &LossRecord, smoothed_rtt: Duration, now: Instant) {
        if self.recovery_needed.is_some() {
            self.recovery_needed = Some(loss.last_decoded_frame);
            return;
        }
        let repeat_delay = 2 * smoothed_rtt + RECOVERY_REPEAT_MARGIN;
        if let Some(recovery) = self.last_recovery {
            let still_arriving = loss.highest_frame_seen < recovery.frame_number
                && now < recovery.sent_at + repeat_delay;
            if recovery.base == loss.last_decoded_frame && still_arriving {
                return;
            }
            let decoded = self.last_decoded.max(loss.last_decoded_frame);
            if decoded < recovery.frame_number {
                self.recovery_failures += 1;
                let doublings = (self.recovery_failures - 1).min(u32::BITS - 1);
                let wait = repeat_delay
                    .saturating_mul(1 << doublings)
                    .min(MAX_RECOVERY_WAIT);
                self.recovery_not_before = Some(recovery.sent_at + wait);
            } else {
                self.recovery_failures = 0;
            }
        }
        self.recovery_needed = Some(loss.last_decoded_frame);
    }

    /// Processes a `KEYFRAME_REQUEST`: the next frame must be a key frame flagged
    /// `RECOVERY` (Section 6.10).
    pub fn handle_keyframe_request(&mut self) {
        self.recovery_needed = Some(FrameNumber::NONE);
        self.recovery_not_before = None;
    }

    /// Returns when [`VideoSender::handle_timeout`] must next be called: to
    /// repair the possible loss of the newest frame, or, while fragments
    /// wait in the host's queue, to report the queue again.
    pub fn poll_timeout(&self, smoothed_rtt: Duration) -> Option<Instant> {
        let queue_check = self
            .queue_checked_at
            .filter(|_| self.sent.iter().any(|frame| frame.queued > 0))
            .map(|checked_at| checked_at + QUEUE_CHECK_INTERVAL);
        let tail = self.tail_deadline(smoothed_rtt);
        queue_check.into_iter().chain(tail).min()
    }

    /// Returns the newest frame's last fragment for retransmission if the
    /// client has not reported the frame decoded in time, as Section 6.8.4
    /// requires when the screen stays still; also discards frames past their
    /// retention time. The last fragment is enough: a client that lacks
    /// others learns from it how many the frame has, and asks for them.
    pub fn handle_timeout(&mut self, smoothed_rtt: Duration, now: Instant) -> Vec<Vec<u8>> {
        self.expire(now, smoothed_rtt);
        let due = self
            .tail_deadline(smoothed_rtt)
            .is_some_and(|deadline| now >= deadline);
        let sent_when = self.sent_when;
        let Some(newest) = self.sent.back_mut().filter(|_| due) else {
            return Vec::new();
        };
        let last = newest.datagrams.len() - 1;
        vec![newest.retransmit(last, sent_when, &mut self.queue, now)]
    }

    /// Returns when the newest frame needs repair if the client has not
    /// reported it decoded: 2 × smoothed RTT + 10 ms after its last fragment
    /// was last sent.
    fn tail_deadline(&self, smoothed_rtt: Duration) -> Option<Instant> {
        let newest = self.sent.back()?;
        let FragmentSend::Sent(sent_at) = *newest.sends.last()? else {
            return None;
        };
        (self.last_decoded < newest.frame_number)
            .then(|| sent_at + 2 * smoothed_rtt + TAIL_LOSS_MARGIN)
    }

    /// Discards frames whose fragments were all sent longer ago than the
    /// retention time, keeping the newest frame for tail-loss repair
    /// (Section 6.8.1).
    fn expire(&mut self, now: Instant, smoothed_rtt: Duration) {
        let retention = (RETENTION_RTTS * smoothed_rtt).max(MIN_RETENTION);
        while self.sent.len() > 1
            && self.sent[0]
                .sent_at
                .is_some_and(|sent_at| now.saturating_duration_since(sent_at) >= retention)
        {
            self.sent.pop_front();
        }
    }
}

/// Returns the fragment data size S for a payload of `payload_len` bytes, such
/// that every fragment, including the largest Fragment Index the frame needs, fits
/// `max_datagram_size` (Section 6.6.2).
fn choose_piece_size(
    payload_len: usize,
    max_datagram_size: usize,
    fixed_overhead: usize,
) -> Result<usize, SendFrameError> {
    for index_len in [1, 2, 4] {
        let Some(piece_size) = max_datagram_size
            .checked_sub(fixed_overhead + index_len)
            .filter(|&size| size > 0)
        else {
            continue;
        };
        let count = payload_len.div_ceil(piece_size).max(1);
        let largest_index =
            VarInt::try_from(count - 1).map_err(|_| SendFrameError::TooManyFragments)?;
        if count as u64 <= MAX_FRAGMENTS_PER_FRAME && largest_index.encoded_len() <= index_len {
            return Ok(piece_size);
        }
    }
    Err(SendFrameError::TooManyFragments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{BlockSet, MissingRange, Timestamp};
    use crate::wire::Reader;

    /// The datagram size used by the tests.
    const DATAGRAM_SIZE: usize = 1200;

    /// A smoothed RTT for the tests.
    const RTT: Duration = Duration::from_millis(10);

    /// Returns a motion frame header for a monitor of `blocks` blocks.
    fn header(blocks: usize) -> FrameHeader {
        FrameHeader {
            capture_timestamp: Timestamp::default(),
            recovery_base: None,
            base_frame_number: None,
            blocks: BlockSet::full(blocks),
        }
    }

    /// Decodes a fragment datagram.
    fn fragment(datagram: &[u8]) -> VideoFragment<'_> {
        let mut reader = Reader::new(&datagram[1..]);
        VideoFragment::decode(&mut reader).unwrap()
    }

    /// Returns a frame number.
    fn frame(number: u32) -> FrameNumber {
        FrameNumber(VarInt::from_u32(number))
    }

    /// Fragments have equal sizes, fit the datagram size and reassemble the
    /// payload, with `LAST` on the final one only.
    #[test]
    fn fragmentation() {
        let mut sender = VideoSender::new(StreamId(VarInt::from_u32(1)));
        let temporal_unit = vec![0xab; 10_000];
        let now = Instant::now();
        let sent = sender
            .send_frame(
                header(510),
                &temporal_unit,
                FrameKind::Key,
                DATAGRAM_SIZE,
                now,
            )
            .unwrap();
        assert_eq!(sent.frame_number, FrameNumber::FIRST);

        let fragments: Vec<_> = sent.datagrams.iter().map(|d| fragment(d)).collect();
        let piece = fragments[0].data.len();
        let mut payload = Vec::new();
        for (index, (fragment, datagram)) in fragments.iter().zip(&sent.datagrams).enumerate() {
            assert!(datagram.len() <= DATAGRAM_SIZE);
            assert_eq!(fragment.fragment_index.into_inner(), index as u64);
            assert!(fragment.flags.contains(FragmentFlags::KEY_FRAME));
            let is_last = index == fragments.len() - 1;
            assert_eq!(fragment.flags.contains(FragmentFlags::LAST), is_last);
            if !is_last {
                assert_eq!(fragment.data.len(), piece);
            }
            payload.extend_from_slice(fragment.data);
        }
        let decoded = FramePayload::decode(
            &payload,
            crate::message::VideoLayer::Motion,
            FragmentFlags::KEY_FRAME,
            crate::message::BlockGrid::for_monitor(1920, 1080),
        )
        .unwrap();
        assert_eq!(decoded.temporal_unit, temporal_unit);
    }

    /// The first frame must be a key frame and frame numbers increase by one.
    #[test]
    fn frame_numbering() {
        let mut sender = VideoSender::new(StreamId(VarInt::from_u32(1)));
        let now = Instant::now();
        assert_eq!(
            sender.send_frame(header(1), &[1], FrameKind::Delta, DATAGRAM_SIZE, now),
            Err(SendFrameError::FirstFrameNotKey)
        );
        for expected in 1..=3 {
            let kind = if expected == 1 {
                FrameKind::Key
            } else {
                FrameKind::Delta
            };
            let sent = sender
                .send_frame(header(1), &[1], kind, DATAGRAM_SIZE, now)
                .unwrap();
            assert_eq!(sent.frame_number, frame(expected));
        }
    }

    /// The piece size leaves room for the largest Fragment Index of the frame.
    #[test]
    fn piece_size_accounts_for_index_length() {
        assert_eq!(choose_piece_size(100, 100, 4), Ok(95));
        // 64 pieces of 95 bytes have indices up to 63, which fit one byte.
        assert_eq!(choose_piece_size(95 * 64, 100, 4), Ok(95));
        // 65 pieces need a two-byte index, so pieces shrink to 94.
        assert_eq!(choose_piece_size(95 * 65, 100, 4), Ok(94));
        assert_eq!(
            choose_piece_size(MAX_FRAME_PAYLOAD_LENGTH, 100, 4),
            Err(SendFrameError::TooManyFragments)
        );
        assert_eq!(
            choose_piece_size(10, 5, 4),
            Err(SendFrameError::TooManyFragments)
        );
    }

    /// NACKed fragments are retransmitted once per smoothed RTT with the
    /// `RETRANSMISSION` flag.
    #[test]
    fn retransmission() {
        let mut sender = VideoSender::new(StreamId(VarInt::from_u32(1)));
        let start = Instant::now();
        let sent = sender
            .send_frame(
                header(510),
                &vec![0; 5000],
                FrameKind::Key,
                DATAGRAM_SIZE,
                start,
            )
            .unwrap();
        let nack = NackRecord {
            stream_id: sender.stream_id(),
            frame_number: sent.frame_number,
            missing: vec![MissingRange {
                first_index: VarInt::from_u32(2),
                count: VarInt::ZERO,
            }],
        };
        assert!(sender.handle_nack(&nack, RTT, start).is_empty());

        let later = start + RTT;
        let retransmitted = sender.handle_nack(&nack, RTT, later);
        assert_eq!(retransmitted.len(), sent.datagrams.len() - 2);
        let first = fragment(&retransmitted[0]);
        assert_eq!(first.fragment_index.into_inner(), 2);
        assert!(first.flags.contains(FragmentFlags::RETRANSMISSION));
        assert_eq!(first.data, fragment(&sent.datagrams[2]).data);
        assert!(sender.handle_nack(&nack, RTT, later).is_empty());
    }

    /// A fragment the client shows lost, by holding one sent after it, is
    /// sent again at once, however recently it left; one it lacks without
    /// such evidence waits a smoothed RTT, and so does a repeat.
    #[test]
    fn evidence_of_loss() {
        let mut sender =
            VideoSender::counting_sent(StreamId(VarInt::from_u32(1)), SentWhen::LeftQueue);
        let start = Instant::now();
        sender.queue_drained(SendQueue::default(), start);
        let sent = sender
            .send_frame(
                header(510),
                &vec![0; 5000],
                FrameKind::Key,
                DATAGRAM_SIZE,
                start,
            )
            .unwrap();
        let offered: u64 = sent.datagrams.iter().map(|d| d.len() as u64).sum();
        let left = start + Duration::from_millis(1);
        sender.queue_drained(
            SendQueue {
                offered,
                departed: offered,
            },
            left,
        );
        let stream_id = sender.stream_id();
        let nack = |first: u32, count: u32| NackRecord {
            stream_id,
            frame_number: sent.frame_number,
            missing: vec![MissingRange {
                first_index: VarInt::from_u32(first),
                count: VarInt::from_u32(count),
            }],
        };
        // Fragment 1 is missing and fragment 2 arrived.
        let now = left + Duration::from_millis(1);
        let repeated = sender.handle_nack(&nack(1, 1), RTT, now);
        assert_eq!(repeated.len(), 1);
        // The repeat left after fragment 2, so fragment 2 shows nothing.
        assert!(sender.handle_nack(&nack(1, 1), RTT, now).is_empty());
        // The tail, which nothing follows, may be on its way.
        let tail = sent.datagrams.len() as u32 - 1;
        assert!(sender.handle_nack(&nack(tail, 1), RTT, now).is_empty());
        assert!(sender.handle_nack(&nack(tail, 0), RTT, now).is_empty());
        assert_eq!(sender.handle_nack(&nack(tail, 0), RTT, left + RTT).len(), 1);
    }

    /// A loss asks for recovery, which is not repeated while the recovery frame
    /// may still be arriving.
    #[test]
    fn loss_recovery() {
        let mut sender = VideoSender::new(StreamId(VarInt::from_u32(1)));
        let now = Instant::now();
        sender
            .send_frame(header(1), &[1], FrameKind::Key, DATAGRAM_SIZE, now)
            .unwrap();
        sender
            .send_frame(header(1), &[2], FrameKind::Delta, DATAGRAM_SIZE, now)
            .unwrap();
        let loss = LossRecord {
            stream_id: sender.stream_id(),
            last_decoded_frame: frame(1),
            highest_frame_seen: frame(2),
        };
        sender.handle_loss(&loss, RTT, now);
        assert_eq!(sender.recovery_needed(now), Some(frame(1)));
        assert_eq!(sender.recovery_wait(), None);

        let recovery = sender
            .send_frame(
                header(1),
                &[3],
                FrameKind::Recovery { base: frame(1) },
                DATAGRAM_SIZE,
                now,
            )
            .unwrap();
        assert_eq!(recovery.frame_number, frame(3));
        assert!(
            fragment(&recovery.datagrams[0])
                .flags
                .contains(FragmentFlags::RECOVERY)
        );
        assert_eq!(sender.recovery_needed(now), None);

        // The same loss reported again before frame 3 can have arrived is ignored.
        sender.handle_loss(&loss, RTT, now + RTT);
        assert_eq!(sender.recovery_needed(now + RTT), None);
        // Once the client has seen frame 3 and still reports the loss, it is
        // repeated, as soon as the first recovery frame could have arrived.
        let seen_recovery = LossRecord {
            highest_frame_seen: frame(3),
            ..loss
        };
        sender.handle_loss(&seen_recovery, RTT, now + RTT);
        let repeat_delay = 2 * RTT + RECOVERY_REPEAT_MARGIN;
        assert_eq!(sender.recovery_wait(), Some(now + repeat_delay));
        assert_eq!(sender.recovery_needed(now + RTT), None);
        assert_eq!(sender.recovery_needed(now + repeat_delay), Some(frame(1)));
    }

    /// Each recovery frame in a row that fails doubles the wait before the
    /// next, up to a limit, until the client decodes one.
    #[test]
    fn recovery_backs_off() {
        let mut sender = VideoSender::new(StreamId(VarInt::from_u32(1)));
        let start = Instant::now();
        sender
            .send_frame(header(1), &[1], FrameKind::Key, DATAGRAM_SIZE, start)
            .unwrap();
        let repeat_delay = 2 * RTT + RECOVERY_REPEAT_MARGIN;
        let mut now = start;
        let mut waits = Vec::new();
        // The client keeps reporting the loss after each recovery frame.
        let report = |sender: &mut VideoSender, now: Instant| {
            sender.handle_loss(
                &LossRecord {
                    stream_id: sender.stream_id(),
                    last_decoded_frame: frame(1),
                    highest_frame_seen: frame(sender.next_frame_number as u32),
                },
                RTT,
                now,
            );
        };
        report(&mut sender, now);
        for _ in 0..8 {
            let due = sender.recovery_wait().unwrap_or(now);
            waits.push(due.saturating_duration_since(now));
            now = due;
            assert!(sender.recovery_needed(now).is_some());
            sender
                .send_frame(header(1), &[2], FrameKind::KeyRecovery, DATAGRAM_SIZE, now)
                .unwrap();
            report(&mut sender, now);
        }
        assert_eq!(
            waits,
            [
                Duration::ZERO,
                repeat_delay,
                repeat_delay * 2,
                repeat_delay * 4,
                repeat_delay * 8,
                repeat_delay * 16,
                MAX_RECOVERY_WAIT,
                MAX_RECOVERY_WAIT,
            ]
        );

        // Once a recovery frame is decoded, nothing is held back, and the
        // next loss is answered at once.
        assert!(sender.recovery_wait().is_some());
        sender.handle_decoded(frame(sender.next_frame_number as u32 - 1));
        assert_eq!(sender.recovery_wait(), None);
        assert!(sender.recovery_needed(now).is_some());
        sender
            .send_frame(header(1), &[3], FrameKind::KeyRecovery, DATAGRAM_SIZE, now)
            .unwrap();
        sender.handle_decoded(frame(sender.next_frame_number as u32 - 1));
        sender.handle_loss(
            &LossRecord {
                stream_id: sender.stream_id(),
                last_decoded_frame: frame(sender.next_frame_number as u32 - 1),
                highest_frame_seen: frame(sender.next_frame_number as u32),
            },
            RTT,
            now,
        );
        assert_eq!(sender.recovery_wait(), None);
        assert!(sender.recovery_needed(now).is_some());
    }

    /// A key frame request requires a key frame.
    #[test]
    fn keyframe_request() {
        let mut sender = VideoSender::new(StreamId(VarInt::from_u32(1)));
        sender.handle_keyframe_request();
        assert_eq!(
            sender.recovery_needed(Instant::now()),
            Some(FrameNumber::NONE)
        );
    }

    /// The newest frame's last fragment is resent if the client does not
    /// report the frame decoded, and again after each resend.
    #[test]
    fn tail_loss_repair() {
        let mut sender = VideoSender::new(StreamId(VarInt::from_u32(1)));
        let start = Instant::now();
        let sent = sender
            .send_frame(
                header(510),
                &vec![0; 5000],
                FrameKind::Key,
                DATAGRAM_SIZE,
                start,
            )
            .unwrap();
        let deadline = sender.poll_timeout(RTT).unwrap();
        assert_eq!(deadline, start + 2 * RTT + TAIL_LOSS_MARGIN);
        assert!(
            sender
                .handle_timeout(RTT, deadline - Duration::from_millis(1))
                .is_empty()
        );
        let repaired = sender.handle_timeout(RTT, deadline);
        assert_eq!(repaired.len(), 1);
        let last = fragment(&repaired[0]);
        assert!(last.flags.contains(FragmentFlags::LAST));
        assert!(last.flags.contains(FragmentFlags::RETRANSMISSION));
        assert_eq!(
            last.fragment_index.into_inner() as usize,
            sent.datagrams.len() - 1
        );
        assert_eq!(
            sender.poll_timeout(RTT),
            Some(deadline + 2 * RTT + TAIL_LOSS_MARGIN)
        );

        sender.handle_decoded(sent.frame_number);
        assert_eq!(sender.poll_timeout(RTT), None);
    }

    /// Fragments waiting in the host's queue are not sent yet: a `NACK` does
    /// not repeat them, and the tail-loss timer starts once the last has
    /// left the queue, which the sender asks to be told of every
    /// millisecond while fragments wait.
    #[test]
    fn counts_fragments_sent_once_they_leave_the_queue() {
        let mut sender =
            VideoSender::counting_sent(StreamId(VarInt::from_u32(1)), SentWhen::LeftQueue);
        let start = Instant::now();
        // 1000 bytes of other datagrams were queued first, and 400 have left.
        sender.queue_drained(
            SendQueue {
                offered: 1000,
                departed: 400,
            },
            start,
        );
        let sent = sender
            .send_frame(
                header(510),
                &vec![0; 5000],
                FrameKind::Key,
                DATAGRAM_SIZE,
                start,
            )
            .unwrap();
        let sizes: Vec<u64> = sent.datagrams.iter().map(|d| d.len() as u64).collect();
        let offered = 1000 + sizes.iter().sum::<u64>();
        let nack = NackRecord {
            stream_id: sender.stream_id(),
            frame_number: sent.frame_number,
            missing: vec![MissingRange {
                first_index: VarInt::ZERO,
                count: VarInt::ZERO,
            }],
        };
        let queued = start + 2 * RTT;
        assert_eq!(sender.poll_timeout(RTT), Some(start + QUEUE_CHECK_INTERVAL));
        sender.queue_drained(
            SendQueue {
                offered,
                departed: 1000,
            },
            queued,
        );
        assert!(sender.handle_nack(&nack, RTT, queued).is_empty());
        assert!(sender.handle_timeout(RTT, queued).is_empty());

        // The first two fragments leave between two looks at the queue, a
        // millisecond apart; they count as sent at the first look, and only
        // they may be repeated, one smoothed RTT after that.
        let looked = queued + RTT;
        sender.queue_drained(
            SendQueue {
                offered,
                departed: 1000,
            },
            looked,
        );
        let left = looked + QUEUE_CHECK_INTERVAL;
        sender.queue_drained(
            SendQueue {
                offered,
                departed: 1000 + sizes[0] + sizes[1],
            },
            left,
        );
        assert!(sender.handle_nack(&nack, RTT, left).is_empty());
        let repeated = sender.handle_nack(&nack, RTT, looked + RTT);
        assert_eq!(repeated.len(), 2);
        assert_eq!(fragment(&repeated[1]).fragment_index.into_inner(), 1);

        // A host too busy to look at its queue for a while finds the rest
        // gone, and the repeats behind them: they count as sent when it
        // last looked, and the tail-loss timer starts then.
        let offered = offered + sizes[0] + sizes[1];
        let busy_until = looked + 5 * RTT;
        sender.queue_drained(
            SendQueue {
                offered,
                departed: offered,
            },
            busy_until,
        );
        assert_eq!(
            sender.poll_timeout(RTT),
            Some(left + 2 * RTT + TAIL_LOSS_MARGIN)
        );
        let probe = sender.handle_timeout(RTT, busy_until);
        assert_eq!(probe.len(), 1);
        // Every fragment but the probed one left at least a smoothed RTT
        // ago.
        assert_eq!(
            sender.handle_nack(&nack, RTT, busy_until).len(),
            sent.datagrams.len() - 1
        );
        // The probe waits in the queue in turn.
        assert!(sender.handle_timeout(RTT, busy_until + 10 * RTT).is_empty());
    }
}
