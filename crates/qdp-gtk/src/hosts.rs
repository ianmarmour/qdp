//! The hosts paired with: their keys and addresses, which `qdp` keeps in the
//! client identity's `paired-hosts` file as it pairs (see the qdp-cli
//! crate's `paired` module), and the names the app knew them by, which it
//! keeps beside the identity in a file of the same form: a line a host, its
//! key pin, a space and its name.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::key_pin::KeyPin;

/// The directory of the client's identity, in the app's directory.
const IDENTITY_DIRECTORY: &str = "client-identity";

/// The file in the identity directory of the hosts `qdp` paired with.
const PAIRED_HOSTS_FILE: &str = "paired-hosts";

/// The file in the app's directory of the paired hosts' names.
const NAMES_FILE: &str = "host-names";

/// A host paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedHost {
    /// Its key pin, which `qdp connect` verifies it by.
    pub pin: KeyPin,
    /// The address it was paired at.
    pub address: String,
    /// Its name for users: the one it advertised, or its address.
    pub name: String,
}

/// The files the app keeps, in its directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppFiles {
    /// The app's directory.
    directory: PathBuf,
}

impl AppFiles {
    /// Returns the files in `directory`.
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// Returns the directory of the client's identity, which `qdp connect`
    /// takes as `--identity`.
    pub fn identity(&self) -> PathBuf {
        self.directory.join(IDENTITY_DIRECTORY)
    }

    /// Returns the hosts paired with, most recently paired first, and none
    /// if the files cannot be read.
    pub fn paired_hosts(&self) -> Vec<PairedHost> {
        let names = read_entries(&self.directory.join(NAMES_FILE));
        let names: BTreeMap<_, _> = names.into_iter().collect();
        let mut hosts: Vec<_> = read_entries(&self.identity().join(PAIRED_HOSTS_FILE))
            .into_iter()
            .map(|(pin, address)| PairedHost {
                name: names.get(&pin).cloned().unwrap_or_else(|| address.clone()),
                pin,
                address,
            })
            .collect();
        // `qdp` adds a host at the end.
        hosts.reverse();
        hosts
    }

    /// Keeps `name` as the name of the host whose pin is `pin`, or forgets
    /// its name if `name` is `None`.
    pub fn set_name(&self, pin: &KeyPin, name: Option<&str>) -> io::Result<()> {
        let path = self.directory.join(NAMES_FILE);
        let mut names: BTreeMap<_, _> = read_entries(&path).into_iter().collect();
        match name {
            Some(name) => names.insert(pin.clone(), name.to_owned()),
            None => names.remove(pin),
        };
        fs::create_dir_all(&self.directory)?;
        fs::write(path, format_entries(&names))
    }
}

/// Reads a file of entries, each a key pin, a space and a label, skipping
/// lines that are not entries; none if the file cannot be read.
fn read_entries(path: &Path) -> Vec<(KeyPin, String)> {
    fs::read_to_string(path)
        .map(|text| parse_entries(&text))
        .unwrap_or_default()
}

/// Reads the entries of `text`.
fn parse_entries(text: &str) -> Vec<(KeyPin, String)> {
    text.lines()
        .filter_map(|line| {
            let (pin, label) = line.trim_end().split_once(' ')?;
            let label = label.trim();
            if label.is_empty() {
                return None;
            }
            Some((KeyPin::parse(pin)?, label.to_owned()))
        })
        .collect()
}

/// Writes `entries` as a file's text.
fn format_entries(entries: &BTreeMap<KeyPin, String>) -> String {
    entries
        .iter()
        .map(|(pin, label)| format!("{pin} {label}\n"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a pin of `byte`s.
    fn pin(byte: &str) -> KeyPin {
        KeyPin::parse(&byte.repeat(32)).unwrap()
    }

    #[test]
    fn reads_entries() {
        let text = format!(
            "{} 192.168.1.5:4433\nnot an entry\n{} \n{} Living Room\n",
            "ab".repeat(32),
            "cd".repeat(32),
            "ef".repeat(32)
        );
        assert_eq!(
            parse_entries(&text),
            [
                (pin("ab"), "192.168.1.5:4433".to_owned()),
                (pin("ef"), "Living Room".to_owned()),
            ]
        );
    }

    #[test]
    fn names_paired_hosts() {
        let directory = std::env::temp_dir().join(format!("qdp-gtk-hosts-{}", std::process::id()));
        let files = AppFiles::new(directory.clone());
        fs::create_dir_all(files.identity()).unwrap();
        fs::write(
            files.identity().join(PAIRED_HOSTS_FILE),
            format!(
                "{} 10.0.0.1:4433\n{} 10.0.0.2:4433\n",
                "ab".repeat(32),
                "cd".repeat(32)
            ),
        )
        .unwrap();
        files.set_name(&pin("ab"), Some("exile")).unwrap();
        let hosts = files.paired_hosts();
        assert_eq!(
            hosts,
            [
                PairedHost {
                    pin: pin("cd"),
                    address: "10.0.0.2:4433".to_owned(),
                    name: "10.0.0.2:4433".to_owned(),
                },
                PairedHost {
                    pin: pin("ab"),
                    address: "10.0.0.1:4433".to_owned(),
                    name: "exile".to_owned(),
                },
            ]
        );
        files.set_name(&pin("ab"), None).unwrap();
        assert_eq!(files.paired_hosts()[1].name, "10.0.0.1:4433");
        fs::remove_dir_all(directory).unwrap();
    }
}
