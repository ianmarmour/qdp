//! The gateway's keys: its TLS identity, which clients pin or check against
//! the organization's certificate authority (Section 13.2), and its Ed25519
//! assertion key, which hosts trust to vouch for users (Section 13.7). Both
//! are kept in the state directory, readable only by the gateway, unless the
//! configuration names a certificate from the organization.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use qdp::gateway::assertion::PUBLIC_KEY_LENGTH;
use qdp_quic::{IdentityFiles, KeyPin, TlsIdentity};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};

use crate::config::Config;

/// The file of the assertion key, PKCS #8, in the state directory.
const ASSERTION_KEY_FILE: &str = "assertion-key.pk8";

/// The gateway's keys.
pub struct GatewayKeys {
    /// The TLS identity it presents.
    pub tls: TlsIdentity,
    /// The key it signs identity assertions with.
    pub assertion: Ed25519KeyPair,
}

impl GatewayKeys {
    /// Loads the keys `config` names, making and keeping those the gateway
    /// makes itself the first time.
    pub fn load(config: &Config) -> io::Result<Self> {
        let tls = match &config.tls {
            Some(tls) => {
                let certificate_chain = CertificateDer::pem_file_iter(&tls.certificate)
                    .and_then(Iterator::collect::<Result<Vec<_>, _>>)
                    .map_err(|error| pem_error(&tls.certificate, &error))?;
                let private_key = PrivateKeyDer::from_pem_file(&tls.private_key)
                    .map_err(|error| pem_error(&tls.private_key, &error))?;
                TlsIdentity {
                    certificate_chain,
                    private_key,
                }
            }
            None => {
                TlsIdentity::load_or_generate(&config.state_directory, IdentityFiles::GATEWAY)?.0
            }
        };
        tls.key_pin().map_err(io::Error::other)?;
        let assertion = load_or_generate_assertion_key(&config.state_directory)?;
        Ok(Self { tls, assertion })
    }

    /// Returns the pin of the gateway's TLS key, which clients pin.
    pub fn tls_pin(&self) -> KeyPin {
        self.tls.key_pin().unwrap_or(KeyPin([0; 32]))
    }

    /// Returns the public half of the assertion key, which hosts trust.
    pub fn assertion_public_key(&self) -> [u8; PUBLIC_KEY_LENGTH] {
        let mut key = [0; PUBLIC_KEY_LENGTH];
        key.copy_from_slice(self.assertion.public_key().as_ref());
        key
    }
}

/// Returns the error of a PEM file that cannot be read.
fn pem_error(path: &Path, error: &impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("cannot read {}: {error}", path.display()),
    )
}

/// Returns the assertion key kept in `directory`, making and keeping one if
/// there is none.
fn load_or_generate_assertion_key(directory: &Path) -> io::Result<Ed25519KeyPair> {
    let path = directory.join(ASSERTION_KEY_FILE);
    let document = match fs::read(&path) {
        Ok(document) => document,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
                .map_err(|_| io::Error::other("cannot generate the assertion key"))?;
            fs::create_dir_all(directory)?;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(&path)?.write_all(document.as_ref())?;
            document.as_ref().to_vec()
        }
        Err(error) => return Err(error),
    };
    Ed25519KeyPair::from_pkcs8(&document).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} is not an Ed25519 key", path.display()),
        )
    })
}
