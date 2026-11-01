//! The hosts on the local network, as `qdp discover --watch` reports them, a
//! line as each comes and goes (see that command's `discover` module).

use std::ffi::OsStr;

use gtk::{gio, glib};

use crate::command;
use crate::key_pin::KeyPin;

/// What a host's record says of pairing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingOffer {
    /// The host lets new clients pair.
    Offered,
    /// The host does not let new clients pair.
    NotOffered,
    /// The host's record cannot be read.
    Unknown,
}

/// A host found on the local network. Nothing here is authenticated: the pin
/// only recognizes a host already paired with, which the connection then
/// verifies (Section 3.13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundHost {
    /// Its name for users.
    pub name: String,
    /// Where it listens, as `qdp connect` takes it.
    pub address: String,
    /// The key pin its record claims, if it can be read.
    pub pin: Option<KeyPin>,
    /// Whether it offers pairing.
    pub pairing: PairingOffer,
}

/// A change to the hosts on the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// A host was found, or found at another address.
    Found(FoundHost),
    /// The host of this name went away.
    Lost(String),
}

impl Change {
    /// Reads `line`, or returns `None` if it is not one `qdp` writes.
    pub fn parse(line: &str) -> Option<Self> {
        let (word, rest) = line.split_once(' ')?;
        match word {
            "found" => {
                // The name comes last, since it may hold spaces.
                let mut fields = rest.splitn(4, ' ');
                let address = fields.next()?.to_owned();
                let pin = KeyPin::parse(fields.next()?);
                let pairing = match fields.next()? {
                    "pairing" => PairingOffer::Offered,
                    "no-pairing" => PairingOffer::NotOffered,
                    _ => PairingOffer::Unknown,
                };
                let name = fields.next().filter(|name| !name.is_empty())?.to_owned();
                Some(Self::Found(FoundHost {
                    name,
                    address,
                    pin,
                    pairing,
                }))
            }
            "lost" => Some(Self::Lost(rest.to_owned())),
            _ => None,
        }
    }
}

/// Watches the hosts on the local network, telling `report` of each change,
/// until `qdp discover --watch` ends, and then calls `ended`. It ends when
/// the network cannot be searched, or when the app exits, closing its
/// standard input.
pub fn watch(
    mut report: impl FnMut(Change) + 'static,
    ended: impl FnOnce() + 'static,
) -> Result<gio::Subprocess, glib::Error> {
    let process = command::start(&[OsStr::new("discover"), OsStr::new("--watch")])?;
    command::read_lines(
        &process,
        move |line| {
            if let Some(change) = Change::parse(line) {
                report(change);
            }
        },
        ended,
    );
    Ok(process)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_found_and_lost_hosts() {
        let pin = "ab".repeat(32);
        assert_eq!(
            Change::parse(&format!("found 192.168.1.5:4433 {pin} pairing Living Room")),
            Some(Change::Found(FoundHost {
                name: "Living Room".to_owned(),
                address: "192.168.1.5:4433".to_owned(),
                pin: KeyPin::parse(&pin),
                pairing: PairingOffer::Offered,
            }))
        );
        assert_eq!(
            Change::parse("found [fe80::1%2]:4433 - - exile"),
            Some(Change::Found(FoundHost {
                name: "exile".to_owned(),
                address: "[fe80::1%2]:4433".to_owned(),
                pin: None,
                pairing: PairingOffer::Unknown,
            }))
        );
        assert_eq!(
            Change::parse("lost Living Room"),
            Some(Change::Lost("Living Room".to_owned()))
        );
        assert_eq!(Change::parse("found 10.0.0.1:4433 - -"), None);
        assert_eq!(Change::parse("No host advertises"), None);
    }
}
