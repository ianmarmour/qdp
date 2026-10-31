//! Sans-I/O video stream logic (Section 6).
//!
//! [`VideoSender`] runs the host's side of one video stream: it splits encoded
//! frames into `VIDEO_FRAGMENT` datagrams, retransmits fragments the client asks
//! for, decides when a recovery frame is needed and repairs the loss of a final
//! frame. Encoding itself, and choosing how to produce a recovery frame, are left
//! to the caller.

mod sender;

pub use sender::{FrameKind, SendFrameError, SentFrameInfo, VideoSender};
