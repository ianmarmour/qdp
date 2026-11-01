//! Host and client identities, their key pins (Section 11.1), and keeping
//! them between runs.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use ring::digest;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::TransportError;

/// The length of a key pin: a SHA-256 digest.
const PIN_LENGTH: usize = 32;

/// Permissions of a kept private key on Unix: readable by its owner only.
#[cfg(unix)]
const KEY_FILE_MODE: u32 = 0o600;

/// The files an identity is kept in, in a directory of its own, and the name
/// its self-signed certificate is given, which is not checked: the key is
/// pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct IdentityFiles {
    /// The file of the DER-encoded certificate.
    pub certificate: &'static str,
    /// The file of the DER-encoded PKCS #8 private key.
    pub private_key: &'static str,
    /// The certificate's name.
    pub name: &'static str,
}

impl IdentityFiles {
    /// A host's identity, which clients pin.
    pub const HOST: Self = Self {
        certificate: "host-certificate.der",
        private_key: "host-key.der",
        name: "qdp-host",
    };

    /// A client's identity, which hosts accept by its key (Section 4.4.2).
    pub const CLIENT: Self = Self {
        certificate: "client-certificate.der",
        private_key: "client-key.der",
        name: "qdp-client",
    };
}

/// Where a kept identity came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdentityOrigin {
    /// It was kept from an earlier run.
    Loaded,
    /// It was generated now, and kept.
    Generated,
}

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

    /// Reads a pin as `qdp pin` prints it: 64 hexadecimal digits, in either
    /// case, optionally with colons between them. Returns `None` for anything
    /// else.
    pub fn from_hex(text: &str) -> Option<Self> {
        let digits: Vec<u8> = text.bytes().filter(|&byte| byte != b':').collect();
        if digits.len() != PIN_LENGTH * 2 || !digits.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        let mut pin = [0; PIN_LENGTH];
        for (byte, pair) in pin.iter_mut().zip(digits.chunks_exact(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        }
        Some(Self(pin))
    }

    /// Returns the pin as 64 lowercase hexadecimal digits.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
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

/// The certificate chain and private key an endpoint presents in the TLS
/// handshake: a host's, or a client's for the `CLIENT_CERTIFICATE` method
/// (Section 4.4.2).
pub struct TlsIdentity {
    /// The certificate chain, end-entity certificate first.
    pub certificate_chain: Vec<CertificateDer<'static>>,
    /// The private key of the end-entity certificate.
    pub private_key: PrivateKeyDer<'static>,
}

/// Hides the private key.
impl fmt::Debug for TlsIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TlsIdentity")
            .field("certificate_chain", &self.certificate_chain.len())
            .finish_non_exhaustive()
    }
}

impl TlsIdentity {
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
    /// PKCS #8 private key, as [`TlsIdentity::to_der`] returns them.
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

    /// Returns the identity kept in `directory`, in `files`, generating and
    /// keeping a new one if there is none; the private key is readable only
    /// by its owner.
    pub fn load_or_generate(
        directory: &Path,
        files: IdentityFiles,
    ) -> io::Result<(Self, IdentityOrigin)> {
        let certificate_path = directory.join(files.certificate);
        let key_path = directory.join(files.private_key);
        if certificate_path.exists() && key_path.exists() {
            let identity = Self::from_der(fs::read(&certificate_path)?, fs::read(&key_path)?)
                .map_err(io::Error::other)?;
            return Ok((identity, IdentityOrigin::Loaded));
        }
        let identity =
            Self::generate_self_signed(vec![files.name.to_owned()]).map_err(io::Error::other)?;
        let (certificate, key) = identity.to_der().map_err(io::Error::other)?;
        fs::create_dir_all(directory)?;
        write_private(&key_path, &key)?;
        fs::write(&certificate_path, certificate)?;
        Ok((identity, IdentityOrigin::Generated))
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

/// Writes a new file that only its owner can read.
fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write as _;

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(KEY_FILE_MODE);
    }
    options.open(path)?.write_all(contents)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_and_writes_pins_in_hex() {
        let text = "ab".repeat(PIN_LENGTH);
        let pin = KeyPin::from_hex(&text).unwrap();
        assert_eq!(pin, KeyPin([0xab; PIN_LENGTH]));
        assert_eq!(pin.to_hex(), text);
        assert_eq!(KeyPin::from_hex(&"AB:".repeat(PIN_LENGTH)), Some(pin));
        assert_eq!(KeyPin::from_hex("ab"), None);
        assert_eq!(KeyPin::from_hex(&"+a".repeat(PIN_LENGTH)), None);
    }

    use super::*;

    /// A new identity is kept and the same one is loaded on the next run;
    /// another kind in the same directory is another identity.
    #[test]
    fn keeps_identities() {
        let directory =
            std::env::temp_dir().join(format!("qdp-quic-identity-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let (first, origin) =
            TlsIdentity::load_or_generate(&directory, IdentityFiles::HOST).unwrap();
        assert_eq!(origin, IdentityOrigin::Generated);
        let (second, origin) =
            TlsIdentity::load_or_generate(&directory, IdentityFiles::HOST).unwrap();
        assert_eq!(origin, IdentityOrigin::Loaded);
        assert_eq!(first.key_pin().unwrap(), second.key_pin().unwrap());
        let (client, _) = TlsIdentity::load_or_generate(&directory, IdentityFiles::CLIENT).unwrap();
        assert_ne!(first.key_pin().unwrap(), client.key_pin().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(directory.join(IdentityFiles::HOST.private_key))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, KEY_FILE_MODE);
        }
        fs::remove_dir_all(&directory).unwrap();
    }

    /// A generated identity has a stable pin, distinct from another identity's.
    #[test]
    fn generated_identity_pins() {
        let first = TlsIdentity::generate_self_signed(vec!["host".to_owned()]).unwrap();
        let second = TlsIdentity::generate_self_signed(vec!["host".to_owned()]).unwrap();
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
        let identity = TlsIdentity::generate_self_signed(vec!["host".to_owned()]).unwrap();
        let (certificate, key) = identity.to_der().unwrap();
        let restored = TlsIdentity::from_der(certificate, key).unwrap();
        assert_eq!(restored.key_pin().unwrap(), identity.key_pin().unwrap());
        assert!(TlsIdentity::from_der(vec![1, 2, 3], vec![4]).is_err());
    }
}
