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

    /// Creates an identity from a DER-encoded certificate and a DER-encoded
    /// PKCS #8 private key, as [`HostIdentity::to_der`] returns them.
    pub fn from_der(certificate: Vec<u8>, private_key: Vec<u8>) -> Result<Self, TransportError> {
        let identity = Self {
            certificate_chain: vec![CertificateDer::from(certificate)],
            private_key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key)),
        };
        // Parsing the pin checks that the certificate is well formed.
        identity.key_pin()?;
        Ok(identity)
    }

    /// Returns the end-entity certificate and the private key, DER-encoded, for
    /// storing the identity. The key is PKCS #8 for generated identities.
    pub fn to_der(&self) -> Result<(Vec<u8>, Vec<u8>), TransportError> {
        let certificate = self
            .certificate_chain
            .first()
            .ok_or(TransportError::Config("the certificate chain is empty"))?;
        Ok((
            certificate.as_ref().to_vec(),
            self.private_key.secret_der().to_vec(),
        ))
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

    /// An identity survives storage as DER.
    #[test]
    fn der_round_trip() {
        let identity = HostIdentity::generate_self_signed(vec!["host".to_owned()]).unwrap();
        let (certificate, key) = identity.to_der().unwrap();
        let restored = HostIdentity::from_der(certificate, key).unwrap();
        assert_eq!(restored.key_pin().unwrap(), identity.key_pin().unwrap());
        assert!(HostIdentity::from_der(vec![1, 2, 3], vec![4]).is_err());
    }
}
