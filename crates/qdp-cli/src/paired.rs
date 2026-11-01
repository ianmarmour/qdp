//! What pairing leaves behind (Section 4.4.9): on a host, the clients that
//! paired, whose keys it accepts; on a client, the hosts it paired with,
//! whose keys it pins. Each is a text file in the identity directory, one
//! entry a line: a key pin in hexadecimal, a space, and the device's name or
//! the host's address.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use qdp_quic::KeyPin;

use crate::args::{format_pin, parse_pin};

/// The file of the clients a host paired with, in its identity directory.
const PAIRED_CLIENTS_FILE: &str = "paired-clients";

/// The file of the hosts a client paired with, in its identity directory.
const PAIRED_HOSTS_FILE: &str = "paired-hosts";

/// A paired device: its key and the text kept with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    /// The device's key pin.
    pub pin: KeyPin,
    /// The client's device name, or the host's address.
    pub label: String,
}

/// A file of paired devices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedFile {
    /// The file's path.
    path: PathBuf,
}

impl PairedFile {
    /// Returns the clients a host whose identity is in `directory` paired
    /// with.
    pub fn clients(directory: &Path) -> Self {
        Self {
            path: directory.join(PAIRED_CLIENTS_FILE),
        }
    }

    /// Returns the hosts a client whose identity is in `directory` paired
    /// with.
    pub fn hosts(directory: &Path) -> Self {
        Self {
            path: directory.join(PAIRED_HOSTS_FILE),
        }
    }

    /// Returns the paired devices, none if the file does not exist yet.
    /// Lines that are not entries are skipped.
    pub fn load(&self) -> io::Result<Vec<Paired>> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        Ok(text.lines().filter_map(parse_entry).collect())
    }

    /// Adds `paired`, replacing an entry with its key or its label.
    pub fn add(&self, paired: &Paired) -> io::Result<()> {
        let mut entries = self.load()?;
        entries.retain(|entry| entry.pin != paired.pin && entry.label != paired.label);
        entries.push(paired.clone());
        self.save(&entries)
    }

    /// Removes the entries whose label is `which`, or whose pin starts with
    /// it, and returns them.
    pub fn remove(&self, which: &str) -> io::Result<Vec<Paired>> {
        let which_pin = which.to_ascii_lowercase().replace(':', "");
        let (removed, kept): (Vec<_>, Vec<_>) = self.load()?.into_iter().partition(|entry| {
            entry.label == which
                || (!which_pin.is_empty() && format_pin(entry.pin).starts_with(&which_pin))
        });
        if !removed.is_empty() {
            self.save(&kept)?;
        }
        Ok(removed)
    }

    /// Returns the entry whose label is `label`, if any.
    pub fn find(&self, label: &str) -> io::Result<Option<Paired>> {
        Ok(self.load()?.into_iter().find(|entry| entry.label == label))
    }

    /// Writes `entries`, readable only by the user.
    fn save(&self, entries: &[Paired]) -> io::Result<()> {
        if let Some(directory) = self.path.parent() {
            fs::create_dir_all(directory)?;
        }
        let mut text = String::new();
        for entry in entries {
            text.push_str(&format!("{} {}\n", format_pin(entry.pin), entry.label));
        }
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        options.open(&self.path)?.write_all(text.as_bytes())
    }
}

/// Reads one line of a file of paired devices.
fn parse_entry(line: &str) -> Option<Paired> {
    let (pin, label) = line.trim_end().split_once(' ')?;
    Some(Paired {
        pin: parse_pin(pin).ok()?,
        label: label.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a fresh directory for a test.
    fn directory(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("qdp-paired-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        directory
    }

    /// Entries are added, replaced by key or label, found, and removed by
    /// label or a prefix of their pin.
    #[test]
    fn keeps_paired_devices() {
        let directory = directory("keeps");
        let file = PairedFile::clients(&directory);
        assert_eq!(file.load().unwrap(), []);
        let thor = Paired {
            pin: KeyPin([0xab; 32]),
            label: "AYN Thor".to_owned(),
        };
        let tablet = Paired {
            pin: KeyPin([0x12; 32]),
            label: "Living room tablet".to_owned(),
        };
        file.add(&thor).unwrap();
        file.add(&tablet).unwrap();
        let renamed = Paired {
            label: "Thor".to_owned(),
            ..thor.clone()
        };
        file.add(&renamed).unwrap();
        assert_eq!(file.load().unwrap(), [tablet.clone(), renamed.clone()]);
        assert_eq!(file.find("Thor").unwrap(), Some(renamed.clone()));
        assert_eq!(file.remove("AB:AB").unwrap(), [renamed]);
        assert_eq!(file.remove("Living room tablet").unwrap(), [tablet]);
        assert_eq!(file.load().unwrap(), []);
        assert_eq!(file.remove("nothing").unwrap(), []);
        fs::remove_dir_all(&directory).unwrap();
    }
}
