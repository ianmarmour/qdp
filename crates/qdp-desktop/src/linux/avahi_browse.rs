//! Finding hosts through Avahi's D-Bus interface: a service browser for
//! `_qdp._udp`, each instance it reports resolved to its address, port and
//! TXT record, preferring IPv4.

use std::net::{IpAddr, SocketAddr, SocketAddrV6};

use futures_lite::StreamExt;
use qdp::discovery::{SERVICE_TYPE, ServiceRecord};
use tracing::debug;
use zbus::Connection;
use zbus::zvariant::OwnedObjectPath;

use crate::browse::{Discovery, FoundHost};

/// Avahi's code for every network interface.
const ANY_INTERFACE: i32 = -1;

/// Avahi's code for both IPv4 and IPv6.
const ANY_PROTOCOL: i32 = -1;

/// Avahi's code for IPv4.
const IPV4: i32 = 0;

/// No flags.
const NO_FLAGS: u32 = 0;

/// The domain Avahi chooses, `local`.
const DEFAULT_DOMAIN: &str = "";

/// What `ResolveService` returns: the interface, protocol, name, type,
/// domain, host name, address protocol, address, port, TXT record and
/// flags.
type Resolved = (
    i32,
    i32,
    String,
    String,
    String,
    String,
    i32,
    String,
    u16,
    Vec<Vec<u8>>,
    u32,
);

/// Avahi's server, as browsing uses it.
#[zbus::proxy(
    interface = "org.freedesktop.Avahi.Server",
    default_service = "org.freedesktop.Avahi",
    default_path = "/",
    gen_blocking = false
)]
trait BrowsingServer {
    /// Creates a service browser that reports nothing until it is started,
    /// so that no report is missed (Avahi 0.8 and later).
    fn service_browser_prepare(
        &self,
        interface: i32,
        protocol: i32,
        service_type: &str,
        domain: &str,
        flags: u32,
    ) -> zbus::Result<OwnedObjectPath>;

    /// Creates a service browser that starts reporting at once.
    fn service_browser_new(
        &self,
        interface: i32,
        protocol: i32,
        service_type: &str,
        domain: &str,
        flags: u32,
    ) -> zbus::Result<OwnedObjectPath>;

    /// Resolves a service to its address, port and TXT record.
    #[allow(clippy::too_many_arguments)]
    fn resolve_service(
        &self,
        interface: i32,
        protocol: i32,
        name: &str,
        service_type: &str,
        domain: &str,
        address_protocol: i32,
        flags: u32,
    ) -> zbus::Result<Resolved>;
}

/// A browser of one service type.
#[zbus::proxy(
    interface = "org.freedesktop.Avahi.ServiceBrowser",
    default_service = "org.freedesktop.Avahi",
    gen_blocking = false
)]
trait ServiceBrowser {
    /// Starts reporting, for a browser made with `ServiceBrowserPrepare`.
    fn start(&self) -> zbus::Result<()>;

    /// Removes the browser.
    fn free(&self) -> zbus::Result<()>;

    /// Reports a service found on an interface.
    #[zbus(signal)]
    fn item_new(
        &self,
        interface: i32,
        protocol: i32,
        name: &str,
        service_type: &str,
        domain: &str,
        flags: u32,
    ) -> zbus::Result<()>;

    /// Reports a service gone from an interface.
    #[zbus(signal)]
    fn item_remove(
        &self,
        interface: i32,
        protocol: i32,
        name: &str,
        service_type: &str,
        domain: &str,
        flags: u32,
    ) -> zbus::Result<()>;

    /// Reports that browsing failed.
    #[zbus(signal)]
    fn failure(&self, error: &str) -> zbus::Result<()>;
}

/// What the browser reported.
enum Report {
    /// A service was found.
    New(ItemNew),
    /// A service went away.
    Removed(ItemRemove),
    /// Browsing failed.
    Failed(Failure),
}

/// Browses for hosts, telling `report` of each found and gone, until
/// browsing fails.
pub(crate) async fn browse(mut report: impl FnMut(Discovery) + Send) -> Result<(), String> {
    let connection = Connection::system()
        .await
        .map_err(|error| format!("cannot reach the system bus: {error}"))?;
    let unavailable = |error: zbus::Error| format!("cannot browse through Avahi: {error}");
    let server = BrowsingServerProxy::new(&connection)
        .await
        .map_err(unavailable)?;
    let prepared = server
        .service_browser_prepare(
            ANY_INTERFACE,
            ANY_PROTOCOL,
            SERVICE_TYPE,
            DEFAULT_DOMAIN,
            NO_FLAGS,
        )
        .await;
    let (path, needs_start) = match prepared {
        Ok(path) => (path, true),
        Err(_) => (
            server
                .service_browser_new(
                    ANY_INTERFACE,
                    ANY_PROTOCOL,
                    SERVICE_TYPE,
                    DEFAULT_DOMAIN,
                    NO_FLAGS,
                )
                .await
                .map_err(unavailable)?,
            false,
        ),
    };
    let browser = ServiceBrowserProxy::builder(&connection)
        .path(path)
        .map_err(unavailable)?
        .build()
        .await
        .map_err(unavailable)?;
    let found = browser.receive_item_new().await.map_err(unavailable)?;
    let gone = browser.receive_item_remove().await.map_err(unavailable)?;
    let failures = browser.receive_failure().await.map_err(unavailable)?;
    let mut reports = found
        .map(Report::New)
        .or(gone.map(Report::Removed))
        .or(failures.map(Report::Failed));
    if needs_start {
        browser.start().await.map_err(unavailable)?;
    }
    while let Some(next) = reports.next().await {
        match next {
            Report::New(signal) => {
                let Ok(arguments) = signal.args() else {
                    continue;
                };
                match resolve(&server, *arguments.interface(), arguments.name()).await {
                    Ok(host) => report(Discovery::Found(host)),
                    Err(error) => debug!(%error, name = arguments.name(), "cannot resolve a host"),
                }
            }
            Report::Removed(signal) => {
                if let Ok(arguments) = signal.args() {
                    report(Discovery::Lost((*arguments.name()).to_owned()));
                }
            }
            Report::Failed(signal) => {
                let error = signal
                    .args()
                    .map(|arguments| arguments.error().to_owned())
                    .unwrap_or_default();
                let _ = browser.free().await;
                return Err(format!("browsing through Avahi failed: {error}"));
            }
        }
    }
    Err("Avahi stopped reporting hosts".to_owned())
}

/// Resolves the host `name` found on `interface`, at an IPv4 address if it
/// has one.
async fn resolve(
    server: &BrowsingServerProxy<'_>,
    interface: i32,
    name: &str,
) -> Result<FoundHost, String> {
    let resolve = |address_protocol| {
        server.resolve_service(
            interface,
            ANY_PROTOCOL,
            name,
            SERVICE_TYPE,
            DEFAULT_DOMAIN,
            address_protocol,
            NO_FLAGS,
        )
    };
    let resolved = match resolve(IPV4).await {
        Ok(resolved) => resolved,
        Err(_) => resolve(ANY_PROTOCOL)
            .await
            .map_err(|error| error.to_string())?,
    };
    let (interface, _, name, _, _, _, _, address, port, txt, _) = resolved;
    let address = socket_address(&address, port, interface)
        .ok_or_else(|| format!("an unreadable address: {address}"))?;
    Ok(FoundHost {
        name,
        address,
        record: ServiceRecord::parse_strings(txt.iter().map(Vec::as_slice)),
    })
}

/// Returns the socket address of `address` and `port`, an IPv6 link-local
/// one scoped to the interface it was found on, which it is reached
/// through.
fn socket_address(address: &str, port: u16, interface: i32) -> Option<SocketAddr> {
    match address.parse::<IpAddr>().ok()? {
        IpAddr::V6(ip) if ip.is_unicast_link_local() => {
            let scope = u32::try_from(interface).unwrap_or(0);
            Some(SocketAddr::V6(SocketAddrV6::new(ip, port, 0, scope)))
        }
        ip => Some(SocketAddr::new(ip, port)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Addresses keep their port, and an IPv6 link-local one gains its
    /// interface as its scope.
    #[test]
    fn reads_addresses() {
        assert_eq!(
            socket_address("192.168.1.44", 4433, 3),
            Some("192.168.1.44:4433".parse().unwrap())
        );
        let link_local = socket_address("fe80::1", 4433, 3).unwrap();
        assert_eq!(link_local.to_string(), "[fe80::1%3]:4433");
        assert_eq!(
            socket_address("2001:db8::1", 4433, 3),
            Some("[2001:db8::1]:4433".parse().unwrap())
        );
        assert_eq!(socket_address("exile.local", 4433, 3), None);
    }
}
