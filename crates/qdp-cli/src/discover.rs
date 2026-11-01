//! `qdp discover`: the hosts advertising on the local network, listed once,
//! or with `--watch` as they come and go, as lines an app reads:
//!
//! | Line | Meaning |
//! |---|---|
//! | `found <address> <pin\|-> pairing\|no-pairing\|- <name>` | The host `<name>` listens at `<address>`, with the key pin and pairing offer its record gives, or `-` where it cannot be read. |
//! | `lost <name>` | The host `<name>` went away. |
//!
//! The name comes last, since it may hold spaces. A host found again at
//! another address is `found` again. Watching ends when standard input
//! closes, so that an app ends it by closing its end of the pipe.

use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use qdp::discovery::PairingOffer;
use qdp_desktop::{Discovery, FoundHost};
use qdp_quic::KeyPin;
use tokio::sync::{mpsc, oneshot};

use crate::args::{DiscoverMode, format_pin};
use crate::{CliResult, interrupted};

/// How long `qdp discover` listens for hosts before listing them.
const DISCOVERY_WAIT: Duration = Duration::from_secs(3);

/// The word in place of a value a host's record does not give.
const UNKNOWN: &str = "-";

/// Lists the hosts on the local network, or watches them, as `mode` says.
pub async fn discover(mode: DiscoverMode) -> CliResult {
    match mode {
        DiscoverMode::List => list().await,
        DiscoverMode::Watch => watch().await,
    }
}

/// The hosts found, by name.
type Hosts = BTreeMap<String, FoundHost>;

/// A change to the hosts found, as a `--watch` line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    /// The host was found, or found at another address.
    Found(FoundHost),
    /// The host of this name went away.
    Lost(String),
}

impl fmt::Display for Change {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Found(host) => {
                let pin = host.record.as_ref().map_or_else(
                    || UNKNOWN.to_owned(),
                    |record| format_pin(KeyPin(record.pin)),
                );
                let pairing = match host.record.as_ref().map(|record| record.pairing) {
                    Some(PairingOffer::Offered) => "pairing",
                    Some(PairingOffer::NotOffered) => "no-pairing",
                    None => UNKNOWN,
                };
                write!(
                    formatter,
                    "found {} {pin} {pairing} {}",
                    host.address, host.name
                )
            }
            Self::Lost(name) => write!(formatter, "lost {name}"),
        }
    }
}

/// Notes `discovery` in `hosts`, and returns what changed, if anything. A
/// host found on several interfaces is kept once, at an IPv4 address if it
/// has one.
fn note(hosts: &mut Hosts, discovery: Discovery) -> Option<Change> {
    match discovery {
        Discovery::Found(host) => {
            let better = hosts.get(&host.name).is_none_or(|known| {
                *known != host && (host.address.is_ipv4() || !known.address.is_ipv4())
            });
            better.then(|| {
                hosts.insert(host.name.clone(), host.clone());
                Change::Found(host)
            })
        }
        Discovery::Lost(name) => hosts.remove(&name).map(|_| Change::Lost(name)),
    }
}

/// Lists the hosts that advertise within [`DISCOVERY_WAIT`], one a line:
/// its name, address and whether it offers pairing.
async fn list() -> CliResult {
    let found = Arc::new(Mutex::new(Hosts::new()));
    let hosts = Arc::clone(&found);
    let browsing = qdp_desktop::browse(move |discovery| {
        note(
            &mut hosts.lock().unwrap_or_else(PoisonError::into_inner),
            discovery,
        );
    });
    if let Ok(Err(error)) = tokio::time::timeout(DISCOVERY_WAIT, browsing).await {
        return Err(error);
    }
    let hosts = found.lock().unwrap_or_else(PoisonError::into_inner);
    if hosts.is_empty() {
        println!("No host advertises on the local network.");
    }
    for host in hosts.values() {
        let pairing = match host.record.as_ref().map(|record| record.pairing) {
            Some(PairingOffer::Offered) => "offers pairing",
            Some(PairingOffer::NotOffered) => "does not offer pairing",
            None => "unreadable record",
        };
        println!("{}  {}  {pairing}", host.name, host.address);
    }
    Ok(())
}

/// Writes a line for each host found and gone, until browsing fails, the
/// user interrupts, or standard input or output closes.
async fn watch() -> CliResult {
    let (sender, mut discoveries) = mpsc::unbounded_channel();
    let browsing = qdp_desktop::browse(move |discovery| {
        // Discoveries after watching ended have no one to tell.
        let _ = sender.send(discovery);
    });
    tokio::pin!(browsing);
    let mut closed = input_closed();
    let mut hosts = Hosts::new();
    loop {
        tokio::select! {
            result = &mut browsing => return result,
            Some(discovery) = discoveries.recv() => {
                if let Some(change) = note(&mut hosts, discovery) {
                    let mut output = std::io::stdout().lock();
                    if writeln!(output, "{change}").and_then(|()| output.flush()).is_err() {
                        // No one reads the lines any more.
                        return Ok(());
                    }
                }
            }
            _ = &mut closed => return Ok(()),
            () = interrupted() => return Ok(()),
        }
    }
}

/// Returns a receiver that completes once standard input closes.
fn input_closed() -> oneshot::Receiver<()> {
    let (sender, receiver) = oneshot::channel();
    // Reading blocks, so it has a thread of its own, which ends with the
    // process.
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink());
        let _ = sender.send(());
    });
    receiver
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use qdp::discovery::ServiceRecord;

    use super::*;

    /// Returns a host named `name` at `address`.
    fn host(name: &str, address: &str, record: Option<ServiceRecord>) -> FoundHost {
        FoundHost {
            name: name.to_owned(),
            address: address.parse::<SocketAddr>().unwrap(),
            record,
        }
    }

    #[test]
    fn formats_changes() {
        let record = ServiceRecord::new([0xab; 32], PairingOffer::Offered);
        assert_eq!(
            Change::Found(host("Living Room", "192.168.1.5:4433", Some(record))).to_string(),
            format!(
                "found 192.168.1.5:4433 {} pairing Living Room",
                "ab".repeat(32)
            )
        );
        let closed = ServiceRecord::new([1; 32], PairingOffer::NotOffered);
        assert_eq!(
            Change::Found(host("exile", "[fe80::1%2]:4433", Some(closed))).to_string(),
            format!(
                "found [fe80::1%2]:4433 {} no-pairing exile",
                "01".repeat(32)
            )
        );
        assert_eq!(
            Change::Found(host("exile", "10.0.0.2:4433", None)).to_string(),
            "found 10.0.0.2:4433 - - exile"
        );
        assert_eq!(
            Change::Lost("Living Room".to_owned()).to_string(),
            "lost Living Room"
        );
    }

    #[test]
    fn keeps_a_host_once_preferring_ipv4() {
        let mut hosts = Hosts::new();
        let ipv6 = host("exile", "[fe80::1%2]:4433", None);
        let ipv4 = host("exile", "192.168.1.5:4433", None);
        assert_eq!(
            note(&mut hosts, Discovery::Found(ipv6.clone())),
            Some(Change::Found(ipv6.clone()))
        );
        // The same again changes nothing.
        assert_eq!(note(&mut hosts, Discovery::Found(ipv6.clone())), None);
        assert_eq!(
            note(&mut hosts, Discovery::Found(ipv4.clone())),
            Some(Change::Found(ipv4.clone()))
        );
        // An IPv6 address does not replace an IPv4 one.
        assert_eq!(note(&mut hosts, Discovery::Found(ipv6)), None);
        // But a new IPv4 address does.
        let moved = host("exile", "192.168.1.9:4433", None);
        assert_eq!(
            note(&mut hosts, Discovery::Found(moved.clone())),
            Some(Change::Found(moved))
        );
        assert_eq!(
            note(&mut hosts, Discovery::Lost("exile".to_owned())),
            Some(Change::Lost("exile".to_owned()))
        );
        // A host not found is not lost.
        assert_eq!(note(&mut hosts, Discovery::Lost("exile".to_owned())), None);
    }
}
