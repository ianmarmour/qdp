//! Verification of the host's certificate by a pinned key (Section 11.1).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, Error, PeerIncompatible, SignatureScheme};

use crate::KeyPin;

/// Accepts a host certificate whose SubjectPublicKeyInfo hashes to one of the
/// pinned values, and checks that the host holds the matching private key.
///
/// Names, validity dates and issuers are not checked: the pin, obtained in
/// advance from a broker or an administrator, is the whole of the trust decision.
#[derive(Debug)]
pub struct PinnedKeyVerifier {
    /// The accepted pins.
    pins: Vec<KeyPin>,
    /// The cryptographic provider whose signature algorithms verify the
    /// handshake.
    provider: Arc<CryptoProvider>,
}

impl PinnedKeyVerifier {
    /// Creates a verifier that accepts any of `pins`.
    pub fn new(pins: Vec<KeyPin>, provider: Arc<CryptoProvider>) -> Self {
        Self { pins, provider }
    }
}

impl ServerCertVerifier for PinnedKeyVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        let pin = KeyPin::of_certificate(end_entity)
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        if self.pins.contains(&pin) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::InvalidCertificate(
                CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        // QUIC requires TLS 1.3 (RFC 9001, Section 4.2).
        Err(Error::PeerIncompatible(
            PeerIncompatible::Tls13RequiredForQuic,
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
