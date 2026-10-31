//! Authentication methods carried in Auth Data (Section 4.4).
//!
//! The session state machines ([`crate::session`]) carry Auth Data without
//! interpreting it. This module implements the method that needs cryptography,
//! SCRAM-SHA-256-PLUS, and the string preparation it relies on. The `TOKEN`,
//! `PASSWORD`, `CLIENT_CERTIFICATE` and `ANONYMOUS` methods need no exchange
//! logic beyond [`crate::message::PasswordAuthData`] and the host's own policy.

mod base64;
pub mod precis;
pub mod scram;
