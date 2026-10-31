//! Storage of a host's or a client's identity between runs.

use std::fs;
use std::io;
use std::path::Path;

use qdp_quic::TlsIdentity;
use tracing::info;

/// Whose identity a directory holds, which names its files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// A host's, which clients pin.
    Host,
    /// A client's, which hosts accept by its key (Section 4.4.2).
    Client,
}

impl Role {
    /// Returns the file holding the DER-encoded certificate.
    fn certificate_file(self) -> &'static str {
        match self {
            Self::Host => "host-certificate.der",
            Self::Client => "client-certificate.der",
        }
    }

    /// Returns the file holding the DER-encoded PKCS #8 private key.
    fn key_file(self) -> &'static str {
        match self {
            Self::Host => "host-key.der",
            Self::Client => "client-key.der",
        }
    }

    /// Returns the name in the self-signed certificate, which is not
    /// checked: the key is pinned.
    fn certificate_name(self) -> &'static str {
        match self {
            Self::Host => "qdp-host",
            Self::Client => "qdp-client",
        }
    }
}

/// Permissions of the private key file on Unix: readable by its owner only.
#[cfg(unix)]
const KEY_FILE_MODE: u32 = 0o600;

/// Returns the identity of `role` stored in `directory`, generating and
/// storing a new one if there is none.
pub fn load_or_create(directory: &Path, role: Role) -> io::Result<TlsIdentity> {
    let certificate_path = directory.join(role.certificate_file());
    let key_path = directory.join(role.key_file());
    if certificate_path.exists() && key_path.exists() {
        let certificate = fs::read(&certificate_path)?;
        let key = fs::read(&key_path)?;
        return TlsIdentity::from_der(certificate, key).map_err(io::Error::other);
    }

    let identity = TlsIdentity::generate_self_signed(vec![role.certificate_name().to_owned()])
        .map_err(io::Error::other)?;
    let (certificate, key) = identity.to_der().map_err(io::Error::other)?;
    fs::create_dir_all(directory)?;
    write_private(&key_path, &key)?;
    fs::write(&certificate_path, certificate)?;
    info!(directory = %directory.display(), ?role, "generated a new identity");
    Ok(identity)
}

/// Writes a file that only its owner can read.
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
    use super::*;

    /// A new identity is stored and the same one is loaded on the next run.
    #[test]
    fn stores_and_reloads() {
        let directory =
            std::env::temp_dir().join(format!("qdp-identity-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        let first = load_or_create(&directory, Role::Host).unwrap();
        let second = load_or_create(&directory, Role::Host).unwrap();
        assert_eq!(first.key_pin().unwrap(), second.key_pin().unwrap());
        // A client's identity in the same directory is a different one.
        let client = load_or_create(&directory, Role::Client).unwrap();
        assert_ne!(first.key_pin().unwrap(), client.key_pin().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(directory.join(Role::Host.key_file()))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, KEY_FILE_MODE);
        }
        fs::remove_dir_all(&directory).unwrap();
    }
}
