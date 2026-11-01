//! Which encoded frame each recently sent frame number carried, so that a
//! recovery frame can be predicted from the frame the client decoded last
//! rather than be a key frame (Section 6.8.4).

use std::collections::VecDeque;

use qdp::message::{FrameNumber, StreamId};

use crate::encode_thread::{EncodedId, RecoveryBase};

/// How many sent frames are remembered: more than an encoder keeps to
/// predict from, so that every frame it could recover from is known.
const REMEMBERED_FRAMES: usize = 64;

/// The encoded frames behind the frame numbers sent lately, oldest first.
#[derive(Debug, Clone, Default)]
pub(crate) struct RecoveryBases {
    /// The stream, frame number and encoded frame of each frame sent.
    sent: VecDeque<(StreamId, FrameNumber, EncodedId)>,
}

impl RecoveryBases {
    /// Notes that frame `frame_number` of `stream` carried encoded frame
    /// `encoded`.
    pub(crate) fn sent(&mut self, stream: StreamId, frame_number: FrameNumber, encoded: EncodedId) {
        if self.sent.len() == REMEMBERED_FRAMES {
            self.sent.pop_front();
        }
        self.sent.push_back((stream, frame_number, encoded));
    }

    /// Returns the frame a recovery from the client's Last Decoded Frame
    /// `frame_number` of `stream` would be predicted from, if it was sent
    /// lately. [`FrameNumber::NONE`] names no frame: the client has none.
    pub(crate) fn base(&self, stream: StreamId, frame_number: FrameNumber) -> Option<RecoveryBase> {
        if frame_number == FrameNumber::NONE {
            return None;
        }
        self.sent
            .iter()
            .rev()
            .find(|(sent_stream, sent_number, _)| {
                *sent_stream == stream && *sent_number == frame_number
            })
            .map(|&(_, frame_number, encoded)| RecoveryBase {
                frame_number,
                encoded,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::wire::VarInt;

    /// Returns frame number `number`.
    fn frame(number: u32) -> FrameNumber {
        FrameNumber(VarInt::from_u32(number))
    }

    /// Returns encoded frame `input_index` of the first encoder.
    fn encoded(input_index: u64) -> EncodedId {
        EncodedId {
            encoder: 0,
            input_index,
        }
    }

    /// Sent frames are found by stream and number, until they are too old,
    /// and no frame is the base of a client that has none.
    #[test]
    fn finds_recent_frames() {
        let (first, second) = (StreamId(VarInt::from_u32(1)), StreamId(VarInt::from_u32(2)));
        let mut bases = RecoveryBases::default();
        for number in 1..=10 {
            bases.sent(first, frame(number), encoded(u64::from(number) + 100));
        }
        bases.sent(second, frame(3), encoded(7));
        let base = bases.base(first, frame(3)).unwrap();
        assert_eq!(base.frame_number, frame(3));
        assert_eq!(base.encoded, encoded(103));
        assert_eq!(bases.base(second, frame(3)).unwrap().encoded, encoded(7));
        assert!(bases.base(second, frame(4)).is_none());
        assert!(bases.base(first, FrameNumber::NONE).is_none());
        for number in 11..=80 {
            bases.sent(first, frame(number), encoded(u64::from(number)));
        }
        assert!(bases.base(first, frame(3)).is_none());
        assert!(bases.base(first, frame(80)).is_some());
    }
}
