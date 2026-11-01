//! A client's connection to a gateway (Section 13): signing in, choosing a
//! host, and the tunnel that then carries the client's QDP connection to the
//! host, each of its UDP payloads in a DATAGRAM frame of the gateway
//! connection.
//!
//! [`GatewayClient`] signs in and opens a host; [`GatewayClient::into_tunnel`]
//! then returns a [`ClientEndpoint`] whose connections go through the tunnel,
//! so that a session runs through a gateway as it runs directly.

use std::fmt;
use std::io::{self, IoSliceMut};
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use bytes::Bytes;
use qdp::gateway::{
    GATEWAY_VERSION, GatewayHello, GatewayMessage, HostEntry, NextFactor, Open, OpenFailed, Opened,
    SessionToken,
};
use qdp::message::{AuthMethod, AuthStart, AuthStatus, PasswordAuthData};
use qdp::wire::ControlStreamDecoder;
use quinn::udp::{RecvMeta, Transmit};
use quinn::{AsyncUdpSocket, Connection, Endpoint, RecvStream, SendStream, UdpPoller};
use tokio::sync::mpsc;

use crate::{ClientEndpoint, HostVerification, TlsIdentity, TransportError};

/// The datagrams from the host a tunnel holds before the inner connection
/// reads them; more are dropped, as a full socket buffer drops them.
const TUNNEL_QUEUE: usize = 1024;

/// The bytes read from the control stream at a time.
const READ_CHUNK: usize = 4096;

/// What a sign-in attempt came to (Section 13.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    /// The user is signed in.
    SignedIn,
    /// The user must also authenticate with one of these methods; with a
    /// TOTP secret to enroll, if the gateway offers one.
    NextFactor(NextFactor),
    /// The attempt failed, for this reason. If `last`, the gateway closes the
    /// connection.
    Failed {
        /// The gateway's reason.
        reason: String,
        /// Whether no other attempt is allowed.
        last: bool,
    },
}

/// A client's signed-in or signing-in connection to a gateway.
pub struct GatewayClient {
    /// The QUIC endpoint the connection uses.
    endpoint: Endpoint,
    /// The connection.
    connection: Connection,
    /// The control stream's sending side.
    send: SendStream,
    /// The control stream's receiving side.
    receive: RecvStream,
    /// The messages split from it.
    decoder: ControlStreamDecoder,
    /// The gateway's name for users.
    gateway_name: String,
    /// The methods the gateway lets a sign-in start with.
    methods: Vec<AuthMethod>,
    /// The latest session token the gateway gave, not yet taken.
    session_token: Option<SessionToken>,
}

impl fmt::Debug for GatewayClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayClient")
            .field("gateway_name", &self.gateway_name)
            .field("remote", &self.connection.remote_address())
            .finish_non_exhaustive()
    }
}

impl GatewayClient {
    /// Connects to the gateway at `address`, verifying it with
    /// `verification`, presenting `identity` so that identity assertions can
    /// name the client's key, and says hello as `client_name`.
    pub async fn connect(
        address: SocketAddr,
        server_name: &str,
        verification: HostVerification,
        identity: Option<TlsIdentity>,
        client_name: &str,
    ) -> Result<Self, TransportError> {
        let local: SocketAddr = if address.is_ipv4() {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
        };
        let endpoint = Endpoint::client(local)?;
        let config = crate::gateway_client_config(verification, identity)?;
        let connection = endpoint.connect_with(config, address, server_name)?.await?;
        let (send, receive) = connection.open_bi().await?;
        let mut client = Self {
            endpoint,
            connection,
            send,
            receive,
            decoder: ControlStreamDecoder::new(),
            gateway_name: String::new(),
            methods: Vec::new(),
            session_token: None,
        };
        client
            .send(GatewayMessage::Hello(GatewayHello {
                version: GATEWAY_VERSION,
                client_name: client_name.to_owned(),
            }))
            .await?;
        match client.receive().await? {
            GatewayMessage::Welcome(welcome) => {
                client.gateway_name = welcome.gateway_name;
                client.methods = welcome.auth_methods;
                Ok(client)
            }
            _ => Err(TransportError::Config(
                "the gateway did not welcome the client",
            )),
        }
    }

    /// Returns the gateway's name for users.
    pub fn gateway_name(&self) -> &str {
        &self.gateway_name
    }

    /// Returns the methods a sign-in may start with.
    pub fn methods(&self) -> &[AuthMethod] {
        &self.methods
    }

    /// Signs in with `method` and `auth_data`, or continues a sign-in with
    /// the next factor.
    pub async fn sign_in(
        &mut self,
        method: AuthMethod,
        auth_data: Vec<u8>,
    ) -> Result<SignIn, TransportError> {
        self.send(GatewayMessage::AuthStart(AuthStart {
            auth_method: method,
            auth_data,
        }))
        .await?;
        let result = loop {
            match self.receive().await? {
                GatewayMessage::AuthResult(result) => break result,
                GatewayMessage::Unknown(_) => {}
                _ => return Err(TransportError::Config("the gateway answered out of turn")),
            }
        };
        Ok(match result.status {
            AuthStatus::Success => SignIn::SignedIn,
            AuthStatus::NextFactor => {
                SignIn::NextFactor(NextFactor::from_bytes(&result.auth_data).map_err(|_| {
                    TransportError::Config("the gateway's next factor is malformed")
                })?)
            }
            AuthStatus::FailedRetry => SignIn::Failed {
                reason: result.reason,
                last: false,
            },
            AuthStatus::FailedFinal => SignIn::Failed {
                reason: result.reason,
                last: true,
            },
        })
    }

    /// Signs in with a directory user name and password.
    pub async fn sign_in_with_password(
        &mut self,
        user_name: &str,
        password: &str,
    ) -> Result<SignIn, TransportError> {
        let auth_data = PasswordAuthData {
            user_name: user_name.to_owned(),
            password: password.to_owned(),
        }
        .to_bytes();
        self.sign_in(AuthMethod::PASSWORD, auth_data).await
    }

    /// Continues a sign-in with a TOTP code from the user's authenticator app.
    pub async fn sign_in_with_code(&mut self, code: &str) -> Result<SignIn, TransportError> {
        self.sign_in(AuthMethod::TOTP, code.trim().as_bytes().to_vec())
            .await
    }

    /// Signs in with a session token from an earlier sign-in.
    pub async fn sign_in_with_token(&mut self, token: &[u8]) -> Result<SignIn, TransportError> {
        self.sign_in(AuthMethod::TOKEN, token.to_vec()).await
    }

    /// Returns the session token the gateway gave since this was last asked,
    /// which the client keeps for later connections (Section 13.4).
    pub fn take_session_token(&mut self) -> Option<SessionToken> {
        self.session_token.take()
    }

    /// Returns the hosts the user may reach.
    pub async fn hosts(&mut self) -> Result<Vec<HostEntry>, TransportError> {
        self.send(GatewayMessage::HostsRequest).await?;
        loop {
            match self.receive().await? {
                GatewayMessage::Hosts(hosts) => return Ok(hosts),
                GatewayMessage::Unknown(_) => {}
                _ => return Err(TransportError::Config("the gateway answered out of turn")),
            }
        }
    }

    /// Opens `target`: a host's ID from [`GatewayClient::hosts`], or its name
    /// or address inside the network. Returns how to verify the host, or why
    /// the gateway refused, after which another target may be opened.
    pub async fn open(
        &mut self,
        target: &str,
    ) -> Result<Result<Opened, OpenFailed>, TransportError> {
        self.send(GatewayMessage::Open(Open {
            target: target.to_owned(),
        }))
        .await?;
        loop {
            match self.receive().await? {
                GatewayMessage::Opened(opened) => return Ok(Ok(opened)),
                GatewayMessage::OpenFailed(failed) => return Ok(Err(failed)),
                GatewayMessage::Unknown(_) => {}
                _ => return Err(TransportError::Config("the gateway answered out of turn")),
            }
        }
    }

    /// Returns, once a host is open, an endpoint whose connections go to it
    /// through the tunnel, and the address to connect them to (Section 13.6).
    /// The gateway connection lasts as long as the endpoint.
    pub fn into_tunnel(self) -> Result<(ClientEndpoint, SocketAddr), TransportError> {
        let peer = self.connection.remote_address();
        let (sender, receiver) = mpsc::channel(TUNNEL_QUEUE);
        let incoming = self.connection.clone();
        tokio::spawn(async move {
            while let Ok(datagram) = incoming.read_datagram().await {
                // A full queue drops the datagram, as a full socket would.
                if let Err(mpsc::error::TrySendError::Closed(_)) = sender.try_send(datagram) {
                    break;
                }
            }
        });
        let socket = TunnelSocket {
            connection: self.connection,
            incoming: Mutex::new(receiver),
            peer,
            _endpoint: self.endpoint,
            _control: Mutex::new((self.send, self.receive)),
        };
        let endpoint = Endpoint::new_with_abstract_socket(
            quinn::EndpointConfig::default(),
            None,
            Arc::new(socket),
            Arc::new(quinn::TokioRuntime),
        )?;
        Ok((ClientEndpoint::from_quinn(endpoint), peer))
    }

    /// Writes `message` on the control stream.
    async fn send(&mut self, message: GatewayMessage) -> Result<(), TransportError> {
        let Some(frame) = message.to_frame() else {
            return Ok(());
        };
        let bytes = frame
            .to_bytes()
            .map_err(|_| TransportError::Config("a gateway message is too large"))?;
        self.send
            .write_all(&bytes)
            .await
            .map_err(|_| TransportError::Config("the gateway closed the control stream"))
    }

    /// Returns the next message other than a session token, keeping any
    /// token that comes first.
    async fn receive(&mut self) -> Result<GatewayMessage, TransportError> {
        let mut chunk = [0; READ_CHUNK];
        loop {
            let malformed = |_| TransportError::Config("the gateway sent a malformed message");
            if let Some(frame) = self.decoder.next_message().map_err(malformed)? {
                match GatewayMessage::decode(&frame).map_err(malformed)? {
                    GatewayMessage::SessionToken(token) => self.session_token = Some(token),
                    message => return Ok(message),
                }
                continue;
            }
            match self.receive.read(&mut chunk).await {
                Ok(Some(read)) => self.decoder.push(&chunk[..read]),
                Ok(None) | Err(_) => {
                    return Err(match self.connection.close_reason() {
                        Some(reason) => reason.into(),
                        None => TransportError::Config("the gateway closed the control stream"),
                    });
                }
            }
        }
    }
}

/// The socket of a tunnel's inner connection: UDP payloads sent in DATAGRAM
/// frames of the gateway connection, and received from them.
struct TunnelSocket {
    /// The gateway connection.
    connection: Connection,
    /// The datagrams from the host, as the gateway forwards them.
    incoming: Mutex<mpsc::Receiver<Bytes>>,
    /// The address the inner connection sees the host at: the gateway's.
    peer: SocketAddr,
    /// The endpoint of the gateway connection, kept for as long as it.
    _endpoint: Endpoint,
    /// The control stream, kept open for as long as the tunnel.
    _control: Mutex<(SendStream, RecvStream)>,
}

impl fmt::Debug for TunnelSocket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TunnelSocket")
            .field("peer", &self.peer)
            .finish_non_exhaustive()
    }
}

/// A poller for a tunnel, which can always send: QUIC queues datagrams.
#[derive(Debug)]
struct AlwaysWritable;

impl UdpPoller for AlwaysWritable {
    fn poll_writable(self: Pin<&mut Self>, _: &mut Context) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncUdpSocket for TunnelSocket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(AlwaysWritable)
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        match self
            .connection
            .send_datagram(Bytes::copy_from_slice(transmit.contents))
        {
            // A packet too large for the gateway's path is lost, which the
            // inner connection's path MTU discovery then avoids.
            Ok(()) | Err(quinn::SendDatagramError::TooLarge) => Ok(()),
            Err(error) => Err(io::Error::new(io::ErrorKind::ConnectionAborted, error)),
        }
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let (Some(buffer), Some(slot)) = (bufs.first_mut(), meta.first_mut()) else {
            return Poll::Ready(Ok(0));
        };
        let mut incoming = self.incoming.lock().unwrap_or_else(PoisonError::into_inner);
        match incoming.poll_recv(cx) {
            Poll::Ready(Some(datagram)) => {
                let length = datagram.len().min(buffer.len());
                buffer[..length].copy_from_slice(&datagram[..length]);
                *slot = RecvMeta {
                    addr: self.peer,
                    len: length,
                    stride: length,
                    ecn: None,
                    dst_ip: None,
                };
                Poll::Ready(Ok(1))
            }
            Poll::Ready(None) => Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "the gateway connection closed",
            ))),
            Poll::Pending => Poll::Pending,
        }
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok((Ipv4Addr::UNSPECIFIED, 0).into())
    }

    fn may_fragment(&self) -> bool {
        // Datagrams too large are dropped whole, never fragmented, so the
        // inner connection may discover the path's MTU.
        false
    }
}
