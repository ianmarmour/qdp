//! The QDP gateway (Section 13 of the specification), which lets users outside
//! an organization's network reach QDP hosts inside it. It checks each user
//! against the organization's directory, LDAP or Active Directory, then a
//! TOTP code from their authenticator app, and tunnels their QDP sessions,
//! end to end, to the hosts its policy lets them reach, listed by the gateway
//! or named as inside the network.
//!
//! It stands apart from QDP's apps and hosts: it never reads a session, and
//! needs only the protocol crate's gateway messages and QUIC. The
//! `qdp-gateway` command runs it.

pub mod config;
pub mod directory;
pub mod enrollments;
pub mod keys;
pub mod limiter;
pub mod locations;
pub mod policy;
pub mod server;
pub mod service;
pub mod tokens;
pub mod totp_store;
