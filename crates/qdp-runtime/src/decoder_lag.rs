//! Whether the client's decoder keeps up with the frames it receives, from
//! its `FRAME_TIMING` records (Section 6.11). A frame's lag is the time from
//! its last fragment arriving to its decoding finishing, both on the client's
//! clock. Behind a slow decoder every frame lags, since each waits for those
//! before it. Behind a lost fragment only the frames that follow the
//! incomplete one lag: the frame completed by the retransmission is decoded
//! at once. So the decoder is behind only while the least lag of the last
//! few frames exceeds a frame interval.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use qdp::message::FrameTimingRecord;

/// How many of the latest frames' lags are compared.
const LAG_FRAMES: usize = 8;

/// How long the decoder counts as behind after its frames last showed it,
/// so that the host holding frames back, which keeps them from waiting,
/// does not at once undo itself.
const BEHIND_HOLD: Duration = Duration::from_secs(1);

/// The client's decoder, as its `FRAME_TIMING` records show it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DecoderLag {
    /// The lags of the latest frames, oldest first.
    recent: VecDeque<Duration>,
    /// Until when the decoder counts as behind, if its frames have shown it.
    behind_until: Option<Instant>,
}

impl DecoderLag {
    /// Notes a frame's `FRAME_TIMING` record, received at `now` for a stream
    /// whose frames are sent `frame_interval` apart.
    pub(crate) fn frame_timing(
        &mut self,
        record: &FrameTimingRecord,
        frame_interval: Duration,
        now: Instant,
    ) {
        let (arrived, decoded) = (
            record.last_fragment_time.as_micros(),
            record.decoded_time.as_micros(),
        );
        if arrived == 0 || decoded == 0 {
            return;
        }
        if self.recent.len() == LAG_FRAMES {
            self.recent.pop_front();
        }
        self.recent
            .push_back(Duration::from_micros(decoded.saturating_sub(arrived)));
        let least = self.recent.iter().min().copied().unwrap_or_default();
        if self.recent.len() == LAG_FRAMES && least > frame_interval {
            self.behind_until = Some(now + BEHIND_HOLD);
        }
    }

    /// Returns whether the decoder is behind at `now`.
    pub(crate) fn is_behind(&self, now: Instant) -> bool {
        self.behind_until.is_some_and(|until| now < until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::{FrameNumber, StreamId, Timestamp};
    use qdp::wire::VarInt;

    /// A frame interval of 60 frames per second.
    const INTERVAL: Duration = Duration::from_micros(16_667);

    /// Returns the record of a frame whose last fragment arrived at
    /// `arrived` and which was decoded `lag_ms` later, in milliseconds.
    fn record(arrived_ms: u32, lag_ms: u32) -> FrameTimingRecord {
        let timestamp = |ms: u32| Timestamp(VarInt::from_u32(ms * 1000));
        FrameTimingRecord {
            stream_id: StreamId(VarInt::from_u32(1)),
            frame_number: FrameNumber(VarInt::from_u32(1)),
            first_fragment_time: timestamp(arrived_ms),
            last_fragment_time: timestamp(arrived_ms),
            decoded_time: timestamp(arrived_ms + lag_ms),
            presented_time: Timestamp::default(),
        }
    }

    /// Frames that wait behind a lost fragment do not make the decoder
    /// behind, since the frame the retransmission completed is decoded at
    /// once.
    #[test]
    fn loss_is_not_a_slow_decoder() {
        let now = Instant::now();
        let mut lag = DecoderLag::default();
        for frame in 0..20 {
            // Every other frame waits 30 ms for an earlier one's fragment.
            let wait = if frame % 2 == 0 { 30 } else { 2 };
            lag.frame_timing(&record(1 + frame * 17, wait), INTERVAL, now);
        }
        assert!(!lag.is_behind(now));
    }

    /// A decoder slower than the frame rate makes every frame lag, and it
    /// counts as behind until a while after its frames stop showing it.
    #[test]
    fn slow_decoder_is_behind() {
        let now = Instant::now();
        let mut lag = DecoderLag::default();
        for frame in 0..LAG_FRAMES as u32 - 1 {
            lag.frame_timing(&record(1 + frame * 17, 40 + frame * 23), INTERVAL, now);
        }
        // Too few frames to judge yet.
        assert!(!lag.is_behind(now));
        lag.frame_timing(&record(200, 40), INTERVAL, now);
        assert!(lag.is_behind(now));

        // Fast frames end it, once the hold has passed.
        let later = now + BEHIND_HOLD / 2;
        lag.frame_timing(&record(300, 2), INTERVAL, later);
        assert!(lag.is_behind(later));
        assert!(!lag.is_behind(now + BEHIND_HOLD));
        // Records without times are ignored.
        let mut unknown = record(0, 0);
        unknown.decoded_time = Timestamp::default();
        lag.frame_timing(&unknown, INTERVAL, later);
        assert_eq!(lag.recent.len(), LAG_FRAMES);
    }
}
