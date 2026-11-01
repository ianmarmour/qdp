//! Sans-I/O video stream logic (Section 6).
//!
//! [`VideoSender`] runs the host's side of one video stream: it splits encoded
//! frames into `VIDEO_FRAGMENT` datagrams, retransmits fragments the client asks
//! for, decides when a recovery frame is needed and repairs the loss of a final
//! frame. Encoding itself, and choosing how to produce a recovery frame, are left
//! to the caller.
//!
//! [`VideoReceiver`] runs the client's side: it reassembles frames, asks for
//! missing fragments, declares frames lost, hands frames to the decoder in the
//! order Section 6.8.3 allows and produces `VIDEO_FEEDBACK` records.
//! [`Composition`] decides, block by block, whether a monitor shows its motion or
//! its refinement layer (Section 6.9).

mod composition;
mod receiver;
mod sender;

pub use composition::{BlockSource, Composition};
pub use receiver::{DecodableFrame, MAX_INCOMPLETE_FRAMES, ReceiverState, VideoReceiver};
pub use sender::{FrameKind, SendFrameError, SendQueue, SentFrameInfo, SentWhen, VideoSender};
