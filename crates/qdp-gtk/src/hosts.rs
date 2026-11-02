//! The hosts paired with: their keys and addresses, which `qdp` keeps in the
//! client identity's `paired-hosts` file as it pairs (see the qdp-cli
//! crate's `paired` module), and the names the app knew them by, which it
//! keeps beside the identity in a file of the same form: a line a host, its
//! key pin, a space and its name. Hosts the user added with a key pin and a
//! token are kept in a file of their own, which only the user may read.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::key_pin::KeyPin;

/// The directory of the client's identity, in the app's directory.
const IDENTITY_DIRECTORY: &str = "client-identity";

/// The file in the identity directory of the hosts `qdp` paired with.
const PAIRED_HOSTS_FILE: &str = "paired-hosts";

/// The file in the app's directory of the paired hosts' names.
const NAMES_FILE: &str = "host-names";

/// The file in the app's directory of the hosts added with a key pin and a
/// token: a line a host, its pin, its address, its token and its name,
/// separated by tabs.
const TOKEN_HOSTS_FILE: &str = "token-hosts";

/// The file in the app's directory of the computers reached through the
/// gateway.
const GATEWAY_HOSTS_FILE: &str = "gateway-hosts";

/// The token hosts file's permissions: its tokens are credentials.
const TOKEN_HOSTS_MODE: u32 = 0o600;

/// A token a host accepts, which never shows in logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Returns `text` as a token, or `None` if it is empty or holds a tab or
    /// a line break, which the file cannot keep.
    pub fn new(text: &str) -> Option<Self> {
        let valid = !text.is_empty() && !text.contains(['\t', '\n', '\r']);
        valid.then(|| Self(text.to_owned()))
    }

    /// Returns the token, as `qdp connect` takes it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(..)")
    }
}

/// A host paired with, or added with a key pin and a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedHost {
    /// Its key pin, which `qdp connect` verifies it by.
    pub pin: KeyPin,
    /// The address it was paired at, or added with.
    pub address: String,
    /// Its name for users: the one it advertised, or its address.
    pub name: String,
    /// The token it accepts, for a host added with one; `None` for a host
    /// paired with, which accepts the client's key.
    pub token: Option<Token>,
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

    /// Returns the file of the computers reached through the gateway.
    pub fn gateway_hosts(&self) -> PathBuf {
        self.directory.join(GATEWAY_HOSTS_FILE)
    }

    /// Returns the hosts paired with, most recently paired first, and none
    /// if the files cannot be read.
    pub fn paired_hosts(&self) -> Vec<PairedHost> {
        let names = read_entries(&self.directory.join(NAMES_FILE));
        let names: BTreeMap<_, _> = names.into_iter().collect();
        let mut hosts = self.token_hosts();
        let mut paired: Vec<_> = read_entries(&self.identity().join(PAIRED_HOSTS_FILE))
            .into_iter()
            .filter(|(pin, _)| !hosts.iter().any(|host| &host.pin == pin))
            .map(|(pin, address)| PairedHost {
                name: names.get(&pin).cloned().unwrap_or_else(|| address.clone()),
                pin,
                address,
                token: None,
            })
            .collect();
        // `qdp` adds a host at the end.
        paired.reverse();
        hosts.extend(paired);
        hosts
    }

    /// Returns the hosts added with a key pin and a token, most recently
    /// added first.
    fn token_hosts(&self) -> Vec<PairedHost> {
        let text = fs::read_to_string(self.directory.join(TOKEN_HOSTS_FILE)).unwrap_or_default();
        let mut hosts: Vec<_> = text
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(4, '\t');
                let pin = KeyPin::parse(fields.next()?)?;
                let address = fields
                    .next()
                    .filter(|address| !address.is_empty())?
                    .to_owned();
                let token = Token::new(fields.next()?)?;
                let name = fields.next().filter(|name| !name.is_empty())?.to_owned();
                Some(PairedHost {
                    pin,
                    address,
                    name,
                    token: Some(token),
                })
            })
            .collect();
        hosts.reverse();
        hosts
    }

    /// Keeps a host added with `pin`, `address`, `token` and `name`,
    /// replacing any added with the same pin.
    pub fn add_token_host(
        &self,
        pin: &KeyPin,
        address: &str,
        token: &Token,
        name: &str,
    ) -> io::Result<()> {
        let mut hosts = self.token_hosts();
        hosts.retain(|host| &host.pin != pin);
        hosts.insert(
            0,
            PairedHost {
                pin: pin.clone(),
                address: address.to_owned(),
                name: name.to_owned(),
                token: Some(token.clone()),
            },
        );
        self.save_token_hosts(&hosts)
    }

    /// Forgets the host added with `pin`, returning whether there was one.
    pub fn remove_token_host(&self, pin: &KeyPin) -> io::Result<bool> {
        let mut hosts = self.token_hosts();
        let before = hosts.len();
        hosts.retain(|host| &host.pin != pin);
        if hosts.len() == before {
            return Ok(false);
        }
        self.save_token_hosts(&hosts).map(|()| true)
    }

    /// Writes the token hosts, most recent first as given, readable only by
    /// the user, replacing the file at once.
    fn save_token_hosts(&self, hosts: &[PairedHost]) -> io::Result<()> {
        let mut text = String::new();
        // The file keeps them oldest first, as `qdp` keeps paired hosts.
        for host in hosts.iter().rev() {
            let Some(token) = &host.token else {
                continue;
            };
            let name = host.name.replace(['\t', '\n', '\r'], " ");
            text.push_str(&format!(
                "{}\t{}\t{}\t{name}\n",
                host.pin,
                host.address,
                token.as_str()
            ));
        }
        fs::create_dir_all(&self.directory)?;
        let path = self.directory.join(TOKEN_HOSTS_FILE);
        let partial = path.with_extension("partial");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(TOKEN_HOSTS_MODE)
            .open(&partial)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(partial, path)
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
                    token: None,
                },
                PairedHost {
                    pin: pin("ab"),
                    address: "10.0.0.1:4433".to_owned(),
                    name: "exile".to_owned(),
                    token: None,
                },
            ]
        );
        files.set_name(&pin("ab"), None).unwrap();
        assert_eq!(files.paired_hosts()[1].name, "10.0.0.1:4433");

        // Hosts added with a token come first, and replace a paired host
        // with the same pin; the file is the user's alone.
        let token = Token::new("s3cret").unwrap();
        files
            .add_token_host(&pin("ef"), "10.0.0.3:4433", &token, "Lab")
            .unwrap();
        files
            .add_token_host(&pin("ab"), "10.0.0.9:4433", &token, "exile 2")
            .unwrap();
        let hosts = files.paired_hosts();
        let summary: Vec<_> = hosts
            .iter()
            .map(|host| (host.name.as_str(), host.token.is_some()))
            .collect();
        assert_eq!(
            summary,
            [("exile 2", true), ("Lab", true), ("10.0.0.2:4433", false)]
        );
        assert_eq!(format!("{token:?}"), "Token(..)");
        let mode = fs::metadata(directory.join(TOKEN_HOSTS_FILE))
            .unwrap()
            .permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
        assert!(files.remove_token_host(&pin("ab")).unwrap());
        assert!(!files.remove_token_host(&pin("ab")).unwrap());
        assert_eq!(files.paired_hosts().len(), 3);
        assert!(Token::new("").is_none());
        assert!(Token::new("a\tb").is_none());
        fs::remove_dir_all(directory).unwrap();
    }
}
