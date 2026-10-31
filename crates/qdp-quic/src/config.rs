//! TLS and QUIC configuration for QDP (Sections 3.1 to 3.4).

use std::sync::Arc;
use std::time::Duration;

use quinn::congestion::BbrConfig;
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use quinn::{ClientConfig, IdleTimeout, ServerConfig, TransportConfig, VarInt};
use rustls::client::Resumption;
use rustls::crypto::CryptoProvider;
use rustls::server::NoServerSessionStorage;

use crate::{HostIdentity, KeyPin, PinnedKeyVerifier, TransportError};

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

/// The number of bidirectional streams the host lets the client open: only the
/// control stream (Section 3.5.1).
const HOST_BIDIRECTIONAL_STREAMS: u32 = 1;

/// The number of bidirectional streams the client lets the host open: none.
const CLIENT_BIDIRECTIONAL_STREAMS: u32 = 0;

/// How the client verifies the host's certificate (Section 11.1).
#[derive(Debug, Clone)]
pub enum HostVerification {
    /// The host's key must hash to one of these pins.
    PinnedKeys(Vec<KeyPin>),
    /// The host's certificate must chain to one of these roots and be valid for
    /// the server name.
    WebPki(Arc<rustls::RootCertStore>),
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
    // recommends.
    config.congestion_controller_factory(Arc::new(BbrConfig::default()));
    config
}

/// Returns the QUIC configuration of a host presenting `identity`.
///
/// TLS 1.3 only, ALPN `qdp-draft-00`, no session tickets and no 0-RTT
/// (Section 3.4).
pub fn host_config(identity: HostIdentity) -> Result<ServerConfig, TransportError> {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(identity.certificate_chain, identity.private_key)?;
    tls.alpn_protocols = vec![qdp::ALPN.to_vec()];
    tls.max_early_data_size = 0;
    tls.send_tls13_tickets = 0;
    tls.session_storage = Arc::new(NoServerSessionStorage {});
    let crypto = QuicServerConfig::try_from(tls)
        .map_err(|_| TransportError::Config("no QUIC-compatible cipher suite"))?;
    let mut config = ServerConfig::with_crypto(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config(HOST_BIDIRECTIONAL_STREAMS)));
    Ok(config)
}

/// Returns the QUIC configuration of a client that verifies the host with
/// `verification`.
pub fn client_config(verification: HostVerification) -> Result<ClientConfig, TransportError> {
    let provider = provider();
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])?;
    let mut tls = match verification {
        HostVerification::PinnedKeys(pins) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(PinnedKeyVerifier::new(pins, provider)))
            .with_no_client_auth(),
        HostVerification::WebPki(roots) => {
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    };
    tls.alpn_protocols = vec![qdp::ALPN.to_vec()];
    tls.resumption = Resumption::disabled();
    tls.enable_early_data = false;
    let crypto = QuicClientConfig::try_from(tls)
        .map_err(|_| TransportError::Config("no QUIC-compatible cipher suite"))?;
    let mut config = ClientConfig::new(Arc::new(crypto));
    config.transport_config(Arc::new(transport_config(CLIENT_BIDIRECTIONAL_STREAMS)));
    Ok(config)
}
