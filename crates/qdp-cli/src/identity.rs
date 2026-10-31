//! Storage of the host's identity between runs.

use std::fs;
use std::io;
use std::path::Path;

use qdp_quic::HostIdentity;
use tracing::info;

/// The file holding the host's DER-encoded certificate.
const CERTIFICATE_FILE: &str = "host-certificate.der";

/// The file holding the host's DER-encoded PKCS #8 private key.
const KEY_FILE: &str = "host-key.der";

/// The name in the host's self-signed certificate. Clients pin the key, so the
/// name is not checked.
const CERTIFICATE_NAME: &str = "qdp-host";

/// Permissions of the private key file on Unix: readable by its owner only.
#[cfg(unix)]
const KEY_FILE_MODE: u32 = 0o600;

/// Returns the identity stored in `directory`, generating and storing a new one
/// if there is none.
pub fn load_or_create(directory: &Path) -> io::Result<HostIdentity> {
    let certificate_path = directory.join(CERTIFICATE_FILE);
    let key_path = directory.join(KEY_FILE);
    if certificate_path.exists() && key_path.exists() {
        let certificate = fs::read(&certificate_path)?;
        let key = fs::read(&key_path)?;
        return HostIdentity::from_der(certificate, key).map_err(io::Error::other);
    }

    let identity = HostIdentity::generate_self_signed(vec![CERTIFICATE_NAME.to_owned()])
        .map_err(io::Error::other)?;
    let (certificate, key) = identity.to_der().map_err(io::Error::other)?;
    fs::create_dir_all(directory)?;
    write_private(&key_path, &key)?;
    fs::write(&certificate_path, certificate)?;
    info!(directory = %directory.display(), "generated a new host identity");
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
        let first = load_or_create(&directory).unwrap();
        let second = load_or_create(&directory).unwrap();
        assert_eq!(first.key_pin().unwrap(), second.key_pin().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = fs::metadata(directory.join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, KEY_FILE_MODE);
        }
        fs::remove_dir_all(&directory).unwrap();
    }
}
