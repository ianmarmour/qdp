//! Storage of a host's or a client's identity between runs.

use std::io;
use std::path::Path;

use qdp_quic::{IdentityFiles, IdentityOrigin, TlsIdentity};
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
    /// Returns the files the role's identity is kept in.
    fn files(self) -> IdentityFiles {
        match self {
            Self::Host => IdentityFiles::HOST,
            Self::Client => IdentityFiles::CLIENT,
        }
    }
}

/// Returns the identity of `role` stored in `directory`, generating and
/// storing a new one if there is none.
pub fn load_or_create(directory: &Path, role: Role) -> io::Result<TlsIdentity> {
    let (identity, origin) = TlsIdentity::load_or_generate(directory, role.files())?;
    if origin == IdentityOrigin::Generated {
        info!(directory = %directory.display(), ?role, "generated a new identity");
    }
    Ok(identity)
}
