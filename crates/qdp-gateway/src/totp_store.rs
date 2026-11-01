//! The users' TOTP secrets (Section 4.4.10), and the last step each user's
//! codes were accepted for, so that no code is accepted twice. They are kept
//! in a file only the gateway can read, a line a user: the secret in base32,
//! a tab, the last step or `-`, a tab, and the user's name, lowercase.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use qdp::auth::totp::TotpSecret;

/// The file of secrets in the state directory.
const SECRETS_FILE: &str = "totp-secrets";

/// The field of a user who has no accepted step yet.
const NO_STEP: &str = "-";

/// A user's TOTP secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TotpEntry {
    /// The secret.
    pub secret: TotpSecret,
    /// The step of the last code accepted.
    pub last_step: Option<u64>,
}

/// The secrets of every enrolled user.
#[derive(Debug)]
pub struct TotpStore {
    /// The file they are kept in.
    path: PathBuf,
    /// The secrets, by lowercase user name.
    entries: BTreeMap<String, TotpEntry>,
}

impl TotpStore {
    /// Reads the secrets kept in `state_directory`, none if there is no file.
    pub fn load(state_directory: &Path) -> io::Result<Self> {
        let path = state_directory.join(SECRETS_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error),
        };
        let mut entries = BTreeMap::new();
        for (number, line) in text.lines().enumerate() {
            let mut fields = line.splitn(3, '\t');
            let (Some(secret), Some(step), Some(user)) =
                (fields.next(), fields.next(), fields.next())
            else {
                return Err(malformed(&path, number));
            };
            let secret = TotpSecret::from_base32(secret).map_err(|_| malformed(&path, number))?;
            let last_step = match step {
                NO_STEP => None,
                step => Some(step.parse().map_err(|_| malformed(&path, number))?),
            };
            entries.insert(user.to_owned(), TotpEntry { secret, last_step });
        }
        Ok(Self { path, entries })
    }

    /// Returns `user`'s secret, if they enrolled one.
    pub fn get(&self, user: &str) -> Option<&TotpEntry> {
        self.entries.get(&user.to_lowercase())
    }

    /// Returns the enrolled users' names.
    pub fn users(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Keeps `secret` as `user`'s, replacing any they had, and saves.
    pub fn set(&mut self, user: &str, secret: TotpSecret) -> io::Result<()> {
        self.entries.insert(
            user.to_lowercase(),
            TotpEntry {
                secret,
                last_step: None,
            },
        );
        self.save()
    }

    /// Notes that `user`'s code of `step` was accepted, and saves.
    pub fn accept(&mut self, user: &str, step: u64) -> io::Result<()> {
        if let Some(entry) = self.entries.get_mut(&user.to_lowercase()) {
            entry.last_step = Some(step);
        }
        self.save()
    }

    /// Forgets `user`'s secret, as when an administrator resets it, and saves.
    /// Returns whether they had one.
    pub fn remove(&mut self, user: &str) -> io::Result<bool> {
        let removed = self.entries.remove(&user.to_lowercase()).is_some();
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// Writes the file, readable only by the gateway, replacing the old one at
    /// once so that a crash leaves one or the other.
    fn save(&self) -> io::Result<()> {
        let mut text = String::new();
        for (user, entry) in &self.entries {
            let step = entry
                .last_step
                .map_or_else(|| NO_STEP.to_owned(), |step| step.to_string());
            text.push_str(&format!("{}\t{step}\t{user}\n", entry.secret.to_base32()));
        }
        if let Some(directory) = self.path.parent() {
            fs::create_dir_all(directory)?;
        }
        let partial = self.path.with_extension("partial");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&partial)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(partial, &self.path)
    }
}

/// Returns the error of a line that is not an entry.
fn malformed(path: &Path, line: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{} line {} is not a TOTP entry", path.display(), line + 1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_secrets_and_steps() {
        let directory =
            std::env::temp_dir().join(format!("qdp-gateway-totp-{}", std::process::id()));
        let mut store = TotpStore::load(&directory).unwrap();
        assert!(store.get("alice").is_none());
        let secret = TotpSecret::generate().unwrap();
        store.set("Alice", secret.clone()).unwrap();
        store
            .set("bob smith", TotpSecret::generate().unwrap())
            .unwrap();
        store.accept("ALICE", 59_000_000).unwrap();
        let reloaded = TotpStore::load(&directory).unwrap();
        let alice = reloaded.get("alice").unwrap();
        assert_eq!(alice.secret, secret);
        assert_eq!(alice.last_step, Some(59_000_000));
        assert_eq!(reloaded.get("Bob Smith").unwrap().last_step, None);
        assert_eq!(reloaded.users().collect::<Vec<_>>(), ["alice", "bob smith"]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(directory.join(SECRETS_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let mut store = reloaded;
        assert!(store.remove("alice").unwrap());
        assert!(!store.remove("alice").unwrap());
        assert!(TotpStore::load(&directory).unwrap().get("alice").is_none());
        fs::write(directory.join(SECRETS_FILE), "not\tan entry\n").unwrap();
        assert!(TotpStore::load(&directory).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
