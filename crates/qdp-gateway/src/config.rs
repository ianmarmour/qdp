//! The gateway's configuration file, in TOML: its name and address, the
//! organization's directory, TOTP, sessions, identity assertions, and the
//! hosts it reaches, with who may reach each.
//!
//! ```toml
//! name = "Example Corp"
//! listen = "0.0.0.0:443"
//! state_directory = "/var/lib/qdp-gateway"
//!
//! [directory]
//! url = "ldaps://dc1.corp.example.com"
//! bind_dn = "CN=qdp-gateway,OU=Service Accounts,DC=corp,DC=example,DC=com"
//! bind_password_file = "/etc/qdp-gateway/directory-password"
//! base_dn = "DC=corp,DC=example,DC=com"
//!
//! [totp]
//! enrollment = "first-login"
//!
//! [[host]]
//! id = "build-01"
//! name = "Build machine"
//! address = "10.0.4.20:4433"
//! pin = "59b81a9e89682a79876919776020611407a3fd03556002d7d25d0c4609584bb7"
//! groups = ["QDP Users"]
//!
//! [[route]]
//! names = ["*.desktops.corp.example.com"]
//! networks = ["10.20.0.0/16"]
//! groups = ["QDP Users"]
//! ```
//!
//! Hosts are listed to users and opened by their IDs. Routes let users open
//! hosts by their names inside the network, as a client configured with the
//! gateway does (Section 13.5).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use qdp::gateway::assertion::MAX_NAME_LENGTH;
use qdp::gateway::{
    DEFAULT_GATEWAY_PORT, MAX_DEVICE_NAME_LENGTH, MAX_FIELD_LENGTH, MAX_HOST_NAME_LENGTH,
};
use qdp_quic::KeyPin;
use serde::Deserialize;

/// The placeholder in the user filter that the user's name replaces.
pub const USER_PLACEHOLDER: &str = "{user}";

/// The filter that finds a user by default: Active Directory's account name.
const DEFAULT_USER_FILTER: &str = "(&(objectClass=user)(sAMAccountName={user}))";

/// The longest a session token may be configured to last: a week.
const MAX_TOKEN_LIFETIME_HOURS: u64 = 7 * 24;

/// The longest an identity assertion may be configured to last; Section 13.7
/// asks for 2 minutes, which leaves room for slow clients.
const MAX_ASSERTION_LIFETIME_SECONDS: u64 = 5 * 60;

/// The server name hosts are reached as by default, which `qdp serve`'s
/// certificate names.
const DEFAULT_SERVER_NAME: &str = "qdp-host";

/// The port QDP hosts listen on by default.
pub const DEFAULT_HOST_PORT: u16 = 4433;

/// Why a configuration cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file cannot be read.
    #[error("cannot read {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// Why.
        source: io::Error,
    },
    /// The file is not valid TOML of the expected shape.
    #[error("{path} is not a valid configuration: {source}")]
    Parse {
        /// The file.
        path: PathBuf,
        /// Why.
        source: Box<toml::de::Error>,
    },
    /// A value breaks a rule.
    #[error("{0}")]
    Invalid(String),
}

/// The gateway's configuration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The gateway's name for users, at most 64 bytes.
    pub name: String,
    /// The UDP address to listen on.
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    /// Where the gateway keeps its keys and TOTP secrets.
    #[serde(default = "default_state_directory")]
    pub state_directory: PathBuf,
    /// A certificate from the organization's certificate authority, in place
    /// of the self-signed one the gateway makes itself.
    #[serde(default)]
    pub tls: Option<TlsConfig>,
    /// The organization's directory.
    pub directory: DirectoryConfig,
    /// TOTP, which is always required (Section 13.4).
    #[serde(default)]
    pub totp: TotpConfig,
    /// Session tokens.
    #[serde(default)]
    pub sessions: SessionConfig,
    /// Identity assertions.
    #[serde(default)]
    pub assertions: AssertionConfig,
    /// Limits on failed sign-ins.
    #[serde(default)]
    pub limits: LimitsConfig,
    /// The hosts users may reach, listed to them.
    #[serde(default, rename = "host")]
    pub hosts: Vec<HostConfig>,
    /// The hosts users may reach by their names inside the network.
    #[serde(default, rename = "route")]
    pub routes: Vec<RouteConfig>,
}

/// A certificate and private key in PEM.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    /// The certificate chain, end-entity certificate first.
    pub certificate: PathBuf,
    /// The private key.
    pub private_key: PathBuf,
}

/// How to reach the directory and find users in it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryConfig {
    /// The server: `ldaps://` for TLS from the start, or `ldap://` with
    /// `starttls`.
    pub url: String,
    /// Whether to upgrade an `ldap://` connection with StartTLS.
    #[serde(default)]
    pub starttls: bool,
    /// The certificate authority the server's certificate chains to, in PEM,
    /// if the system does not trust it.
    #[serde(default)]
    pub ca_certificate: Option<PathBuf>,
    /// The account the gateway binds as to find users.
    pub bind_dn: String,
    /// The file holding that account's password.
    pub bind_password_file: PathBuf,
    /// Where users are searched for.
    pub base_dn: String,
    /// The filter that finds a user, with `{user}` for the name they type.
    #[serde(default = "default_user_filter")]
    pub user_filter: String,
    /// The attribute of the user's name, which TOTP secrets and assertions
    /// know the user by, whatever case they typed it in.
    #[serde(default = "default_name_attribute")]
    pub name_attribute: String,
    /// The attribute of the user's full name.
    #[serde(default = "default_display_name_attribute")]
    pub display_name_attribute: String,
    /// The attribute listing the groups the user is in.
    #[serde(default = "default_group_attribute")]
    pub group_attribute: String,
    /// How long to wait for the directory, in seconds.
    #[serde(default = "default_directory_timeout")]
    pub timeout_seconds: u64,
}

/// Who may enroll a TOTP secret as they sign in (Section 4.4.10).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Enrollment {
    /// Only an administrator enrolls secrets, with `qdp-gateway totp enroll`.
    #[default]
    AdminOnly,
    /// A user without a secret enrolls one the first time they sign in.
    FirstLogin,
}

/// TOTP settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TotpConfig {
    /// The issuer authenticator apps show beside the codes; the gateway's
    /// name if not given.
    #[serde(default)]
    pub issuer: Option<String>,
    /// Who may enroll a secret.
    #[serde(default)]
    pub enrollment: Enrollment,
}

/// Session token settings.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionConfig {
    /// How long a session token lasts, in hours: a working day by default.
    #[serde(default = "default_token_lifetime_hours")]
    pub token_lifetime_hours: u64,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            token_lifetime_hours: default_token_lifetime_hours(),
        }
    }
}

/// Identity assertion settings (Section 13.7).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertionConfig {
    /// Whether the gateway vouches for users to hosts.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// How long an assertion lasts, in seconds.
    #[serde(default = "default_assertion_lifetime_seconds")]
    pub lifetime_seconds: u64,
    /// The groups an assertion reports, of those the user is in.
    #[serde(default)]
    pub groups: Vec<String>,
}

impl Default for AssertionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            lifetime_seconds: default_assertion_lifetime_seconds(),
            groups: Vec::new(),
        }
    }
}

/// Limits on failed sign-ins (Section 4.4.1), which keep attempts through the
/// gateway below the directory's own lockout threshold (Section 13.4).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitsConfig {
    /// Failures for one user name within the window, after which the gateway
    /// stops trying it until the window passes.
    #[serde(default = "default_user_failures")]
    pub user_failures: u32,
    /// Failures from one address within the window, after which the gateway
    /// refuses it until the window passes.
    #[serde(default = "default_address_failures")]
    pub address_failures: u32,
    /// The window, in minutes.
    #[serde(default = "default_window_minutes")]
    pub window_minutes: u64,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            user_failures: default_user_failures(),
            address_failures: default_address_failures(),
            window_minutes: default_window_minutes(),
        }
    }
}

impl LimitsConfig {
    /// Returns the window.
    pub fn window(&self) -> Duration {
        Duration::from_secs(self.window_minutes * 60)
    }
}

/// Hosts users may reach by their names or addresses inside the network.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    /// The names the route allows: exact, or `*.` and a domain for every name
    /// in it. Empty to allow any name that resolves into `networks`.
    #[serde(default)]
    pub names: Vec<String>,
    /// The networks every address of a host must lie in, as `10.20.0.0/16`.
    /// Empty to allow any address the names resolve to.
    #[serde(default)]
    pub networks: Vec<String>,
    /// The ports the route allows.
    #[serde(default = "default_ports")]
    pub ports: Vec<u16>,
    /// The groups whose members may use the route.
    #[serde(default)]
    pub groups: Vec<String>,
    /// The users who may use the route, besides those groups' members.
    #[serde(default)]
    pub users: Vec<String>,
}

/// A network, as an address and a prefix length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    /// The network's address.
    address: IpAddr,
    /// The number of leading bits that name the network.
    prefix: u8,
}

impl Network {
    /// Reads `10.20.0.0/16`, `fd00::/8`, or a single address.
    pub fn parse(text: &str) -> Option<Self> {
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => {
                (address.parse::<IpAddr>().ok()?, Some(prefix.parse().ok()?))
            }
            None => (text.parse::<IpAddr>().ok()?, None),
        };
        let bits = if address.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(bits);
        (prefix <= bits).then_some(Self { address, prefix })
    }

    /// Returns whether `address` lies in the network.
    pub fn contains(&self, address: IpAddr) -> bool {
        /// Returns whether the first `prefix` bits of `a` and `b` match.
        fn same_prefix(a: &[u8], b: &[u8], prefix: u8) -> bool {
            let whole = usize::from(prefix / 8);
            let rest = prefix % 8;
            a[..whole] == b[..whole] && (rest == 0 || (a[whole] ^ b[whole]) >> (8 - rest) == 0)
        }
        match (self.address, address) {
            (IpAddr::V4(network), IpAddr::V4(address)) => {
                same_prefix(&network.octets(), &address.octets(), self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V6(address)) => {
                same_prefix(&network.octets(), &address.octets(), self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V4(address)) => {
                // An IPv4-mapped network holds the IPv4 addresses it maps.
                network.to_ipv4_mapped().is_some_and(|mapped| {
                    self.prefix >= 96
                        && same_prefix(&mapped.octets(), &address.octets(), self.prefix - 96)
                })
            }
            (IpAddr::V4(_), IpAddr::V6(address)) => address
                .to_ipv4_mapped()
                .is_some_and(|mapped| self.contains(IpAddr::V4(mapped))),
        }
    }
}

/// A host users may reach.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostConfig {
    /// The gateway's identifier of the host, at most 255 bytes.
    pub id: String,
    /// Its name for users, at most 64 bytes.
    pub name: String,
    /// More about it, at most 255 bytes.
    #[serde(default)]
    pub description: String,
    /// Where it listens, as a name or address with a port.
    pub address: String,
    /// Its key pin, as `qdp pin` prints it.
    pub pin: String,
    /// The name its certificate is for.
    #[serde(default = "default_server_name")]
    pub server_name: String,
    /// The groups whose members may reach it.
    #[serde(default)]
    pub groups: Vec<String>,
    /// The users who may reach it, besides those groups' members.
    #[serde(default)]
    pub users: Vec<String>,
}

impl HostConfig {
    /// Returns the host's key pin, which [`Config::load`] checked.
    pub fn key_pin(&self) -> KeyPin {
        KeyPin::from_hex(&self.pin).unwrap_or(KeyPin([0; 32]))
    }
}

impl Config {
    /// Reads and checks the configuration in `path`.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let config: Self = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source: Box::new(source),
        })?;
        config.check()?;
        Ok(config)
    }

    /// Returns the issuer authenticator apps show.
    pub fn issuer(&self) -> &str {
        self.totp.issuer.as_deref().unwrap_or(&self.name)
    }

    /// Returns how long a session token lasts.
    pub fn token_lifetime(&self) -> Duration {
        Duration::from_secs(self.sessions.token_lifetime_hours * 60 * 60)
    }

    /// Checks the rules TOML cannot express.
    fn check(&self) -> Result<(), ConfigError> {
        let invalid = |message: String| Err(ConfigError::Invalid(message));
        if self.name.is_empty() || self.name.len() > MAX_DEVICE_NAME_LENGTH {
            return invalid(format!(
                "name must have 1 to {MAX_DEVICE_NAME_LENGTH} bytes"
            ));
        }
        let directory = &self.directory;
        match directory.url.split_once("://").map(|(scheme, _)| scheme) {
            Some("ldaps") => {}
            Some("ldap") if directory.starttls => {}
            Some("ldap") => {
                return invalid(
                    "directory.url is ldap:// without starttls, which would send users' \
                     passwords unencrypted: use ldaps://, or set starttls = true"
                        .to_owned(),
                );
            }
            _ => return invalid("directory.url must start with ldaps:// or ldap://".to_owned()),
        }
        if !directory.user_filter.contains(USER_PLACEHOLDER) {
            return invalid(format!(
                "directory.user_filter must contain {USER_PLACEHOLDER}"
            ));
        }
        if directory.timeout_seconds == 0 {
            return invalid("directory.timeout_seconds must be at least 1".to_owned());
        }
        if !(1..=MAX_TOKEN_LIFETIME_HOURS).contains(&self.sessions.token_lifetime_hours) {
            return invalid(format!(
                "sessions.token_lifetime_hours must be 1 to {MAX_TOKEN_LIFETIME_HOURS}"
            ));
        }
        if !(1..=MAX_ASSERTION_LIFETIME_SECONDS).contains(&self.assertions.lifetime_seconds) {
            return invalid(format!(
                "assertions.lifetime_seconds must be 1 to {MAX_ASSERTION_LIFETIME_SECONDS}"
            ));
        }
        if self.issuer().is_empty() || self.issuer().len() > MAX_NAME_LENGTH {
            return invalid(format!(
                "totp.issuer must have 1 to {MAX_NAME_LENGTH} bytes"
            ));
        }
        let mut ids = HashSet::new();
        for host in &self.hosts {
            let id = &host.id;
            if id.is_empty() || id.len() > MAX_FIELD_LENGTH {
                return invalid(format!(
                    "host id {id:?} must have 1 to {MAX_FIELD_LENGTH} bytes"
                ));
            }
            if !ids.insert(id) {
                return invalid(format!("host id {id:?} is used twice"));
            }
            if host.name.is_empty() || host.name.len() > MAX_HOST_NAME_LENGTH {
                return invalid(format!(
                    "host {id:?}: name must have 1 to {MAX_HOST_NAME_LENGTH} bytes"
                ));
            }
            if host.description.len() > MAX_FIELD_LENGTH
                || host.server_name.len() > MAX_FIELD_LENGTH
            {
                return invalid(format!(
                    "host {id:?}: description and server_name have at most {MAX_FIELD_LENGTH} bytes"
                ));
            }
            if KeyPin::from_hex(&host.pin).is_none() {
                return invalid(format!(
                    "host {id:?}: pin must be 64 hexadecimal digits, as `qdp pin` prints"
                ));
            }
            if host
                .address
                .rsplit_once(':')
                .is_none_or(|(_, port)| port.parse::<u16>().is_err())
            {
                return invalid(format!(
                    "host {id:?}: address must have a port, as in 10.0.4.20:4433"
                ));
            }
            if host.groups.is_empty() && host.users.is_empty() {
                return invalid(format!(
                    "host {id:?} names no groups or users, so no one could reach it"
                ));
            }
        }
        for (index, route) in self.routes.iter().enumerate() {
            let number = index + 1;
            if route.names.is_empty() && route.networks.is_empty() {
                return invalid(format!(
                    "route {number} names no names or networks, and would reach every host"
                ));
            }
            if let Some(network) = route
                .networks
                .iter()
                .find(|text| Network::parse(text).is_none())
            {
                return invalid(format!(
                    "route {number}: {network:?} is not a network, as in 10.20.0.0/16"
                ));
            }
            if route.ports.is_empty() || route.groups.is_empty() && route.users.is_empty() {
                return invalid(format!(
                    "route {number} names no ports, or no groups or users, so no one could use it"
                ));
            }
        }
        if self.limits.user_failures == 0
            || self.limits.address_failures == 0
            || self.limits.window_minutes == 0
        {
            return invalid("limits must be at least 1".to_owned());
        }
        Ok(())
    }
}

/// The address gateways listen on by default.
fn default_listen() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], DEFAULT_GATEWAY_PORT))
}

/// The state directory by default.
fn default_state_directory() -> PathBuf {
    crate::locations::default_state_directory()
}

/// The user filter by default.
fn default_user_filter() -> String {
    DEFAULT_USER_FILTER.to_owned()
}

/// The name attribute by default, Active Directory's account name.
fn default_name_attribute() -> String {
    "sAMAccountName".to_owned()
}

/// Failures per user name by default: below Active Directory's common lockout
/// thresholds.
fn default_user_failures() -> u32 {
    5
}

/// Failures per address by default.
fn default_address_failures() -> u32 {
    20
}

/// The failure window by default, in minutes.
fn default_window_minutes() -> u64 {
    15
}

/// The ports routes allow by default.
fn default_ports() -> Vec<u16> {
    vec![DEFAULT_HOST_PORT]
}

/// The display name attribute by default, Active Directory's and inetOrgPerson's.
fn default_display_name_attribute() -> String {
    "displayName".to_owned()
}

/// The group attribute by default, Active Directory's and OpenLDAP's memberof
/// overlay's.
fn default_group_attribute() -> String {
    "memberOf".to_owned()
}

/// How long to wait for the directory by default, in seconds.
fn default_directory_timeout() -> u64 {
    10
}

/// How long session tokens last by default: a working day.
fn default_token_lifetime_hours() -> u64 {
    12
}

/// How long assertions last by default (Section 13.7).
fn default_assertion_lifetime_seconds() -> u64 {
    120
}

/// The server name of hosts by default.
fn default_server_name() -> String {
    DEFAULT_SERVER_NAME.to_owned()
}

/// `true`, for settings on by default.
fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A complete configuration.
    const EXAMPLE: &str = r#"
        name = "Example Corp"

        [directory]
        url = "ldaps://dc1.corp.example.com"
        bind_dn = "CN=qdp-gateway,OU=Service Accounts,DC=corp,DC=example,DC=com"
        bind_password_file = "/etc/qdp-gateway/directory-password"
        base_dn = "DC=corp,DC=example,DC=com"

        [totp]
        enrollment = "first-login"

        [[host]]
        id = "build-01"
        name = "Build machine"
        address = "10.0.4.20:4433"
        pin = "59b81a9e89682a79876919776020611407a3fd03556002d7d25d0c4609584bb7"
        groups = ["QDP Users"]
    "#;

    /// Parses and checks `text`.
    fn parse(text: &str) -> Result<Config, ConfigError> {
        let config: Config = toml::from_str(text).map_err(|source| ConfigError::Parse {
            path: PathBuf::new(),
            source: Box::new(source),
        })?;
        config.check()?;
        Ok(config)
    }

    #[test]
    fn reads_a_configuration_with_defaults() {
        let config = parse(EXAMPLE).unwrap();
        assert_eq!(config.listen, "0.0.0.0:443".parse().unwrap());
        assert_eq!(
            config.state_directory,
            PathBuf::from("/var/lib/qdp-gateway")
        );
        assert_eq!(
            config.directory.user_filter,
            "(&(objectClass=user)(sAMAccountName={user}))"
        );
        assert_eq!(config.directory.group_attribute, "memberOf");
        assert_eq!(config.totp.enrollment, Enrollment::FirstLogin);
        assert_eq!(config.issuer(), "Example Corp");
        assert_eq!(config.token_lifetime(), Duration::from_secs(12 * 3600));
        assert!(config.assertions.enabled);
        assert_eq!(config.assertions.lifetime_seconds, 120);
        let host = &config.hosts[0];
        assert_eq!(host.server_name, "qdp-host");
        assert_eq!(host.key_pin().to_hex(), host.pin);
    }

    #[test]
    fn reads_routes_and_networks() {
        let text = format!(
            "{EXAMPLE}\n[[route]]\nnames = [\"*.desktops.corp.example.com\"]\nnetworks = [\"10.20.0.0/16\"]\ngroups = [\"QDP Users\"]\n"
        );
        let config = parse(&text).unwrap();
        assert_eq!(config.routes[0].ports, [4433]);
        let network = Network::parse("10.20.0.0/16").unwrap();
        assert!(network.contains("10.20.255.1".parse().unwrap()));
        assert!(!network.contains("10.21.0.1".parse().unwrap()));
        assert!(network.contains("::ffff:10.20.0.9".parse().unwrap()));
        assert!(
            Network::parse("10.0.4.20")
                .unwrap()
                .contains("10.0.4.20".parse().unwrap())
        );
        assert!(
            Network::parse("fd00::/8")
                .unwrap()
                .contains("fd12::1".parse().unwrap())
        );
        assert!(Network::parse("10.0.0.0/33").is_none());
        let open = text.replace(
            "names = [\"*.desktops.corp.example.com\"]\nnetworks = [\"10.20.0.0/16\"]\n",
            "",
        );
        assert!(parse(&open).is_err());
    }

    #[test]
    fn refuses_unencrypted_directories() {
        let plain = EXAMPLE.replace("ldaps://", "ldap://");
        assert!(matches!(parse(&plain), Err(ConfigError::Invalid(_))));
        let upgraded = plain.replace(
            "url = \"ldap://dc1.corp.example.com\"",
            "url = \"ldap://dc1.corp.example.com\"\nstarttls = true",
        );
        assert!(parse(&upgraded).is_ok());
    }

    #[test]
    fn refuses_mistakes() {
        for (from, to) in [
            ("name = \"Example Corp\"", "name = \"\""),
            ("pin = \"59b8", "pin = \"zz"),
            ("10.0.4.20:4433", "10.0.4.20"),
            ("groups = [\"QDP Users\"]", ""),
            ("[totp]", "[totp]\nunknown = 1"),
            ("enrollment = \"first-login\"", "enrollment = \"always\""),
        ] {
            assert!(parse(&EXAMPLE.replace(from, to)).is_err(), "{to}");
        }
        let twice = format!(
            "{EXAMPLE}\n[[host]]\nid = \"build-01\"\nname = \"Again\"\naddress = \"10.0.4.21:4433\"\npin = \"{}\"\nusers = [\"ian\"]\n",
            "ab".repeat(32)
        );
        assert!(parse(&twice).is_err());
    }
}
