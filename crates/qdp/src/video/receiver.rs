//! The client's side of one video stream: reassembly, loss detection, decode
//! ordering and feedback (Sections 6.6.3, 6.7 and 6.8).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::message::{
    BlockGrid, DecodedRecord, FeedbackRecord, FragmentFlags, FrameHeader, FrameNumber,
    FramePayload, FrameTimingRecord, LossRecord, MAX_FRAGMENTS_PER_FRAME, MAX_FRAME_PAYLOAD_LENGTH,
    MissingRange, NackRecord, StreamId, Timestamp, VideoFragment, VideoLayer,
};
use crate::time::SessionClock;
use crate::wire::VarInt;

/// The number of incomplete frames kept per stream (Section 6.6.3).
pub const MAX_INCOMPLETE_FRAMES: usize = 16;

/// The most frames kept per stream, complete or not, while they wait for an
/// earlier, missing frame. Complete frames that wait only for the decoder are
/// limited by [`MAX_KEPT_BYTES`] alone.
pub const MAX_KEPT_FRAMES: usize = 128;

/// The most bytes of frame data kept per stream: room for the largest frame
/// and as much again.
pub const MAX_KEPT_BYTES: usize = 2 * MAX_FRAME_PAYLOAD_LENGTH;

/// The extra time, beyond one smoothed RTT, after a `NACK` before a frame without
/// progress is declared lost (Section 6.8.2).
const LOSS_MARGIN: Duration = Duration::from_millis(4);

/// The shortest time without a new fragment before a `NACK` is sent for an idle
/// frame (Section 6.8.1).
const MIN_IDLE_NACK_DELAY: Duration = Duration::from_millis(1);

/// The divisor of the smoothed RTT that gives the idle `NACK` delay
/// (Section 6.8.1).
const IDLE_NACK_RTT_DIVISOR: u32 = 8;

/// How often `DECODED` is sent while frames arrive (Section 6.7).
const DECODED_INTERVAL_ACTIVE: Duration = Duration::from_millis(100);

/// How often `DECODED` is sent while no frames arrive (Section 6.7).
const DECODED_INTERVAL_IDLE: Duration = Duration::from_secs(1);

/// How often feedback is sent in the `LOSS` and `KEY_ONLY` states
/// (Section 6.8.3).
const LOSS_FEEDBACK_INTERVAL: Duration = Duration::from_millis(20);

/// The decoding state of a stream (Section 6.8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReceiverState {
    /// Only a complete key frame can be decoded; Last Decoded Frame is 0.
    KeyOnly,
    /// Frames are decoded in order.
    Decoding,
    /// A frame was lost; a key frame or a recovery frame from Last Decoded Frame is
    /// needed.
    Loss,
}

/// A complete frame ready to be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodableFrame {
    /// The frame's number.
    pub frame_number: FrameNumber,
    /// The frame's `KEY_FRAME` and `RECOVERY` flags.
    pub flags: FragmentFlags,
    /// The frame header.
    pub header: FrameHeader,
    /// Whether the frame marks every block changed for composition: Frame Number 1
    /// and key frames that end a loss (Section 6.9).
    pub resets_composition: bool,
    /// The reassembled frame payload.
    payload: Vec<u8>,
    /// The offset of the temporal unit in `payload`.
    temporal_unit_offset: usize,
}

impl DecodableFrame {
    /// Returns the AV1 temporal unit to pass to the decoder.
    pub fn temporal_unit(&self) -> &[u8] {
        &self.payload[self.temporal_unit_offset..]
    }
}

/// The fragments received so far of one frame.
#[derive(Debug)]
struct PendingFrame {
    /// The frame's `KEY_FRAME` and `RECOVERY` flags, which every fragment
    /// repeats; `None` for a frame known only from a gap in the frame numbers
    /// received, until one of its fragments arrives.
    flags: Option<FragmentFlags>,
    /// The size S of every fragment but the last, once a non-last one has arrived.
    piece_size: Option<usize>,
    /// The index of the `LAST` fragment, once it has arrived.
    last_index: Option<u64>,
    /// The fragments received, by index.
    fragments: BTreeMap<u64, Vec<u8>>,
    /// The bytes of the fragments received, which the payload keeps once
    /// the frame is complete.
    bytes: usize,
    /// When the first fragment arrived.
    first_arrival: Instant,
    /// When the latest new fragment arrived.
    last_arrival: Instant,
    /// When a `NACK` was last sent for the frame.
    nacked_at: Option<Instant>,
    /// When the frame is declared lost if no new fragment arrives, once a `NACK`
    /// was sent.
    loss_deadline: Option<Instant>,
    /// The reassembled payload and parsed header, once complete.
    complete: Option<(FrameHeader, Vec<u8>, usize)>,
}

/// Why a `NACK` is sent, which decides which fragments it can name
/// (Section 6.8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NackReason {
    /// A fragment with a higher index arrived: only the gaps below it are known to
    /// be missing.
    Gap,
    /// A later frame arrived, or the frame went idle: the fragments after the
    /// highest one received may be missing too.
    Stalled,
}

/// How a fragment changed its frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Insertion {
    /// The fragment was new.
    Added,
    /// The fragment was a duplicate.
    Duplicate,
    /// The fragment contradicts the others; the frame is lost.
    Inconsistent,
}

impl PendingFrame {
    /// Creates the state of a frame from its first fragment's flags, or with no
    /// flags for a frame none of whose fragments has arrived.
    fn new(flags: Option<FragmentFlags>, now: Instant) -> Self {
        Self {
            flags,
            piece_size: None,
            last_index: None,
            fragments: BTreeMap::new(),
            bytes: 0,
            first_arrival: now,
            last_arrival: now,
            nacked_at: None,
            loss_deadline: None,
            complete: None,
        }
    }

    /// Adds a fragment, checking it against the rules of Section 6.6.3.
    fn insert(&mut self, fragment: &VideoFragment<'_>, frame_flags: FragmentFlags) -> Insertion {
        let index = fragment.fragment_index.into_inner();
        let is_last = fragment.flags.contains(FragmentFlags::LAST);
        if self.flags.is_some_and(|flags| flags != frame_flags)
            || index >= MAX_FRAGMENTS_PER_FRAME
            || fragment.data.is_empty()
            || self.last_index.is_some_and(|last| index > last)
            || (is_last && self.last_index.is_some_and(|last| last != index))
        {
            return Insertion::Inconsistent;
        }
        if self.fragments.contains_key(&index) {
            return Insertion::Duplicate;
        }
        let length = fragment.data.len();
        if is_last {
            if self.fragments.keys().any(|&other| other > index) {
                return Insertion::Inconsistent;
            }
            self.last_index = Some(index);
        } else {
            match self.piece_size {
                Some(size) if size != length => return Insertion::Inconsistent,
                Some(_) => {}
                None => self.piece_size = Some(length),
            }
        }
        if let (Some(size), Some(last)) = (self.piece_size, self.last_index) {
            let last_length = self
                .fragments
                .get(&last)
                .map_or(if is_last { length } else { 0 }, Vec::len);
            let total = size as u64 * last + last_length as u64;
            if last_length > size || total > MAX_FRAME_PAYLOAD_LENGTH as u64 {
                return Insertion::Inconsistent;
            }
        }
        self.flags = Some(frame_flags);
        self.bytes += length;
        self.fragments.insert(index, fragment.data.to_vec());
        Insertion::Added
    }

    /// Returns whether the frame is known to carry `flag`.
    fn has_flag(&self, flag: FragmentFlags) -> bool {
        self.flags.is_some_and(|flags| flags.contains(flag))
    }

    /// Returns whether every fragment up to the `LAST` one has arrived.
    fn is_complete(&self) -> bool {
        self.last_index
            .is_some_and(|last| self.fragments.len() as u64 == last + 1)
    }

    /// Returns the missing fragments. For [`NackReason::Stalled`] the list ends
    /// with an open range if the `LAST` fragment has not arrived.
    fn missing(&self, reason: NackReason) -> Vec<MissingRange> {
        let mut ranges = Vec::new();
        let mut expected = 0u64;
        let range = |first: u64, count: u64| MissingRange {
            first_index: VarInt::new(first).unwrap_or(VarInt::MAX),
            count: VarInt::new(count).unwrap_or(VarInt::MAX),
        };
        for &index in self.fragments.keys() {
            if index > expected {
                ranges.push(range(expected, index - expected));
            }
            expected = index + 1;
        }
        match self.last_index {
            Some(last) if expected <= last => ranges.push(range(expected, last + 1 - expected)),
            Some(_) => {}
            None if reason == NackReason::Stalled => ranges.push(range(expected, 0)),
            None => {}
        }
        ranges
    }

    /// Returns whether fragments are missing below the highest index received.
    fn has_gap(&self) -> bool {
        self.fragments
            .keys()
            .next_back()
            .is_some_and(|&highest| self.fragments.len() as u64 <= highest)
    }
}

/// The client's state for one video stream.
#[derive(Debug)]
pub struct VideoReceiver {
    /// The stream's identifier.
    stream_id: StreamId,
    /// Motion or refinement, which decides the frame header layout.
    layer: VideoLayer,
    /// The composition grid of the stream's monitor.
    grid: BlockGrid,
    /// The client's session clock, for feedback timestamps.
    clock: SessionClock,
    /// The decoding state.
    state: ReceiverState,
    /// The highest frame decoded, L.
    last_decoded: FrameNumber,
    /// The highest frame of which any fragment has arrived.
    highest_seen: FrameNumber,
    /// The frame handed to the decoder and not yet reported.
    decoding: Option<FrameNumber>,
    /// Frames above Last Decoded Frame, complete or not.
    frames: BTreeMap<FrameNumber, PendingFrame>,
    /// Records waiting to be sent in `VIDEO_FEEDBACK`.
    pending_records: Vec<FeedbackRecord>,
    /// Whether a frame was decoded since `DECODED` was last sent.
    decoded_changed: bool,
    /// When `DECODED` was last sent.
    decoded_sent_at: Option<Instant>,
    /// When `LOSS` was last sent.
    loss_sent_at: Option<Instant>,
    /// When the latest fragment of the stream arrived.
    last_fragment_at: Option<Instant>,
    /// Arrival and decode times of decoded frames awaiting presentation, for
    /// `FRAME_TIMING`.
    timings: BTreeMap<FrameNumber, FrameTimingRecord>,
}

impl VideoReceiver {
    /// Creates the receiver of a new stream of `layer` for a monitor divided into
    /// `grid`. The stream starts in the `KEY_ONLY` state.
    pub fn new(
        stream_id: StreamId,
        layer: VideoLayer,
        grid: BlockGrid,
        clock: SessionClock,
    ) -> Self {
        Self {
            stream_id,
            layer,
            grid,
            clock,
            state: ReceiverState::KeyOnly,
            last_decoded: FrameNumber::NONE,
            highest_seen: FrameNumber::NONE,
            decoding: None,
            frames: BTreeMap::new(),
            pending_records: Vec::new(),
            decoded_changed: false,
            decoded_sent_at: None,
            loss_sent_at: None,
            last_fragment_at: None,
            timings: BTreeMap::new(),
        }
    }

    /// Returns the decoding state.
    pub fn state(&self) -> ReceiverState {
        self.state
    }

    /// Returns the highest frame decoded.
    pub fn last_decoded(&self) -> FrameNumber {
        self.last_decoded
    }

    /// Processes a fragment of this stream.
    pub fn handle_fragment(
        &mut self,
        fragment: &VideoFragment<'_>,
        smoothed_rtt: Duration,
        now: Instant,
    ) {
        let frame_number = fragment.frame_number;
        if frame_number <= self.last_decoded || frame_number == FrameNumber::NONE {
            return;
        }
        self.last_fragment_at = Some(now);
        let is_new_frame = frame_number > self.highest_seen;
        if is_new_frame {
            self.add_skipped_frames(frame_number, now);
        }
        self.highest_seen = self.highest_seen.max(frame_number);

        let frame_flags = fragment.flags.intersection(FragmentFlags::FRAME_FLAGS);
        let frame = self
            .frames
            .entry(frame_number)
            .or_insert_with(|| PendingFrame::new(Some(frame_flags), now));
        if frame.complete.is_some() {
            return;
        }
        if frame.flags.is_none() {
            // The first fragment of a frame that was missing entirely.
            frame.first_arrival = now;
        }
        match frame.insert(fragment, frame_flags) {
            Insertion::Duplicate => return,
            Insertion::Inconsistent => {
                self.frames.remove(&frame_number);
                self.declare_lost();
                return;
            }
            Insertion::Added => {}
        }
        frame.last_arrival = now;
        if frame.loss_deadline.is_some() {
            frame.loss_deadline = Some(now + smoothed_rtt + LOSS_MARGIN);
        }
        if frame.is_complete() {
            self.complete_frame(frame_number, now);
        } else if frame.has_gap() {
            self.nack(frame_number, NackReason::Gap, smoothed_rtt, now);
        }
        if is_new_frame {
            // A fragment of a later frame is evidence that earlier incomplete frames
            // are missing fragments.
            let earlier: Vec<FrameNumber> = self
                .frames
                .range(..frame_number)
                .filter(|(_, frame)| frame.complete.is_none())
                .map(|(&number, _)| number)
                .collect();
            for number in earlier {
                self.nack(number, NackReason::Stalled, smoothed_rtt, now);
            }
        }
        self.enforce_memory_limit();
    }

    /// Returns the next frame to decode, if one is acceptable in the current
    /// state. Frames are handed out one at a time; the caller reports the outcome
    /// with [`VideoReceiver::frame_decoded`] or [`VideoReceiver::decode_failed`].
    pub fn poll_decodable(&mut self) -> Option<DecodableFrame> {
        if self.decoding.is_some() {
            return None;
        }
        let next_in_order = FrameNumber(VarInt::new(self.last_decoded.0.into_inner() + 1).ok()?);
        let in_order = (self.state == ReceiverState::Decoding)
            .then(|| self.frames.get(&next_in_order))
            .flatten()
            .filter(|frame| frame.complete.is_some())
            .map(|_| next_in_order);
        let jump = self
            .frames
            .iter()
            .rev()
            .find(|(_, frame)| self.is_jump_target(frame))
            .map(|(&number, _)| number);
        let frame_number = jump.max(in_order)?;

        let resets_composition = frame_number == FrameNumber::FIRST
            || (self.state != ReceiverState::Decoding
                && self.frames[&frame_number].has_flag(FragmentFlags::KEY_FRAME));
        // Frames below the chosen one are abandoned.
        let mut later = self.frames.split_off(&frame_number);
        self.frames = std::mem::take(&mut later);
        let frame = self.frames.remove(&frame_number)?;
        let (header, payload, temporal_unit_offset) = frame.complete?;
        self.decoding = Some(frame_number);
        Some(DecodableFrame {
            frame_number,
            flags: frame.flags.unwrap_or_default(),
            header,
            resets_composition,
            payload,
            temporal_unit_offset,
        })
    }

    /// Records that the decoder decoded `frame_number` at `now`.
    pub fn frame_decoded(&mut self, frame_number: FrameNumber, now: Instant) {
        if self.decoding != Some(frame_number) {
            return;
        }
        self.decoding = None;
        self.last_decoded = frame_number;
        self.state = ReceiverState::Decoding;
        self.decoded_changed = true;
        if let Some(timing) = self.timings.get_mut(&frame_number) {
            timing.decoded_time = self.clock.timestamp(now);
        }
        self.frames = self.frames.split_off(&FrameNumber(
            VarInt::new(frame_number.0.into_inner() + 1).unwrap_or(VarInt::MAX),
        ));
    }

    /// Records that the decoder failed or was reset: the stream enters the
    /// `KEY_ONLY` state, and the caller sends `KEYFRAME_REQUEST` (Section 6.10).
    pub fn decode_failed(&mut self) {
        self.decoding = None;
        self.state = ReceiverState::KeyOnly;
        self.last_decoded = FrameNumber::NONE;
    }

    /// Records when a decoded frame was presented, or `None` if a newer frame
    /// replaced it first, queuing its `FRAME_TIMING` record.
    pub fn frame_presented(&mut self, frame_number: FrameNumber, presented: Option<Instant>) {
        if let Some(mut timing) = self.timings.remove(&frame_number) {
            timing.presented_time =
                presented.map_or(Timestamp::default(), |at| self.clock.timestamp(at));
            self.pending_records
                .push(FeedbackRecord::FrameTiming(timing));
        }
        // Timings of older frames that were never reported are dropped.
        self.timings = self.timings.split_off(&frame_number);
    }

    /// Returns when [`VideoReceiver::handle_timeout`] must next be called.
    pub fn poll_timeout(&self, smoothed_rtt: Duration) -> Option<Instant> {
        let idle_delay = idle_nack_delay(smoothed_rtt);
        let frame_deadlines = self
            .frames
            .iter()
            .filter(|(_, frame)| frame.complete.is_none())
            .filter_map(|(&number, frame)| {
                // Frames that may not be NACKed in this state only wait for their
                // loss timer.
                let idle = self.may_nack(number).then(|| match frame.nacked_at {
                    Some(nacked_at) => {
                        nacked_at.max(frame.last_arrival) + smoothed_rtt.max(idle_delay)
                    }
                    None => frame.last_arrival + idle_delay,
                });
                idle.into_iter().chain(frame.loss_deadline).min()
            });
        let feedback = self.next_feedback_due();
        frame_deadlines.chain(feedback).min()
    }

    /// Sends `NACK`s for idle frames and declares frames lost whose deadline has
    /// passed.
    pub fn handle_timeout(&mut self, smoothed_rtt: Duration, now: Instant) {
        let idle_delay = idle_nack_delay(smoothed_rtt);
        let mut lost = Vec::new();
        let mut idle = Vec::new();
        for (&number, frame) in &self.frames {
            if frame.complete.is_some() {
                continue;
            }
            if frame.loss_deadline.is_some_and(|deadline| now >= deadline) {
                lost.push(number);
            } else if now >= frame.last_arrival + idle_delay && self.may_nack(number) {
                idle.push(number);
            }
        }
        for number in lost {
            self.frames.remove(&number);
            self.declare_lost();
        }
        for number in idle {
            self.nack(number, NackReason::Stalled, smoothed_rtt, now);
        }
    }

    /// Returns the feedback records to send now, if any. In the `LOSS` and
    /// `KEY_ONLY` states every non-empty result includes a `LOSS` record.
    pub fn poll_feedback(&mut self, now: Instant) -> Vec<FeedbackRecord> {
        let mut records = std::mem::take(&mut self.pending_records);
        if self.decoded_due_at().is_some_and(|due| now >= due) {
            records.push(FeedbackRecord::Decoded(DecodedRecord {
                stream_id: self.stream_id,
                last_decoded_frame: self.last_decoded,
            }));
            self.decoded_changed = false;
            self.decoded_sent_at = Some(now);
        }
        let in_loss = self.state != ReceiverState::Decoding;
        let loss_due = self
            .loss_sent_at
            .is_none_or(|sent_at| now >= sent_at + LOSS_FEEDBACK_INTERVAL);
        if in_loss && (loss_due || !records.is_empty()) {
            records.push(FeedbackRecord::Loss(LossRecord {
                stream_id: self.stream_id,
                last_decoded_frame: self.last_decoded,
                highest_frame_seen: self.highest_seen,
            }));
            self.loss_sent_at = Some(now);
        }
        records
    }

    /// Returns the stream's `LOSS` record while it is in the `LOSS` or
    /// `KEY_ONLY` state, which every `VIDEO_FEEDBACK` datagram must carry
    /// (Section 6.8.3, rule 3), without counting it as sent.
    pub fn loss_record(&self) -> Option<FeedbackRecord> {
        (self.state != ReceiverState::Decoding).then_some(FeedbackRecord::Loss(LossRecord {
            stream_id: self.stream_id,
            last_decoded_frame: self.last_decoded,
            highest_frame_seen: self.highest_seen,
        }))
    }

    /// Returns when the next periodic feedback is due.
    fn next_feedback_due(&self) -> Option<Instant> {
        let loss = (self.state != ReceiverState::Decoding).then(|| {
            self.loss_sent_at.map_or(
                self.last_fragment_at.unwrap_or(self.clock.epoch()),
                |sent_at| sent_at + LOSS_FEEDBACK_INTERVAL,
            )
        });
        loss.into_iter().chain(self.decoded_due_at()).min()
    }

    /// Returns when the next `DECODED` record is due, if the stream has decoded a
    /// frame: at once after a new frame is decoded, 100 ms after the last one if
    /// fragments have arrived since, and one second after it otherwise
    /// (Section 6.7).
    ///
    /// [`VideoReceiver::poll_feedback`] and [`VideoReceiver::poll_timeout`] both
    /// use this, so that a deadline that has passed is always acted on.
    fn decoded_due_at(&self) -> Option<Instant> {
        if self.last_decoded == FrameNumber::NONE {
            return None;
        }
        let Some(sent_at) = self.decoded_sent_at else {
            return Some(self.clock.epoch());
        };
        if self.decoded_changed {
            return Some(sent_at);
        }
        let arriving = self.last_fragment_at.is_some_and(|at| at > sent_at);
        let interval = if arriving {
            DECODED_INTERVAL_ACTIVE
        } else {
            DECODED_INTERVAL_IDLE
        };
        Some(sent_at + interval)
    }

    /// Returns whether `frame` can be decoded at once in the current state,
    /// abandoning lower frames (Section 6.8.3).
    fn is_jump_target(&self, frame: &PendingFrame) -> bool {
        let Some((header, _, _)) = &frame.complete else {
            return false;
        };
        if frame.has_flag(FragmentFlags::KEY_FRAME) {
            return true;
        }
        self.state != ReceiverState::KeyOnly
            && frame.has_flag(FragmentFlags::RECOVERY)
            && header.recovery_base == Some(self.last_decoded)
    }

    /// Returns whether `NACK`s may be sent for `frame_number` in the current
    /// state: in the `LOSS` and `KEY_ONLY` states, only for frames that may be
    /// acceptable and the frames after them (Section 6.8.3).
    fn may_nack(&self, frame_number: FrameNumber) -> bool {
        if self.state == ReceiverState::Decoding {
            return true;
        }
        let state = self.state;
        self.frames.range(..=frame_number).any(|(_, frame)| {
            frame.has_flag(FragmentFlags::KEY_FRAME)
                || (state == ReceiverState::Loss && frame.has_flag(FragmentFlags::RECOVERY))
        })
    }

    /// Queues a `NACK` for the missing fragments of `frame_number`, at most once
    /// per smoothed RTT, and starts its loss timer if this is its first `NACK`.
    fn nack(
        &mut self,
        frame_number: FrameNumber,
        reason: NackReason,
        smoothed_rtt: Duration,
        now: Instant,
    ) {
        if !self.may_nack(frame_number) {
            return;
        }
        let Some(frame) = self.frames.get_mut(&frame_number) else {
            return;
        };
        if frame
            .nacked_at
            .is_some_and(|nacked_at| now < nacked_at + smoothed_rtt)
        {
            return;
        }
        frame.nacked_at = Some(now);
        // Only progress restarts the loss timer: were each repeated NACK to
        // restart it, a frame whose fragments never come would wait forever.
        frame
            .loss_deadline
            .get_or_insert(now + smoothed_rtt + LOSS_MARGIN);
        let missing = frame.missing(reason);
        self.pending_records.push(FeedbackRecord::Nack(NackRecord {
            stream_id: self.stream_id,
            frame_number,
            missing,
        }));
    }

    /// Reassembles a complete frame and parses its header. A frame whose header
    /// does not parse is lost.
    fn complete_frame(&mut self, frame_number: FrameNumber, now: Instant) {
        let Some(frame) = self.frames.get_mut(&frame_number) else {
            return;
        };
        let payload: Vec<u8> = std::mem::take(&mut frame.fragments)
            .into_values()
            .flatten()
            .collect();
        let flags = frame.flags.unwrap_or_default();
        match FramePayload::decode(&payload, self.layer, flags, self.grid) {
            Ok(decoded) => {
                let header = decoded.header;
                let offset = payload.len() - decoded.temporal_unit.len();
                self.timings.insert(
                    frame_number,
                    FrameTimingRecord {
                        stream_id: self.stream_id,
                        frame_number,
                        first_fragment_time: self.clock.timestamp(frame.first_arrival),
                        last_fragment_time: self.clock.timestamp(now),
                        decoded_time: Timestamp::default(),
                        presented_time: Timestamp::default(),
                    },
                );
                frame.complete = Some((header, payload, offset));
                frame.loss_deadline = None;
            }
            Err(_) => {
                self.frames.remove(&frame_number);
                self.declare_lost();
            }
        }
    }

    /// Adds frames above the highest seen and below `frame_number`, none of
    /// whose fragments has arrived: they are missing entirely, and are `NACK`ed
    /// and declared lost like any other incomplete frame (Section 6.8.1).
    fn add_skipped_frames(&mut self, frame_number: FrameNumber, now: Instant) {
        let first = self.highest_seen.max(self.last_decoded).0.into_inner() + 1;
        let end = frame_number.0.into_inner();
        // Only the latest skipped frames are kept; older ones are lost outright.
        let kept_from = end.saturating_sub(MAX_INCOMPLETE_FRAMES as u64).max(first);
        if kept_from > first {
            self.declare_lost();
        }
        for number in kept_from..end {
            let Ok(number) = VarInt::new(number) else {
                break;
            };
            self.frames
                .entry(FrameNumber(number))
                .or_insert_with(|| PendingFrame::new(None, now));
        }
    }

    /// Enters the `LOSS` state after a frame was lost, unless the stream already
    /// needs a key frame.
    fn declare_lost(&mut self) {
        if self.state == ReceiverState::Decoding {
            self.state = ReceiverState::Loss;
        }
    }

    /// Limits the memory the stream's frames take: in the `LOSS` and
    /// `KEY_ONLY` states, discards the frames that can never be decoded
    /// (Section 6.8.3, rule 1); then drops the oldest incomplete frame while
    /// too many are kept (Section 6.6.3), and the oldest frame while too many
    /// frames or bytes are, each as lost.
    fn enforce_memory_limit(&mut self) {
        self.discard_unusable();
        loop {
            let incomplete: Vec<FrameNumber> = self
                .frames
                .iter()
                .filter(|(_, frame)| frame.complete.is_none())
                .map(|(&number, _)| number)
                .collect();
            if incomplete.len() <= MAX_INCOMPLETE_FRAMES {
                break;
            }
            self.frames.remove(&incomplete[0]);
            self.declare_lost();
        }
        while self.frames.len() - self.ready_frames() > MAX_KEPT_FRAMES {
            // The missing frame they wait for is lost.
            let missing = self
                .frames
                .iter()
                .find(|(_, frame)| frame.complete.is_none())
                .map(|(&number, _)| number);
            match missing {
                Some(number) if self.state == ReceiverState::Decoding => {
                    self.frames.remove(&number);
                }
                _ => {
                    self.frames.pop_first();
                }
            }
            self.declare_lost();
        }
        while self.frames.values().map(|frame| frame.bytes).sum::<usize>() > MAX_KEPT_BYTES {
            self.frames.pop_first();
            self.declare_lost();
        }
    }

    /// Returns how many kept frames wait only for the decoder: the complete
    /// frames that follow, in order, the frame last decoded or being decoded
    /// in the `DECODING` state, or the newest frame the decoder can jump to
    /// in the others.
    fn ready_frames(&self) -> usize {
        let first = match self.state {
            ReceiverState::Decoding => {
                Some(self.decoding.unwrap_or(self.last_decoded).0.into_inner() + 1)
            }
            ReceiverState::Loss | ReceiverState::KeyOnly => self
                .frames
                .iter()
                .rev()
                .find(|(_, frame)| self.is_jump_target(frame))
                .map(|(number, _)| number.0.into_inner()),
        };
        let Some(first) = first.and_then(|first| VarInt::new(first).ok()) else {
            return 0;
        };
        let mut next = first.into_inner();
        self.frames
            .range(FrameNumber(first)..)
            .take_while(|(number, frame)| {
                let ready = number.0.into_inner() == next && frame.complete.is_some();
                next += 1;
                ready
            })
            .count()
    }

    /// In the `LOSS` and `KEY_ONLY` states, discards the frames below the
    /// first that may be acceptable: the decoder can only jump past them.
    fn discard_unusable(&mut self) {
        if self.state == ReceiverState::Decoding {
            return;
        }
        let state = self.state;
        let may_be_acceptable = |frame: &PendingFrame| {
            frame.flags.is_none()
                || frame.has_flag(FragmentFlags::KEY_FRAME)
                || (state == ReceiverState::Loss && frame.has_flag(FragmentFlags::RECOVERY))
        };
        let first = self
            .frames
            .iter()
            .find(|(_, frame)| may_be_acceptable(frame))
            .map(|(&number, _)| number);
        match first {
            Some(first) => self.frames = self.frames.split_off(&first),
            None => self.frames.clear(),
        }
    }
}

/// Returns how long an incomplete frame may go without a new fragment before a
/// `NACK` is sent for it.
fn idle_nack_delay(smoothed_rtt: Duration) -> Duration {
    (smoothed_rtt / IDLE_NACK_RTT_DIVISOR).max(MIN_IDLE_NACK_DELAY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{BlockSet, DatagramPayload};
    use crate::video::{FrameKind, VideoSender};
    use crate::wire::Reader;

    /// A smoothed RTT for the tests.
    const RTT: Duration = Duration::from_millis(10);

    /// The datagram size for the tests.
    const DATAGRAM_SIZE: usize = 200;

    /// A test harness: a sender and a receiver for one 128 × 64 monitor.
    struct Harness {
        /// The host side.
        sender: VideoSender,
        /// The client side.
        receiver: VideoReceiver,
        /// The current time.
        now: Instant,
    }

    impl Harness {
        /// Creates a harness.
        fn new() -> Self {
            let now = Instant::now();
            let stream_id = StreamId(VarInt::from_u32(1));
            Self {
                sender: VideoSender::new(stream_id),
                receiver: VideoReceiver::new(
                    stream_id,
                    VideoLayer::Motion,
                    BlockGrid::for_monitor(128, 64),
                    SessionClock::new(now),
                ),
                now,
            }
        }

        /// Encodes a frame of `kind` with `size` bytes of temporal unit.
        fn send(&mut self, kind: FrameKind, size: usize) -> Vec<Vec<u8>> {
            let header = FrameHeader {
                capture_timestamp: Timestamp::default(),
                recovery_base: None,
                base_frame_number: None,
                blocks: BlockSet::full(2),
            };
            self.sender
                .send_frame(header, &vec![7; size], kind, DATAGRAM_SIZE, self.now)
                .unwrap()
                .datagrams
        }

        /// Delivers datagrams to the receiver.
        fn deliver(&mut self, datagrams: &[Vec<u8>]) {
            for datagram in datagrams {
                let mut reader = Reader::new(&datagram[1..]);
                let fragment = VideoFragment::decode(&mut reader).unwrap();
                self.receiver.handle_fragment(&fragment, RTT, self.now);
            }
        }

        /// Decodes every decodable frame and returns their numbers.
        fn decode_all(&mut self) -> Vec<u64> {
            let mut decoded = Vec::new();
            while let Some(frame) = self.receiver.poll_decodable() {
                decoded.push(frame.frame_number.0.into_inner());
                self.receiver.frame_decoded(frame.frame_number, self.now);
            }
            decoded
        }

        /// Passes the receiver's feedback to the sender and returns the records.
        fn feedback(&mut self) -> Vec<FeedbackRecord> {
            let records = self.receiver.poll_feedback(self.now);
            for record in &records {
                match record {
                    FeedbackRecord::Decoded(decoded) => {
                        self.sender.handle_decoded(decoded.last_decoded_frame);
                    }
                    FeedbackRecord::Loss(loss) => self.sender.handle_loss(loss, RTT, self.now),
                    _ => {}
                }
            }
            records
        }
    }

    /// Returns a frame number.
    fn frame(number: u32) -> FrameNumber {
        FrameNumber(VarInt::from_u32(number))
    }

    /// Complete frames waiting for a slow decoder are all kept, in order.
    #[test]
    fn keeps_frames_for_a_slow_decoder() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 100);
        harness.deliver(&key);
        for _ in 0..MAX_KEPT_FRAMES * 2 {
            let delta = harness.send(FrameKind::Delta, 100);
            harness.deliver(&delta);
        }
        assert_eq!(harness.receiver.state(), ReceiverState::KeyOnly);
        let decoded = harness.decode_all();
        assert_eq!(
            decoded,
            (1..=MAX_KEPT_FRAMES as u64 * 2 + 1).collect::<Vec<_>>()
        );
        assert_eq!(harness.receiver.state(), ReceiverState::Decoding);
    }

    /// A stream keeps a bounded number of frames waiting for a missing one,
    /// and in the `LOSS` state discards those that can never be decoded.
    #[test]
    fn bounds_kept_frames() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 100);
        harness.deliver(&key);
        assert_eq!(harness.decode_all(), [1]);
        // Frame 2 never arrives, so later frames wait for it.
        harness.send(FrameKind::Delta, 100);
        for _ in 0..MAX_KEPT_FRAMES + 10 {
            let delta = harness.send(FrameKind::Delta, 100);
            harness.deliver(&delta);
        }
        assert!(harness.receiver.frames.len() <= MAX_KEPT_FRAMES);
        assert_eq!(harness.receiver.state(), ReceiverState::Loss);
        let delta = harness.send(FrameKind::Delta, 100);
        harness.deliver(&delta);
        assert!(harness.receiver.frames.is_empty());
        let key = harness.send(FrameKind::KeyRecovery, 100);
        harness.deliver(&key);
        assert_eq!(harness.decode_all().len(), 1);
        assert_eq!(harness.receiver.state(), ReceiverState::Decoding);
    }

    /// Frames arriving out of order are reassembled and decoded in order.
    #[test]
    fn decodes_in_order() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 1000);
        let delta = harness.send(FrameKind::Delta, 500);
        let mut reversed = key.clone();
        reversed.reverse();
        harness.deliver(&delta);
        harness.deliver(&reversed);
        assert_eq!(harness.decode_all(), [1, 2]);
        assert_eq!(harness.receiver.state(), ReceiverState::Decoding);
        assert!(matches!(
            harness.feedback().as_slice(),
            [.., FeedbackRecord::Decoded(DecodedRecord { last_decoded_frame, .. })]
                if *last_decoded_frame == frame(2)
        ));
    }

    /// The first frame resets composition and carries the temporal unit.
    #[test]
    fn first_frame_resets_composition() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 300);
        harness.deliver(&key);
        let decodable = harness.receiver.poll_decodable().unwrap();
        assert!(decodable.resets_composition);
        assert_eq!(decodable.temporal_unit(), vec![7; 300]);
        assert!(decodable.header.blocks.is_full());
    }

    /// A gap in a frame's fragments is NACKed and retransmissions complete it.
    #[test]
    fn nack_and_retransmission() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 1000);
        let mut partial = key.clone();
        let missing = partial.remove(2);
        harness.deliver(&partial);
        let records = harness.feedback();
        let nack = records
            .iter()
            .find_map(|record| match record {
                FeedbackRecord::Nack(nack) => Some(nack.clone()),
                _ => None,
            })
            .expect("a NACK for the gap");
        assert_eq!(nack.missing[0].first_index.into_inner(), 2);
        assert_eq!(nack.missing[0].count.into_inner(), 1);

        harness.now += RTT;
        let retransmitted = harness.sender.handle_nack(&nack, RTT, harness.now);
        assert_eq!(retransmitted.len(), 1);
        assert_eq!(retransmitted[0][1..4], missing[1..4]);
        harness.deliver(&retransmitted);
        assert_eq!(harness.decode_all(), [1]);
    }

    /// A frame that makes no progress after a NACK is lost, the stream enters
    /// `LOSS`, and a recovery frame from Last Decoded Frame ends it.
    #[test]
    fn loss_and_recovery() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 100);
        harness.deliver(&key);
        assert_eq!(harness.decode_all(), [1]);

        let mut lost = harness.send(FrameKind::Delta, 1000);
        lost.remove(1);
        harness.deliver(&lost);
        harness.feedback();
        harness.now += RTT + LOSS_MARGIN;
        harness.receiver.handle_timeout(RTT, harness.now);
        assert_eq!(harness.receiver.state(), ReceiverState::Loss);

        let later = harness.send(FrameKind::Delta, 100);
        harness.deliver(&later);
        assert!(harness.decode_all().is_empty());

        let records = harness.feedback();
        assert!(records.iter().any(|record| matches!(
            record,
            FeedbackRecord::Loss(LossRecord { last_decoded_frame, .. }) if *last_decoded_frame == frame(1)
        )));
        assert_eq!(harness.sender.recovery_needed(), Some(frame(1)));

        // Every feedback datagram carries the LOSS record while in LOSS.
        assert!(matches!(
            harness.receiver.loss_record(),
            Some(FeedbackRecord::Loss(LossRecord { last_decoded_frame, .. })) if last_decoded_frame == frame(1)
        ));

        let recovery = harness.send(FrameKind::Recovery { base: frame(1) }, 100);
        harness.deliver(&recovery);
        assert_eq!(harness.decode_all(), [4]);
        assert_eq!(harness.receiver.state(), ReceiverState::Decoding);
        assert_eq!(harness.receiver.loss_record(), None);
    }

    /// In `KEY_ONLY` only key frames are decoded, and a decode failure returns
    /// there.
    #[test]
    fn key_only() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 100);
        let delta = harness.send(FrameKind::Delta, 100);
        harness.deliver(&delta);
        assert!(harness.decode_all().is_empty());
        harness.deliver(&key);
        assert_eq!(harness.decode_all(), [1, 2]);

        harness.receiver.decode_failed();
        assert_eq!(harness.receiver.state(), ReceiverState::KeyOnly);
        let records = harness.feedback();
        assert!(records.iter().any(|record| matches!(
            record,
            FeedbackRecord::Loss(LossRecord { last_decoded_frame, .. }) if *last_decoded_frame == FrameNumber::NONE
        )));
        let next_delta = harness.send(FrameKind::Delta, 100);
        harness.deliver(&next_delta);
        assert!(harness.decode_all().is_empty());
        let next_key = harness.send(FrameKind::KeyRecovery, 100);
        harness.deliver(&next_key);
        let decodable = harness.receiver.poll_decodable().unwrap();
        assert_eq!(decodable.frame_number, frame(4));
        assert!(decodable.resets_composition);
    }

    /// Inconsistent fragments make a frame lost.
    #[test]
    fn inconsistent_fragments() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 100);
        harness.deliver(&key);
        harness.decode_all();

        let fragment = |index: u32, flags: FragmentFlags, data: &'static [u8]| VideoFragment {
            stream_id: StreamId(VarInt::from_u32(1)),
            frame_number: frame(2),
            fragment_index: VarInt::from_u32(index),
            flags,
            data,
        };
        harness.receiver.handle_fragment(
            &fragment(0, FragmentFlags::default(), &[1, 2, 3]),
            RTT,
            harness.now,
        );
        harness.receiver.handle_fragment(
            &fragment(1, FragmentFlags::default(), &[1, 2]),
            RTT,
            harness.now,
        );
        assert_eq!(harness.receiver.state(), ReceiverState::Loss);
    }

    /// Too many incomplete frames drop the oldest as lost.
    #[test]
    fn memory_limit() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 100);
        harness.deliver(&key);
        harness.decode_all();
        for _ in 0..=MAX_INCOMPLETE_FRAMES {
            let mut frame = harness.send(FrameKind::Delta, 1000);
            frame.pop();
            harness.deliver(&frame);
        }
        assert_eq!(harness.receiver.state(), ReceiverState::Loss);
    }

    /// A frame that may not be NACKed in the `LOSS` state does not keep the
    /// timer due.
    #[test]
    fn no_idle_timer_for_unacceptable_frames() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 100);
        harness.deliver(&key);
        harness.decode_all();
        harness.receiver.state = ReceiverState::Loss;
        harness.receiver.loss_sent_at = Some(harness.now);
        let mut delta = harness.send(FrameKind::Delta, 1000);
        delta.pop();
        harness.deliver(&delta);
        harness.feedback();
        harness.now += Duration::from_millis(5);
        harness.receiver.handle_timeout(RTT, harness.now);
        let deadline = harness.receiver.poll_timeout(RTT).unwrap();
        assert!(deadline > harness.now);
    }

    /// Once feedback has been polled, the next feedback deadline is in the future,
    /// whether or not frames are still arriving.
    #[test]
    fn feedback_deadline_moves_forward() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 100);
        harness.deliver(&key);
        harness.decode_all();
        for step_ms in [0u64, 50, 150, 1200, 2500] {
            harness.now += Duration::from_millis(step_ms);
            if step_ms == 50 {
                let delta = harness.send(FrameKind::Delta, 100);
                harness.deliver(&delta);
                harness.decode_all();
            }
            harness.feedback();
            let deadline = harness.receiver.poll_timeout(RTT).unwrap();
            assert!(deadline > harness.now, "after {step_ms} ms");
        }
    }

    /// Missing ranges describe gaps and an open tail.
    #[test]
    fn missing_ranges() {
        let now = Instant::now();
        let mut frame = PendingFrame::new(Some(FragmentFlags::default()), now);
        for index in [0u32, 3] {
            let fragment = VideoFragment {
                stream_id: StreamId(VarInt::from_u32(1)),
                frame_number: FrameNumber::FIRST,
                fragment_index: VarInt::from_u32(index),
                flags: FragmentFlags::default(),
                data: &[0; 4],
            };
            frame.insert(&fragment, FragmentFlags::default());
        }
        let ranges = |reason| -> Vec<(u64, u64)> {
            frame
                .missing(reason)
                .iter()
                .map(|range| (range.first_index.into_inner(), range.count.into_inner()))
                .collect()
        };
        assert_eq!(ranges(NackReason::Stalled), [(1, 2), (4, 0)]);
        assert_eq!(ranges(NackReason::Gap), [(1, 2)]);
    }

    /// Presentation queues a `FRAME_TIMING` record.
    #[test]
    fn frame_timing() {
        let mut harness = Harness::new();
        let key = harness.send(FrameKind::Key, 100);
        harness.deliver(&key);
        harness.decode_all();
        harness.receiver.frame_presented(
            FrameNumber::FIRST,
            Some(harness.now + Duration::from_millis(3)),
        );
        let records = harness.feedback();
        assert!(records.iter().any(|record| matches!(
            record,
            FeedbackRecord::FrameTiming(timing) if timing.presented_time.as_micros() == 3000
        )));
    }
}
