//! Host identities and key pins (Section 11.1).

use std::fmt;

use ring::digest;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::TransportError;

/// The length of a key pin: a SHA-256 digest.
const PIN_LENGTH: usize = 32;

/// The SHA-256 hash of a host certificate's SubjectPublicKeyInfo, which a client
/// that pins the host's key compares with the certificate it is shown
/// (Section 11.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyPin(pub [u8; PIN_LENGTH]);

impl KeyPin {
    /// Returns the pin of a DER-encoded SubjectPublicKeyInfo.
    pub fn of_spki(spki: &[u8]) -> Self {
        let mut pin = [0; PIN_LENGTH];
        pin.copy_from_slice(digest::digest(&digest::SHA256, spki).as_ref());
        Self(pin)
    }

    /// Returns the pin of a DER-encoded certificate.
    pub fn of_certificate(certificate: &CertificateDer<'_>) -> Result<Self, TransportError> {
        let parsed = rustls::server::ParsedCertificate::try_from(certificate)?;
        Ok(Self::of_spki(parsed.subject_public_key_info().as_ref()))
    }
}

/// Shows the pin in hexadecimal, as a fingerprint a user can compare.
impl fmt::Debug for KeyPin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "KeyPin(")?;
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        write!(formatter, ")")
    }
}

/// The certificate chain and private key a host presents in the TLS handshake.
pub struct HostIdentity {
    /// The certificate chain, end-entity certificate first.
    pub certificate_chain: Vec<CertificateDer<'static>>,
    /// The private key of the end-entity certificate.
    pub private_key: PrivateKeyDer<'static>,
}

/// Hides the private key.
impl fmt::Debug for HostIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostIdentity")
            .field("certificate_chain", &self.certificate_chain.len())
            .finish_non_exhaustive()
    }
}

impl HostIdentity {
    /// Generates a self-signed identity valid for `names`, for hosts whose clients
    /// pin their key rather than use public key infrastructure.
    pub fn generate_self_signed(names: Vec<String>) -> Result<Self, TransportError> {
        let certified = rcgen::generate_simple_self_signed(names)?;
        let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            certified.signing_key.serialize_der(),
        ));
        Ok(Self {
            certificate_chain: vec![certified.cert.der().clone()],
            private_key,
        })
    }

    /// Returns the pin of the identity's end-entity certificate, for a client to
    /// pin.
    pub fn key_pin(&self) -> Result<KeyPin, TransportError> {
        let certificate = self
            .certificate_chain
            .first()
            .ok_or(TransportError::Config("the certificate chain is empty"))?;
        KeyPin::of_certificate(certificate)
    }

    /// Returns a copy of the identity, for building more than one TLS
    /// configuration.
    pub fn clone_key(&self) -> Self {
        Self {
            certificate_chain: self.certificate_chain.clone(),
            private_key: self.private_key.clone_key(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generated identity has a stable pin, distinct from another identity's.
    #[test]
    fn generated_identity_pins() {
        let first = HostIdentity::generate_self_signed(vec!["host".to_owned()]).unwrap();
        let second = HostIdentity::generate_self_signed(vec!["host".to_owned()]).unwrap();
        assert_eq!(first.key_pin().unwrap(), first.key_pin().unwrap());
        assert_ne!(first.key_pin().unwrap(), second.key_pin().unwrap());
        assert!(!format!("{first:?}").contains("private_key"));
        assert_eq!(
            format!("{:?}", KeyPin([0xab; 32])).len(),
            "KeyPin()".len() + 64
        );
    }
}
