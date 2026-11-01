//! An implementation of the Quick Desktop Protocol (QDP).
//!
//! QDP is a low-latency protocol for remote desktop and game streaming over QUIC.
//! The protocol is specified in the `doc/spec` directory of the repository.
//!
//! This crate is organised in layers:
//!
//! - [`registry`] holds the code points of every type and identifier space the
//!   specification defines (Section 12).
//! - [`wire`] holds the encoding primitives shared by every message: variable-length
//!   integers, strings and octet sequences, and the framing of control messages and
//!   datagrams (Sections 1.5, 3.6 and 3.7).
//! - [`message`] holds the typed control messages and datagrams, built on [`wire`].
//! - [`session`] holds the sans-I/O state machines that run the control stream for
//!   a client and a host (Section 4).
//! - [`auth`] implements SCRAM-SHA-256-PLUS for the Auth Data of the session
//!   (Section 4.4.4).
//! - [`discovery`] describes the DNS-SD service hosts advertise on a local
//!   network (Section 3.13).
//! - [`video`] holds the sans-I/O logic of video streams: fragmentation,
//!   reassembly, retransmission, loss recovery and composition (Section 6).
//! - [`hid`] parses and validates the HID report descriptors of passthrough
//!   devices (Section 8.2.5).
//! - [`input`] holds the sans-I/O delivery of input reports: retention and repeats
//!   on the client, ordering and acknowledgement on the host (Section 8).
//! - [`time`] holds the session clock that timestamps are measured on and the
//!   estimation of the peer's clock offset (Sections 5.1 and 5.2).

pub mod auth;
pub mod discovery;
pub mod gateway;
pub mod hid;
pub mod input;
pub mod message;
pub mod registry;
pub mod session;
pub mod time;
pub mod video;
pub mod wire;

/// The ALPN token of the draft of the specification this crate implements
/// (Section 3.2).
pub const ALPN: &[u8] = b"qdp-draft-00";
