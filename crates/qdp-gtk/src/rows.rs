//! The list of computers: the hosts found on the network, recognized as
//! paired by their pins, then the paired hosts not found, each with what
//! choosing it does. The Mac and iPhone app's `HostStore` lists them alike.

use crate::discovery::{FoundHost, PairingOffer};
use crate::hosts::PairedHost;
use crate::key_pin::KeyPin;

/// How a row stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Paired and on this network: choosing it streams.
    Ready,
    /// Paired, but not seen on this network: choosing it tries anyway.
    NotSeen,
    /// New and offering pairing: choosing it pairs.
    New,
    /// New, with pairing off.
    PairingOff,
    /// Found under a paired host's name with another key, which may pose as
    /// it.
    DifferentKey,
}

/// How a session authenticates the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authentication {
    /// By the key pin kept from pairing.
    Paired(KeyPin),
    /// By pairing, with the code the host shows.
    Pair,
}

/// A host to stream from, or to pair with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// Its name for users.
    pub name: String,
    /// Where it listens.
    pub address: String,
    /// How it is authenticated.
    pub authentication: Authentication,
}

impl Target {
    /// Returns what tells this target's sessions apart from others': the
    /// host's pin once paired, its address until then.
    pub fn key(&self) -> &str {
        match &self.authentication {
            Authentication::Paired(pin) => pin.as_str(),
            Authentication::Pair => &self.address,
        }
    }
}

/// A row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The host's name.
    pub name: String,
    /// What the row says under it.
    pub detail: String,
    /// How it stands.
    pub status: Status,
    /// What choosing it streams from or pairs with, if anything.
    pub target: Option<Target>,
    /// The paired host it is, which can be forgotten.
    pub paired: Option<PairedHost>,
}

/// Returns the rows of the hosts `found` on the network and those `paired`
/// with: those found, by name, then those paired with that are not found.
pub fn rows(found: &[FoundHost], paired: &[PairedHost]) -> Vec<Row> {
    let mut found: Vec<_> = found.iter().collect();
    found.sort_by_key(|host| host.name.to_lowercase());
    let mut seen = Vec::new();
    let mut rows: Vec<_> = found
        .into_iter()
        .map(|host| {
            let pairs = host.pairing == PairingOffer::Offered;
            let pair = pairs.then(|| Target {
                name: host.name.clone(),
                address: host.address.clone(),
                authentication: Authentication::Pair,
            });
            if let Some(known) = paired
                .iter()
                .find(|known| Some(&known.pin) == host.pin.as_ref())
            {
                seen.push(known.pin.clone());
                return Row {
                    name: host.name.clone(),
                    detail: format!("Ready · {}", host.address),
                    status: Status::Ready,
                    target: Some(Target {
                        name: host.name.clone(),
                        address: host.address.clone(),
                        authentication: Authentication::Paired(known.pin.clone()),
                    }),
                    paired: Some(known.clone()),
                };
            }
            if paired.iter().any(|known| known.name == host.name) {
                return Row {
                    name: host.name.clone(),
                    detail: format!("This isn’t the {} you paired with", host.name),
                    status: Status::DifferentKey,
                    target: pair,
                    paired: None,
                };
            }
            if pairs {
                return Row {
                    name: host.name.clone(),
                    detail: format!("New · {}", host.address),
                    status: Status::New,
                    target: pair,
                    paired: None,
                };
            }
            Row {
                name: host.name.clone(),
                detail: "Pairing is off on this computer".to_owned(),
                status: Status::PairingOff,
                target: None,
                paired: None,
            }
        })
        .collect();
    rows.extend(
        paired
            .iter()
            .filter(|host| !seen.contains(&host.pin))
            .map(|host| Row {
                name: host.name.clone(),
                detail: format!("Not on this network · {}", host.address),
                status: Status::NotSeen,
                target: Some(Target {
                    name: host.name.clone(),
                    address: host.address.clone(),
                    authentication: Authentication::Paired(host.pin.clone()),
                }),
                paired: Some(host.clone()),
            }),
    );
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a pin of `byte`s.
    fn pin(byte: &str) -> KeyPin {
        KeyPin::parse(&byte.repeat(32)).unwrap()
    }

    /// Returns a host found as `name` at `address`.
    fn found(name: &str, address: &str, pin: Option<KeyPin>, pairing: PairingOffer) -> FoundHost {
        FoundHost {
            name: name.to_owned(),
            address: address.to_owned(),
            pin,
            pairing,
        }
    }

    /// Returns a host paired with as `name`.
    fn paired(name: &str, address: &str, pin: KeyPin) -> PairedHost {
        PairedHost {
            pin,
            address: address.to_owned(),
            name: name.to_owned(),
        }
    }

    #[test]
    fn lists_found_hosts_by_name_then_paired_hosts_not_found() {
        let found = [
            found(
                "zeta",
                "10.0.0.9:4433",
                Some(pin("99")),
                PairingOffer::NotOffered,
            ),
            found(
                "exile",
                "10.0.0.5:4433",
                Some(pin("ab")),
                PairingOffer::Offered,
            ),
            found(
                "Attic",
                "10.0.0.7:4433",
                Some(pin("77")),
                PairingOffer::Offered,
            ),
            found(
                "office",
                "10.0.0.8:4433",
                Some(pin("88")),
                PairingOffer::Offered,
            ),
        ];
        let paired = [
            paired("exile", "10.0.0.4:4433", pin("ab")),
            paired("office", "10.0.0.3:4433", pin("cd")),
            paired("den", "10.0.0.2:4433", pin("ef")),
        ];
        let rows = rows(&found, &paired);
        let summary: Vec<_> = rows
            .iter()
            .map(|row| {
                (
                    row.name.as_str(),
                    row.status,
                    row.target.as_ref().map(Target::key),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("Attic", Status::New, Some("10.0.0.7:4433")),
                // Found where it is now, not where it was paired.
                ("exile", Status::Ready, Some(pin("ab").as_str())),
                // Claims a paired host's name with another key: pairs anew.
                ("office", Status::DifferentKey, Some("10.0.0.8:4433")),
                ("zeta", Status::PairingOff, None),
                ("office", Status::NotSeen, Some(pin("cd").as_str())),
                ("den", Status::NotSeen, Some(pin("ef").as_str())),
            ]
        );
        assert_eq!(rows[1].detail, "Ready · 10.0.0.5:4433");
        assert_eq!(rows[1].target.as_ref().unwrap().address, "10.0.0.5:4433");
        assert_eq!(rows[5].detail, "Not on this network · 10.0.0.2:4433");
        assert!(rows[2].paired.is_none());
        assert_eq!(rows[4].paired.as_ref().unwrap().pin, pin("cd"));
    }
}
