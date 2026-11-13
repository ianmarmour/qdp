//! Finding hosts on the local network, which advertise themselves with
//! DNS-SD as `_qdp._udp`: over Multicast DNS of QDP's own on Linux, for the
//! Linux app's list of computers and `qdp discover`.

use std::net::SocketAddr;

use qdp::discovery::ServiceRecord;

/// A host found on the local network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundHost {
    /// Its instance name, the host's name for users.
    pub name: String,
    /// Where it listens.
    pub address: SocketAddr,
    /// What its TXT record says, if it can be read.
    pub record: Option<ServiceRecord>,
}

/// What browsing tells of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    /// A host was found, or found on another interface.
    Found(FoundHost),
    /// The host of this instance name went away from an interface.
    Lost(String),
}

/// Browses for hosts, telling `report` of each found and gone, until
/// browsing fails.
///
/// # Errors
///
/// Fails if Multicast DNS cannot start, or on systems this crate does not
/// browse on yet.
pub async fn browse(report: impl FnMut(Discovery) + Send) -> Result<(), String> {
    #[cfg(all(target_os = "linux", feature = "discovery"))]
    {
        crate::linux::mdns::browse(report).await
    }
    #[cfg(not(all(target_os = "linux", feature = "discovery")))]
    {
        let _ = report;
        Err("finding hosts on the network is not available on this system yet".to_owned())
    }
}
