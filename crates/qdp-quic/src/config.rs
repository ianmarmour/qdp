//! TLS and QUIC configuration for QDP (Sections 3.1 to 3.4).

use std::sync::Arc;
use std::time::Duration;

use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, IdleTimeout, ServerConfig, TransportConfig, VarInt};
use rustls::client::Resumption;
use rustls::crypto::CryptoProvider;
use rustls::server::NoServerSessionStorage;

use crate::congestion::FlooredBbrConfig;

use crate::verify::ClientKeyVerifier;
use crate::{KeyPin, PinnedKeyVerifier, TlsIdentity, TransportError};

/// The `max_datagram_frame_size` an endpoint advertises: the recommended 65535
/// (Section 3.3). quinn advertises its datagram receive buffer size, capped at
/// this value.
const MAX_DATAGRAM_FRAME_SIZE: usize = 65535;

/// The datagram receive buffer, which holds several frames' worth of fragments.
const DATAGRAM_RECEIVE_BUFFER: usize = 8 << 20;

/// The datagram send buffer. Senders should keep far less than this queued:
/// see [`QdpConnection::queued_datagram_bytes`](crate::QdpConnection::queued_datagram_bytes).
pub(crate) const DATAGRAM_SEND_BUFFER: usize = 8 << 20;

/// The QUIC idle timeout (Section 3.3).
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an endpoint waits before sending a keep-alive PING (Section 3.3).
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(10);

/// The number of unidirectional streams each endpoint lets its peer open
/// (Section 3.3).
const UNIDIRECTIONAL_STREAMS: u32 = 64;

/// The number of bidirectional streams the host lets the client open: the
/// control stream, and one more, so that a client opening another is seen
/// and closed with `QDP_STREAM_CREATION_ERROR` rather than a QUIC stream
/// limit error (Section 3.5.1).
const HOST_BIDIRECTIONAL_STREAMS: u32 = 2;

/// The number of bidirectional streams the client lets the host open: one,
/// only so that a host opening it is seen and closed with
/// `QDP_STREAM_CREATION_ERROR`.
const CLIENT_BIDIRECTIONAL_STREAMS: u32 = 1;

/// The UDP payload size a gateway connection starts with: enough for
/// DATAGRAM frames with the 1200-byte payloads of a tunnelled connection, and
/// what every IPv6 path carries (Section 13.2). Path MTU discovery raises it
/// where the path allows.
const GATEWAY_INITIAL_MTU: u16 = 1232;

/// The number of bidirectional streams either end of a gateway connection
/// lets the other open: the client's control stream (Section 13.3).
const GATEWAY_BIDIRECTIONAL_STREAMS: u32 = 1;

/// How the client verifies the host's certificate (Section 11.1).
#[derive(Debug, Clone)]
pub enum HostVerification {
    /// The host's key must hash to one of these pins.
    PinnedKeys(Vec<KeyPin>),
    /// The host's certificate must chain to one of these roots and be valid for
    /// the server name.
    WebPki(Arc<rustls::RootCertStore>),
    /// Any key the host proves it holds, which the client then authenticates
    /// by pairing (Section 4.4.9). Only for a session that authenticates with
    /// `PAIRING`, and pins the key once that succeeds.
    Pairing,
}

/// Returns the cryptographic provider QDP uses: ring, with TLS 1.3 only.
fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// Returns the transport parameters both endpoints use (Section 3.3).
fn transport_config(bidirectional_streams: u32) -> TransportConfig {
    let mut config = TransportConfig::default();
    config
        .datagram_receive_buffer_size(Some(DATAGRAM_RECEIVE_BUFFER.max(MAX_DATAGRAM_FRAME_SIZE)))
        .datagram_send_buffer_size(DATAGRAM_SEND_BUFFER)
        .max_idle_timeout(IdleTimeout::try_from(IDLE_TIMEOUT).ok())
        .keep_alive_interval(Some(KEEP_ALIVE_INTERVAL))
        .max_concurrent_uni_streams(VarInt::from_u32(UNIDIRECTIONAL_STREAMS))
        .max_concurrent_bidi_streams(VarInt::from_u32(bidirectional_streams));
    // BBR sizes the congestion window from the measured bandwidth and RTT. The
    // default, CUBIC, shrinks it on every loss and only grows it back while the
    // window is full, which a video stream rarely fills: on Wi-Fi, with random
    // loss, its window ratchets down to a few packets and large frames then
    // take many round trips. BBR also keeps queues short, as Section 5.5.1
    // recommends. On a short path its window has a floor, which lets a large
    // frame leave at once after the stream sent little.
    config.congestion_controller_factory(Arc::new(FlooredBbrConfig::default()));
    config
}

/// Returns the transport parameters of a gateway connection (Section 13.2):
/// QDP's, with only the control stream, and a first packet size that carries
/// a tunnelled connection's packets.
fn gateway_transport_config() -> TransportConfig {
    let mut config = transport_config(GATEWAY_BIDIRECTIONAL_STREAMS);
    config
        .max_concurrent_uni_streams(VarInt::from_u32(0))
        .initial_mtu(GATEWAY_INITIAL_MTU);
    config
}

/// Returns the QUIC configuration of a host presenting `identity`.
///
/// TLS 1.3 only, ALPN `qdp-draft-00`, no session tickets and no 0-RTT
/// (Section 3.4). The host asks every client for a certificate, which it
/// may decline, so that it can authorize clients by their keys (Section
/// 4.4.2).
pub fn host_config(identity: TlsIdentity) -> Result<ServerConfig, TransportError> {
    server_config(
        identity,
        qdp::ALPN,
        transport_config(HOST_BIDIRECTIONAL_STREAMS),
    )
}

/// Returns the QUIC configuration of a gateway presenting `identity`
/// (Section 13.2): as a host's, with the gateway protocol's ALPN, asking
/// clients for the certificate that identity assertions name.
pub fn gateway_server_config(identity: TlsIdentity) -> Result<ServerConfig, TransportError> {
    server_config(
        identity,
        qdp::gateway::GATEWAY_ALPN,
        gateway_transport_config(),
    )
}

/// Returns the configuration of a server presenting `identity`, speaking
/// `alpn`, with `transport`.
fn server_config(
    identity: TlsIdentity,
    alpn: &[u8],
    transport: TransportConfig,
) -> Result<ServerConfig, TransportError> {
    let provider = provider();
    let mut tls = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(ClientKeyVerifier::new(provider)))
        .with_single_cert(identity.certificate_chain, identity.private_key)?;
    tls.alpn_protocols = vec![alpn.to_vec()];
    tls.max_early_data_size = 0;
    tls.send_tls13_tickets = 0;
    tls.session_storage = Arc::new(NoServerSessionStorage {});
    let crypto = QuicServerConfig::try_from(tls)
        .map_err(|_| TransportError::Config("no QUIC-compatible cipher suite"))?;
    let mut config = ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// Returns the QUIC configuration of a client that verifies the host with
/// `verification`, and presents `identity`, if given, when the host asks
/// for a certificate.
pub fn client_config(
    verification: HostVerification,
    identity: Option<TlsIdentity>,
) -> Result<ClientConfig, TransportError> {
    connecting_config(
        verification,
        identity,
        qdp::ALPN,
        transport_config(CLIENT_BIDIRECTIONAL_STREAMS),
    )
}

/// Returns the QUIC configuration of a client of a gateway, which verifies
/// the gateway with `verification` and presents `identity`, if given, for
/// identity assertions to name (Section 13.2). The gateway is sent the
/// user's password, so `verification` must not be
/// [`HostVerification::Pairing`].
pub fn gateway_client_config(
    verification: HostVerification,
    identity: Option<TlsIdentity>,
) -> Result<ClientConfig, TransportError> {
    if matches!(verification, HostVerification::Pairing) {
        return Err(TransportError::Config(
            "a gateway must be verified by its pin or a certificate authority",
        ));
    }
    connecting_config(
        verification,
        identity,
        qdp::gateway::GATEWAY_ALPN,
        gateway_transport_config(),
    )
}

/// Returns the configuration of a client that verifies the server with
/// `verification`, presents `identity`, speaks `alpn` and uses `transport`.
fn connecting_config(
    verification: HostVerification,
    identity: Option<TlsIdentity>,
    alpn: &[u8],
    transport: TransportConfig,
) -> Result<ClientConfig, TransportError> {
    let provider = provider();
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?;
    let builder = match verification {
        HostVerification::PinnedKeys(pins) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedKeyVerifier::new(pins, provider))),
        HostVerification::WebPki(roots) => builder.with_root_certificates(roots),
        HostVerification::Pairing => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedKeyVerifier::for_pairing(provider))),
    };
    let mut tls = match identity {
        Some(identity) => {
            builder.with_client_auth_cert(identity.certificate_chain, identity.private_key)?
        }
        None => builder.with_no_client_auth(),
    };
    tls.alpn_protocols = vec![alpn.to_vec()];
    tls.resumption = Resumption::disabled();
    tls.enable_early_data = false;
    let crypto = QuicClientConfig::try_from(tls)
        .map_err(|_| TransportError::Config("no QUIC-compatible cipher suite"))?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport));
    Ok(config)
}
