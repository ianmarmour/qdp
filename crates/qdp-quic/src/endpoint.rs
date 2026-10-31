//! Host and client endpoints that establish QDP connections (Sections 3.1 and
//! 3.5.1).

use std::net::SocketAddr;

use qdp::registry::ErrorCode;
use quinn::Endpoint;

use crate::config::{client_config, host_config};
use crate::{HostIdentity, HostVerification, QdpConnection, TransportError};

/// A host listening for QDP connections.
#[derive(Debug)]
pub struct HostEndpoint {
    /// The QUIC endpoint.
    endpoint: Endpoint,
}

impl HostEndpoint {
    /// Listens on `address`, presenting `identity` to clients.
    pub fn bind(address: SocketAddr, identity: HostIdentity) -> Result<Self, TransportError> {
        let endpoint = Endpoint::server(host_config(identity)?, address)?;
        Ok(Self { endpoint })
    }

    /// Returns the address the host listens on.
    pub fn local_address(&self) -> Result<SocketAddr, TransportError> {
        Ok(self.endpoint.local_addr()?)
    }

    /// Waits for the next client, completes its handshake and accepts its control
    /// stream, the first bidirectional stream, on which it sends `CLIENT_HELLO`.
    /// Returns `None` once the endpoint is closed.
    pub async fn accept(&self) -> Option<Result<QdpConnection, TransportError>> {
        let incoming = self.endpoint.accept().await?;
        Some(Self::establish(incoming).await)
    }

    /// Completes the handshake of one incoming connection.
    async fn establish(incoming: quinn::Incoming) -> Result<QdpConnection, TransportError> {
        let connection = incoming.await?;
        let (send, recv) = connection.accept_bi().await?;
        QdpConnection::new(connection, send, recv)
    }

    /// Stops accepting connections and closes the open ones with
    /// `QDP_HOST_SHUTDOWN` (Section 4.7).
    pub fn close(&self) {
        let code = quinn::VarInt::from_u64(ErrorCode::HOST_SHUTDOWN.0.into_inner())
            .unwrap_or(quinn::VarInt::MAX);
        self.endpoint.close(code, b"host shutting down");
    }
}

/// A client endpoint, which may connect to several hosts.
#[derive(Debug)]
pub struct ClientEndpoint {
    /// The QUIC endpoint.
    endpoint: Endpoint,
}

impl ClientEndpoint {
    /// Creates a client endpoint bound to `address`, usually an unspecified
    /// address with port 0.
    pub fn bind(address: SocketAddr) -> Result<Self, TransportError> {
        Ok(Self {
            endpoint: Endpoint::client(address)?,
        })
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
        let config = client_config(verification)?;
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
}
