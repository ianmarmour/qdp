//! Host and client endpoints that establish QDP connections (Sections 3.1 and
//! 3.5.1).

use std::net::SocketAddr;
use std::time::Duration;

use qdp::registry::ErrorCode;
use quinn::Endpoint;

use crate::config::{client_config, host_config};
use crate::{HostVerification, QdpConnection, TlsIdentity, TransportError};

/// A host listening for QDP connections.
#[derive(Debug)]
pub struct HostEndpoint {
    /// The QUIC endpoint.
    endpoint: Endpoint,
}

impl HostEndpoint {
    /// Listens on `address`, presenting `identity` to clients.
    pub fn bind(address: SocketAddr, identity: TlsIdentity) -> Result<Self, TransportError> {
        let endpoint = Endpoint::server(host_config(identity)?, address)?;
        Ok(Self { endpoint })
    }

    /// Returns the address the host listens on.
    pub fn local_address(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Waits for the next client's first packet, returning its connection
    /// before the handshake, so that the host can refuse it or establish it
    /// on a task of its own: one slow client then delays no other. Returns
    /// `None` once the endpoint is closed.
    pub async fn accept(&self) -> Option<IncomingConnection> {
        let incoming = self.endpoint.accept().await?;
        Some(IncomingConnection { incoming })
    }

    /// Waits until every connection has closed and its peer has been told.
    pub async fn wait_idle(&self) {
        self.endpoint.wait_idle().await;
    }

    /// Stops accepting connections and closes the open ones with
    /// `QDP_HOST_SHUTDOWN` (Section 4.7).
    pub fn close(&self) {
        let code = quinn::VarInt::from_u64(ErrorCode::HOST_SHUTDOWN.0.into_inner())
            .unwrap_or(quinn::VarInt::MAX);
        self.endpoint.close(code, b"host shutting down");
    }
}

/// How long a client has to complete the handshake and open its control
/// stream; the session's own timer then gives it the rest of the 10 seconds
/// to send `CLIENT_HELLO` (Section 4.2).
const ESTABLISH_TIMEOUT: Duration = Duration::from_secs(10);

/// A client's connection before its handshake.
#[derive(Debug)]
pub struct IncomingConnection {
    /// The connection attempt.
    incoming: quinn::Incoming,
}

impl IncomingConnection {
    /// Returns the client's address.
    pub fn remote_address(&self) -> SocketAddr {
        self.incoming.remote_address()
    }

    /// Returns whether the client has proved it receives packets at its
    /// address, through QUIC's Retry.
    pub fn address_validated(&self) -> bool {
        self.incoming.remote_address_validated()
    }

    /// Asks the client to prove its address with a QUIC Retry, which it
    /// answers with a new connection attempt (Section 11.7).
    pub fn retry(self) {
        if let Err(error) = self.incoming.retry() {
            error.into_incoming().refuse();
        }
    }

    /// Refuses the connection.
    pub fn refuse(self) {
        self.incoming.refuse();
    }

    /// Completes the handshake and accepts the client's control stream, the
    /// first bidirectional stream, on which it sends `CLIENT_HELLO`. A client
    /// that does not open it in time is closed with `QDP_HANDSHAKE_TIMEOUT`.
    pub async fn establish(self) -> Result<QdpConnection, TransportError> {
        let connection = tokio::time::timeout(ESTABLISH_TIMEOUT, self.incoming)
            .await
            .map_err(|_| TransportError::Config("the handshake did not complete in time"))??;
        match tokio::time::timeout(ESTABLISH_TIMEOUT, connection.accept_bi()).await {
            Ok(streams) => {
                let (send, recv) = streams?;
                QdpConnection::new(connection, send, recv)
            }
            Err(_) => {
                connection.close(
                    crate::connection::quic_code(ErrorCode::HANDSHAKE_TIMEOUT),
                    b"the control stream was not opened in time",
                );
                Err(TransportError::Config(
                    "the control stream was not opened in time",
                ))
            }
        }
    }
}

/// A client endpoint, which may connect to several hosts.
#[derive(Debug)]
pub struct ClientEndpoint {
    /// The QUIC endpoint.
    endpoint: Endpoint,
    /// The certificate presented to hosts that ask for one, if any.
    identity: Option<TlsIdentity>,
}

impl ClientEndpoint {
    /// Creates a client endpoint bound to `address`, usually an unspecified
    /// address with port 0, which presents no certificate.
    pub fn bind(address: SocketAddr) -> Result<Self, TransportError> {
        Ok(Self {
            endpoint: Endpoint::client(address)?,
            identity: None,
        })
    }

    /// Returns a client endpoint over `endpoint`, as one whose connections go
    /// through a gateway's tunnel (Section 13.6).
    pub(crate) fn from_quinn(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            identity: None,
        }
    }

    /// Presents `identity` to the hosts the endpoint connects to from now
    /// on, for the `CLIENT_CERTIFICATE` method (Section 4.4.2).
    pub fn set_identity(&mut self, identity: TlsIdentity) {
        self.identity = Some(identity);
    }

    /// Connects to the host at `address`, verifying it with `verification`, and
    /// opens the control stream.
    ///
    /// `server_name` is sent in the TLS handshake; with public key infrastructure
    /// it must be a name the host's certificate is valid for.
    pub async fn connect(
        &self,
        address: SocketAddr,
        server_name: &str,
        verification: HostVerification,
    ) -> Result<QdpConnection, TransportError> {
        let identity = self.identity.as_ref().map(TlsIdentity::clone_key);
        let config = client_config(verification, identity)?;
        let connection = self
            .endpoint
            .connect_with(config, address, server_name)?
            .await?;
        let (send, recv) = connection.open_bi().await?;
        QdpConnection::new(connection, send, recv)
    }

    /// Waits until every connection of the endpoint has closed cleanly.
    pub async fn wait_idle(&self) {
        self.endpoint.wait_idle().await;
    }

    /// Ends the endpoint's sessions normally, with `QDP_NO_ERROR` and
    /// `reason`, as when the viewer closes before a session ends on its own.
    pub fn close(&self, reason: &str) {
        let code = quinn::VarInt::from_u64(ErrorCode::NO_ERROR.0.into_inner())
            .unwrap_or(quinn::VarInt::MAX);
        self.endpoint.close(code, reason.as_bytes());
    }
}
