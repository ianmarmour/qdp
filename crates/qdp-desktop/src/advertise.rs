//! Advertising the host on the local network with DNS-SD (Section 3.13), so
//! that clients list it without the user typing its address: through Avahi
//! on Linux, which GNOME, KDE and most distributions run.

use std::net::SocketAddr;

use qdp::discovery::ServiceRecord;

/// What a host advertises.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advertisement {
    /// The host's name for users, which becomes the service's instance name.
    pub name: String,
    /// The address the host listens on: on all of the machine's addresses,
    /// or the service would point clients to addresses it does not listen
    /// on.
    pub address: SocketAddr,
    /// The service's TXT record.
    pub record: ServiceRecord,
}

/// A service being advertised. Dropping it stops advertising when the
/// process leaves the bus; [`Advertising::withdraw`] stops at once.
#[derive(Debug)]
pub struct Advertising {
    /// The platform's advertisement.
    #[cfg(all(target_os = "linux", feature = "discovery"))]
    inner: crate::linux::avahi::Advertising,
}

impl Advertising {
    /// Stops advertising.
    pub async fn withdraw(self) {
        #[cfg(all(target_os = "linux", feature = "discovery"))]
        self.inner.withdraw().await;
    }
}

/// Starts advertising `advertisement` on the local network. Under a name
/// another service holds, the service takes the next free one, such as
/// `studio #2`.
///
/// # Errors
///
/// Fails if the host listens on one address only, if the system has no
/// DNS-SD service, such as when Avahi is not running, or on systems this
/// crate does not advertise on yet.
pub async fn advertise(advertisement: Advertisement) -> Result<Advertising, String> {
    if !advertisement.address.ip().is_unspecified() {
        return Err(format!(
            "the host listens on {} only, and clients would find its other addresses",
            advertisement.address.ip()
        ));
    }
    #[cfg(all(target_os = "linux", feature = "discovery"))]
    {
        crate::linux::avahi::advertise(advertisement)
            .await
            .map(|inner| Advertising { inner })
    }
    #[cfg(not(all(target_os = "linux", feature = "discovery")))]
    {
        let _ = advertisement;
        Err("this system has no local network advertising yet".to_owned())
    }
}
