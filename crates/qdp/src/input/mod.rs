//! Sans-I/O input delivery (Sections 8.5 to 8.7).
//!
//! [`InputSender`] runs the client's side: it numbers reports, merges motion,
//! retains every report until the host acknowledges it, repeats unacknowledged
//! reports and sends heartbeats while anything is held. [`InputReceiver`] runs the
//! host's side: it applies each report exactly once in sequence order, skips
//! reports that will never arrive and acknowledges every datagram. Producing
//! reports from platform events, and injecting them on the host, are left to the
//! caller.

mod receiver;
mod sender;

pub use receiver::{InputEvent, InputReceiver, total_delta};
pub use sender::{DeviceClass, DeviceRest, InputSender, RETENTION, ReportKind};
