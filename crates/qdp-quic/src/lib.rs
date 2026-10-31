//! QUIC transport for the Quick Desktop Protocol (QDP), built on quinn and
//! rustls.
//!
//! This crate connects the sans-I/O protocol logic of the `qdp` crate to the
//! network (Section 3 of the specification):
//!
//! - [`HostEndpoint`] and [`ClientEndpoint`] establish connections with TLS 1.3,
//!   ALPN `qdp-draft-00`, QUIC datagrams, no session resumption and no 0-RTT, and
//!   open the control stream.
//! - [`HostVerification`] selects how the client trusts the host: a pinned key,
//!   checked by [`PinnedKeyVerifier`], or public key infrastructure.
//! - [`HostIdentity`] holds a host's certificate and key, and can generate a
//!   self-signed one whose [`KeyPin`] clients pin.
//! - [`SessionDriver`] runs a client or host session state machine over a
//!   [`QdpConnection`].

mod config;
mod connection;
mod driver;
mod endpoint;
mod identity;
mod verify;

use qdp::session::{SessionError, UsageError};
use thiserror::Error;

pub use config::{HostVerification, client_config, host_config};
pub use connection::QdpConnection;
pub use driver::{ClientDriver, DrivenSession, DriverInput, HostDriver, SessionDriver};
pub use endpoint::{ClientEndpoint, HostEndpoint};
pub use identity::{HostIdentity, KeyPin};
pub use verify::PinnedKeyVerifier;

/// An error from the transport or the session it carries.
#[derive(Debug, Error)]
pub enum TransportError {
    /// The configuration is invalid.
    #[error("invalid configuration: {0}")]
    Config(&'static str),
    /// TLS rejected the configuration or a certificate.
    #[error(transparent)]
    Tls(#[from] rustls::Error),
    /// A certificate could not be generated.
    #[error(transparent)]
    Certificate(#[from] rcgen::Error),
    /// A socket operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A connection could not be started.
    #[error(transparent)]
    Connect(#[from] quinn::ConnectError),
    /// The connection failed or was closed.
    #[error(transparent)]
    Connection(#[from] quinn::ConnectionError),
    /// Writing to the control stream failed.
    #[error(transparent)]
    Write(#[from] quinn::WriteError),
    /// Reading from the control stream failed.
    #[error(transparent)]
    Read(#[from] quinn::ReadError),
    /// A datagram could not be sent.
    #[error(transparent)]
    SendDatagram(#[from] quinn::SendDatagramError),
    /// The peer does not support QUIC datagrams.
    #[error("the peer does not support QUIC datagrams")]
    DatagramsUnsupported,
    /// The peer closed its direction of the control stream.
    #[error("the control stream was closed")]
    ControlStreamClosed,
    /// The session ended with this error, and the connection was closed with its
    /// code.
    #[error("session ended: {0}")]
    Session(SessionError),
    /// The application asked the session for something it does not allow.
    #[error(transparent)]
    Usage(#[from] UsageError),
}
