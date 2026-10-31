//! The host's Stream IDs, which video and audio streams share (Sections 6.3
//! and 7.3). Any change of a stream's parameters needs a new stream, so each
//! new stream gets an ID never used before in the session.

use qdp::message::StreamId;
use qdp::wire::VarInt;

/// Hands out Stream IDs, from 1 up.
#[derive(Debug, Default)]
pub(crate) struct StreamIds {
    /// The last ID handed out, or 0 before the first.
    last: u64,
}

impl StreamIds {
    /// Returns an ID not handed out before.
    pub(crate) fn next(&mut self) -> StreamId {
        self.last += 1;
        StreamId(VarInt::new(self.last).unwrap_or(VarInt::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IDs start at 1 and never repeat.
    #[test]
    fn hands_out_new_ids() {
        let mut ids = StreamIds::default();
        assert_eq!(ids.next(), StreamId(VarInt::from_u32(1)));
        assert_eq!(ids.next(), StreamId(VarInt::from_u32(2)));
    }
}
