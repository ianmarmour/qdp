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
    /// When the latest frames were decoded, on the client's clock, in
    /// microseconds, oldest first.
    decoded: VecDeque<u64>,
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
        if self.decoded.len() == LAG_FRAMES {
            self.decoded.pop_front();
        }
        self.decoded.push_back(decoded);
        if self.recent.len() < LAG_FRAMES {
            return;
        }
        let mut lags: Vec<Duration> = self.recent.iter().copied().collect();
        lags.sort_unstable();
        let (least, median) = (lags[0], lags[lags.len() / 2]);
        // Every frame lagging shows a slow decoder, where a lost fragment
        // delays only some. Once behind, the decoder stays so while most of
        // its frames lag: holding frames back makes some arrive just as it
        // is ready for them, without lag, though it is no faster.
        let lagging = least > frame_interval || (self.is_behind(now) && median > frame_interval);
        if lagging {
            self.behind_until = Some(now + BEHIND_HOLD);
        }
    }

    /// Returns whether the decoder is behind at `now`.
    pub(crate) fn is_behind(&self, now: Instant) -> bool {
        self.behind_until.is_some_and(|until| now < until)
    }

    /// Returns how long the client took to decode each of its latest
    /// frames, on average, from when one was decoded to when the next was:
    /// while it is behind, the pace at which it takes frames.
    pub(crate) fn decode_interval(&self) -> Option<Duration> {
        let (&first, &last) = (self.decoded.front()?, self.decoded.back()?);
        let intervals = self.decoded.len() as u64 - 1;
        (intervals > 0 && last > first).then(|| Duration::from_micros((last - first) / intervals))
    }

    /// Returns the client's decoder at `now`: behind, taking a frame about
    /// every interval its frames show, or keeping up.
    pub(crate) fn state(&self, now: Instant) -> ClientDecoder {
        if self.is_behind(now) {
            ClientDecoder::Behind {
                pace: self.decode_interval(),
            }
        } else {
            ClientDecoder::KeepingUp
        }
    }
}

/// How the client's decoder keeps up with the frames sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClientDecoder {
    /// It decodes frames as fast as they arrive.
    KeepingUp,
    /// It decodes frames slower than they arrive.
    Behind {
        /// The time between its decoding one frame and the next, once its
        /// frames have shown it.
        pace: Option<Duration>,
    },
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
    /// counts as behind until a while after most of its frames stop lagging.
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

        // One fast frame does not end it; once most frames are fast, the
        // hold runs out.
        let later = now + BEHIND_HOLD / 2;
        lag.frame_timing(&record(300, 2), INTERVAL, later);
        assert!(lag.is_behind(now + BEHIND_HOLD));
        for frame in 0..LAG_FRAMES as u32 / 2 {
            lag.frame_timing(&record(320 + frame * 17, 2), INTERVAL, later);
        }
        let renewed = later + BEHIND_HOLD;
        assert!(lag.is_behind(later));
        assert!(!lag.is_behind(renewed));
        // Records without times are ignored.
        let mut unknown = record(0, 0);
        unknown.decoded_time = Timestamp::default();
        lag.frame_timing(&unknown, INTERVAL, later);
        assert_eq!(lag.recent.len(), LAG_FRAMES);
    }

    /// A decoder that is behind stays behind while most of its frames lag,
    /// though holding frames back lets some arrive just in time, without
    /// lag; once most stop lagging, it is behind no more after the hold.
    #[test]
    fn stays_behind_while_most_frames_lag() {
        let now = Instant::now();
        let mut lag = DecoderLag::default();
        for frame in 0..LAG_FRAMES as u32 {
            lag.frame_timing(&record(1 + frame * 120, 120), INTERVAL, now);
        }
        assert!(lag.is_behind(now));
        // Every third frame arrives just in time, for a few holds over.
        let mut at = now;
        for frame in 0..4 * LAG_FRAMES as u32 {
            at += BEHIND_HOLD / 2;
            let wait = if frame % 3 == 0 { 1 } else { 120 };
            lag.frame_timing(&record(2000 + frame * 120, wait), INTERVAL, at);
            assert!(lag.is_behind(at), "frame {frame}");
        }
        // Most frames stop lagging: the hold runs out.
        for frame in 0..LAG_FRAMES as u32 {
            let wait = if frame % 3 == 0 { 120 } else { 1 };
            lag.frame_timing(&record(9000 + frame * 17, wait), INTERVAL, at);
        }
        assert!(!lag.is_behind(at + BEHIND_HOLD));
    }

    /// The pace of a slow decoder is the time between its frames' decoding,
    /// whatever their lag.
    #[test]
    fn measures_the_decoders_pace() {
        let now = Instant::now();
        let mut lag = DecoderLag::default();
        assert_eq!(lag.decode_interval(), None);
        assert_eq!(lag.state(now), ClientDecoder::KeepingUp);
        // Frames arriving every 17 ms are decoded every 40 ms.
        for frame in 0..LAG_FRAMES as u32 {
            lag.frame_timing(&record(1 + frame * 17, 40 + frame * 23), INTERVAL, now);
        }
        assert_eq!(lag.decode_interval(), Some(Duration::from_millis(40)));
        assert_eq!(
            lag.state(now),
            ClientDecoder::Behind {
                pace: Some(Duration::from_millis(40))
            }
        );
    }
}
