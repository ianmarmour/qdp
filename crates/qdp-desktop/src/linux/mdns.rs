//! DNS-SD over Multicast DNS of QDP's own, which needs no system service
//! such as Avahi: a responder that advertises the host's service and moves
//! it to a free name when another service holds its own, and a browser that
//! finds hosts.
//!
//! Each runs on its own thread, sharing the mDNS port with the system's
//! responder, if it has one, and stops when dropped.

use std::fmt;
use std::net::{IpAddr, SocketAddr, SocketAddrV6};

use mdns_sd::{
    DaemonEvent, IfKind, ResolvedService, ScopedIp, ServiceDaemon, ServiceEvent, ServiceInfo,
};
use qdp::discovery::{PIN_LENGTH, SERVICE_TYPE, ServiceRecord, instance_name};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use crate::advertise::Advertisement;
use crate::browse::{Discovery, FoundHost};

/// The service type in the domain `local.`, as mDNS names it.
fn service_domain() -> String {
    format!("{SERVICE_TYPE}.local.")
}

/// An mDNS responder and browser of this process's own, stopped when
/// dropped, which withdraws the services it advertised.
struct Responder(ServiceDaemon);

impl fmt::Debug for Responder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Responder").finish_non_exhaustive()
    }
}

impl Responder {
    /// Starts one.
    fn start() -> Result<Self, String> {
        ServiceDaemon::new()
            .map(Self)
            .map_err(|error| format!("cannot start Multicast DNS: {error}"))
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        // It has already stopped if it was withdrawn.
        let _ = self.0.shutdown();
    }
}

/// A service the host's own responder advertises.
#[derive(Debug)]
pub(crate) struct Advertising {
    /// The responder.
    responder: Responder,
    /// The task that reports the responder's renames and errors.
    task: JoinHandle<()>,
}

impl Advertising {
    /// Withdraws the service, telling clients it went away, and stops the
    /// responder.
    pub(crate) async fn withdraw(self) {
        self.task.abort();
        match self.responder.0.shutdown() {
            Ok(stopped) => {
                let _ = stopped.recv_async().await;
            }
            Err(error) => warn!(%error, "cannot stop advertising on the local network"),
        }
    }
}

impl Drop for Advertising {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Returns the name the host's addresses go under. It is the host's own
/// rather than the machine's, which the system's responder may answer for
/// with addresses of its own, and comes from the start of its key pin, so
/// that it stays the same.
fn host_name(pin: &[u8; PIN_LENGTH]) -> String {
    let id: String = pin[..4].iter().map(|byte| format!("{byte:02x}")).collect();
    format!("qdp-{id}.local.")
}

/// Advertises `advertisement` at the machine's addresses, following them as
/// they change.
pub(crate) fn advertise(advertisement: &Advertisement) -> Result<Advertising, String> {
    let unavailable = |error: mdns_sd::Error| format!("cannot advertise: {error}");
    let responder = Responder::start()?;
    let daemon = &responder.0;
    if advertisement.address.is_ipv4() {
        // A host listening on IPv4 only is reached at no IPv6 address.
        daemon
            .disable_interface(IfKind::IPv6)
            .map_err(unavailable)?;
    }
    let events = daemon.monitor().map_err(unavailable)?;
    let name = instance_name(&advertisement.name);
    let service = ServiceInfo::new(
        &service_domain(),
        &name,
        &host_name(&advertisement.record.pin),
        (),
        advertisement.address.port(),
        &advertisement.record.entries()[..],
    )
    .map_err(unavailable)?
    .enable_addr_auto();
    daemon.register(service).map_err(unavailable)?;
    let task = tokio::spawn(async move {
        let mut announced = false;
        while let Ok(event) = events.recv_async().await {
            match event {
                DaemonEvent::Announce(..) if !announced => {
                    announced = true;
                    info!(name, "advertising on the local network");
                }
                DaemonEvent::NameChange(change) => info!(
                    from = change.original,
                    to = change.new_name,
                    "renamed on the local network, where another service held the name"
                ),
                DaemonEvent::Error(error) => {
                    warn!(%error, "advertising on the local network failed");
                }
                _ => {}
            }
        }
    });
    Ok(Advertising { responder, task })
}

/// Browses for hosts, telling `report` of each found and gone, until
/// browsing fails.
pub(crate) async fn browse(mut report: impl FnMut(Discovery) + Send) -> Result<(), String> {
    let responder = Responder::start()?;
    let domain = service_domain();
    let events = responder
        .0
        .browse(&domain)
        .map_err(|error| format!("cannot search the local network: {error}"))?;
    while let Ok(event) = events.recv_async().await {
        match event {
            ServiceEvent::ServiceResolved(service) => match found_host(&service, &domain) {
                Some(host) => report(Discovery::Found(host)),
                None => debug!(name = service.fullname, "a host without an address"),
            },
            ServiceEvent::ServiceRemoved(_, fullname) => {
                report(Discovery::Lost(instance(&fullname, &domain).to_owned()));
            }
            _ => {}
        }
    }
    Err("searching the local network stopped".to_owned())
}

/// Returns the instance name of the service `fullname` of type `domain`.
fn instance<'a>(fullname: &'a str, domain: &str) -> &'a str {
    fullname
        .strip_suffix(domain)
        .and_then(|name| name.strip_suffix('.'))
        .unwrap_or(fullname)
}

/// Returns the host `service` describes, at the address clients reach it
/// at most surely, or `None` if it has none.
fn found_host(service: &ResolvedService, domain: &str) -> Option<FoundHost> {
    let address = service
        .addresses
        .iter()
        .min_by_key(|address| (preference(address), address.to_ip_addr()))?;
    Some(FoundHost {
        name: instance(&service.fullname, domain).to_owned(),
        address: socket_address(address, service.port),
        record: ServiceRecord::parse(
            service
                .txt_properties
                .iter()
                .map(|property| (property.key(), property.val())),
        ),
    })
}

/// Ranks `address`, lowest first: an IPv4 address, as the hosts' other
/// clients prefer, then a link-local one, then IPv6's, and this machine's
/// own last.
fn preference(address: &ScopedIp) -> u8 {
    match address.to_ip_addr() {
        IpAddr::V4(ip) if ip.is_loopback() => 4,
        IpAddr::V4(ip) if ip.is_link_local() => 1,
        IpAddr::V4(_) => 0,
        IpAddr::V6(ip) if ip.is_loopback() => 4,
        IpAddr::V6(ip) if ip.is_unicast_link_local() => 3,
        IpAddr::V6(_) => 2,
    }
}

/// Returns the socket address of `address` and `port`, an IPv6 link-local
/// one scoped to the interface it was found on, which it is reached
/// through.
fn socket_address(address: &ScopedIp, port: u16) -> SocketAddr {
    match address {
        ScopedIp::V6(scoped) if scoped.addr().is_unicast_link_local() => SocketAddr::V6(
            SocketAddrV6::new(*scoped.addr(), port, 0, scoped.scope_id().index),
        ),
        other => SocketAddr::new(other.to_ip_addr(), port),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::time::Duration;

    use qdp::discovery::PairingOffer;

    use super::*;

    /// Instance names keep their spaces and dots, and lose the service type.
    #[test]
    fn reads_instance_names() {
        let domain = service_domain();
        assert_eq!(domain, "_qdp._udp.local.");
        assert_eq!(instance("studio._qdp._udp.local.", &domain), "studio");
        assert_eq!(
            instance("Ian's PC v2.1._qdp._udp.local.", &domain),
            "Ian's PC v2.1"
        );
        assert_eq!(instance("odd", &domain), "odd");
    }

    /// The host's name for its addresses comes from its pin.
    #[test]
    fn names_the_host_after_its_pin() {
        let mut pin = [0; PIN_LENGTH];
        pin[..5].copy_from_slice(&[0x1a, 0x2b, 0x3c, 0x4d, 0x5e]);
        assert_eq!(host_name(&pin), "qdp-1a2b3c4d.local.");
    }

    /// Hosts are reached at an IPv4 address when they have one, and at this
    /// machine's own last.
    #[test]
    fn prefers_ipv4() {
        let addresses = |list: &[&str]| -> HashSet<ScopedIp> {
            list.iter()
                .map(|address| ScopedIp::from(address.parse::<IpAddr>().unwrap()))
                .collect()
        };
        let best = |list: &[&str]| {
            let addresses = addresses(list);
            let best = addresses
                .iter()
                .min_by_key(|address| (preference(address), address.to_ip_addr()))
                .unwrap();
            socket_address(best, 4433).to_string()
        };
        assert_eq!(
            best(&["fe80::1", "2001:db8::1", "192.168.1.44", "127.0.0.1"]),
            "192.168.1.44:4433"
        );
        assert_eq!(
            best(&["169.254.3.4", "192.168.1.44", "10.0.0.2"]),
            "10.0.0.2:4433"
        );
        assert_eq!(best(&["fe80::1", "2001:db8::1"]), "[2001:db8::1]:4433");
        assert_eq!(best(&["127.0.0.1", "::1"]), "127.0.0.1:4433");
    }

    /// A host advertised by one responder is found, record and all, by
    /// another on the same machine, and lost once it withdraws.
    #[tokio::test(flavor = "multi_thread")]
    async fn finds_what_it_advertises() {
        let mut pin = [0; PIN_LENGTH];
        pin[0] = 0xa7;
        pin[31] = 0x5c;
        // Unique per run, so that runs at once on one network do not meet.
        let name = format!("qdp test {}", std::process::id());
        let advertising = advertise(&Advertisement {
            name: name.clone(),
            address: "[::]:4433".parse().unwrap(),
            record: ServiceRecord::new(pin, PairingOffer::Offered),
        })
        .unwrap();

        let (found, mut discoveries) = tokio::sync::mpsc::unbounded_channel();
        let browsing = tokio::spawn(browse(move |discovery| {
            let _ = found.send(discovery);
        }));
        let host = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match discoveries.recv().await {
                    Some(Discovery::Found(host)) if host.name == name => break host,
                    Some(_) => {}
                    None => panic!("browsing stopped"),
                }
            }
        })
        .await
        .expect("the host is found");
        assert_eq!(host.address.port(), 4433);
        assert_eq!(
            host.record,
            Some(ServiceRecord::new(pin, PairingOffer::Offered))
        );

        advertising.withdraw().await;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                match discoveries.recv().await {
                    Some(Discovery::Lost(lost)) if lost == name => break,
                    Some(_) => {}
                    None => panic!("browsing stopped"),
                }
            }
        })
        .await
        .expect("the host is lost once it withdraws");
        browsing.abort();
    }
}
