//! Advertising through Avahi's D-Bus interface: an entry group holding the
//! host's service, kept under a free name when another service takes it.
//!
//! Avahi frees a client's entry groups when the client leaves the system
//! bus, so the service also goes away if the host exits without
//! withdrawing it.

use futures_lite::StreamExt;
use qdp::discovery::{SERVICE_TYPE, instance_name};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use zbus::Connection;
use zbus::zvariant::OwnedObjectPath;

use crate::advertise::Advertisement;

/// Avahi's code for every network interface.
const ANY_INTERFACE: i32 = -1;

/// Avahi's code for both IPv4 and IPv6.
const ANY_PROTOCOL: i32 = -1;

/// Avahi's code for IPv4.
const IPV4: i32 = 0;

/// No flags for `AddService`.
const NO_FLAGS: u32 = 0;

/// The domain Avahi chooses, `local`.
const DEFAULT_DOMAIN: &str = "";

/// The machine's own host name, for the SRV record.
const THIS_HOST: &str = "";

/// The error `AddService` returns when another of the machine's services
/// holds the name.
const COLLISION_ERROR: &str = "org.freedesktop.Avahi.CollisionError";

/// How many names a service tries before giving up.
const MAX_RENAMES: u32 = 32;

/// The entry group state once its records are published.
const ESTABLISHED: i32 = 2;

/// The entry group state when another service holds its name.
const COLLISION: i32 = 3;

/// The entry group state when publishing failed.
const FAILURE: i32 = 4;

/// Avahi's server.
#[zbus::proxy(
    interface = "org.freedesktop.Avahi.Server",
    default_service = "org.freedesktop.Avahi",
    default_path = "/",
    gen_blocking = false
)]
trait Server {
    /// Creates an entry group.
    fn entry_group_new(&self) -> zbus::Result<OwnedObjectPath>;

    /// Returns the name to take when `name` is held.
    fn get_alternative_service_name(&self, name: &str) -> zbus::Result<String>;
}

/// A group of records Avahi publishes together.
#[zbus::proxy(
    interface = "org.freedesktop.Avahi.EntryGroup",
    default_service = "org.freedesktop.Avahi",
    gen_blocking = false
)]
trait EntryGroup {
    /// Adds a DNS-SD service.
    #[allow(clippy::too_many_arguments)]
    fn add_service(
        &self,
        interface: i32,
        protocol: i32,
        flags: u32,
        name: &str,
        service_type: &str,
        domain: &str,
        host: &str,
        port: u16,
        txt: &[&[u8]],
    ) -> zbus::Result<()>;

    /// Publishes the group's records.
    fn commit(&self) -> zbus::Result<()>;

    /// Removes the group's records.
    fn reset(&self) -> zbus::Result<()>;

    /// Removes the group and its records.
    fn free(&self) -> zbus::Result<()>;

    /// Reports that the group's state changed.
    #[zbus(signal)]
    fn state_changed(&self, state: i32, error: &str) -> zbus::Result<()>;
}

/// The service as Avahi publishes it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Service {
    /// The instance name.
    name: String,
    /// The protocols the host is reached on.
    protocol: i32,
    /// The UDP port the host listens on.
    port: u16,
    /// The TXT record's `key=value` strings.
    txt: Vec<String>,
}

impl Service {
    /// Adds the service to `group` and publishes it, under the next free
    /// name while one of this machine's other services holds its own.
    async fn publish(
        &mut self,
        server: &ServerProxy<'_>,
        group: &EntryGroupProxy<'_>,
    ) -> zbus::Result<()> {
        let txt: Vec<&[u8]> = self.txt.iter().map(String::as_bytes).collect();
        let mut renames = 0;
        loop {
            let added = group
                .add_service(
                    ANY_INTERFACE,
                    self.protocol,
                    NO_FLAGS,
                    &self.name,
                    SERVICE_TYPE,
                    DEFAULT_DOMAIN,
                    THIS_HOST,
                    self.port,
                    &txt,
                )
                .await;
            match added {
                Err(error) if is_collision(&error) && renames < MAX_RENAMES => {
                    renames += 1;
                    self.name = server.get_alternative_service_name(&self.name).await?;
                }
                added => break added?,
            }
        }
        group.commit().await
    }
}

/// Returns whether `error` is Avahi's report that a service of this
/// machine holds the name.
fn is_collision(error: &zbus::Error) -> bool {
    matches!(error, zbus::Error::MethodError(name, ..) if name.as_str() == COLLISION_ERROR)
}

/// A service Avahi publishes.
#[derive(Debug)]
pub(crate) struct Advertising {
    /// The service's entry group.
    group: EntryGroupProxy<'static>,
    /// The task that keeps the service under a free name.
    task: JoinHandle<()>,
}

impl Advertising {
    /// Removes the service.
    pub(crate) async fn withdraw(self) {
        self.task.abort();
        if let Err(error) = self.group.free().await {
            warn!(%error, "cannot stop advertising on the local network");
        }
    }
}

impl Drop for Advertising {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Publishes `advertisement` through Avahi.
pub(crate) async fn advertise(advertisement: Advertisement) -> Result<Advertising, String> {
    let connection = Connection::system()
        .await
        .map_err(|error| format!("cannot reach the system bus: {error}"))?;
    let unavailable = |error: zbus::Error| format!("cannot advertise through Avahi: {error}");
    let server = ServerProxy::new(&connection).await.map_err(unavailable)?;
    let path = server.entry_group_new().await.map_err(unavailable)?;
    let group = EntryGroupProxy::builder(&connection)
        .path(path)
        .map_err(unavailable)?
        .build()
        .await
        .map_err(unavailable)?;
    let states = group.receive_state_changed().await.map_err(unavailable)?;
    let mut service = Service {
        name: instance_name(&advertisement.name),
        protocol: if advertisement.address.is_ipv4() {
            IPV4
        } else {
            ANY_PROTOCOL
        },
        port: advertisement.address.port(),
        txt: advertisement.record.strings(),
    };
    service
        .publish(&server, &group)
        .await
        .map_err(unavailable)?;
    let task = tokio::spawn(keep_published(server, group.clone(), service, states));
    Ok(Advertising { group, task })
}

/// Follows the group's state, moving the service to a free name whenever
/// another service holds its own.
async fn keep_published(
    server: ServerProxy<'static>,
    group: EntryGroupProxy<'static>,
    mut service: Service,
    mut states: StateChangedStream,
) {
    while let Some(change) = states.next().await {
        let Ok(arguments) = change.args() else {
            continue;
        };
        match *arguments.state() {
            ESTABLISHED => info!(name = service.name, "advertising on the local network"),
            COLLISION => {
                let renamed = async {
                    service.name = server.get_alternative_service_name(&service.name).await?;
                    group.reset().await?;
                    service.publish(&server, &group).await
                };
                if let Err(error) = renamed.await {
                    warn!(%error, "stopped advertising on the local network: its name is taken");
                    return;
                }
            }
            FAILURE => {
                warn!(
                    error = arguments.error(),
                    "advertising on the local network failed"
                );
                return;
            }
            _ => {}
        }
    }
}
