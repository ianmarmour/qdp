//! Who may reach which host (Section 13.5): the listed hosts each name the
//! groups and users that may reach them, and routes let users open hosts by
//! their names and addresses inside the network.

use std::net::IpAddr;

use crate::config::{Config, DEFAULT_HOST_PORT, HostConfig, Network, RouteConfig};
use crate::directory::DirectoryUser;

/// The longest name a target may have, in bytes (Section 13.5).
const MAX_TARGET_LENGTH: usize = 255;

/// What an `OPEN` names, once it is not a listed host's ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedTarget {
    /// The host's name or address, lowercase and without brackets.
    pub name: String,
    /// The port.
    pub port: u16,
}

impl RoutedTarget {
    /// Reads a name or address with an optional port: `build-01`,
    /// `build-01.corp.example.com:4433`, `10.0.4.20` or `[fd00::1]:4433`.
    pub fn parse(text: &str) -> Option<Self> {
        if text.is_empty() || text.len() > MAX_TARGET_LENGTH {
            return None;
        }
        let (name, port) = if let Some(rest) = text.strip_prefix('[') {
            let (address, after) = rest.split_once(']')?;
            let port = match after {
                "" => DEFAULT_HOST_PORT,
                after => after.strip_prefix(':')?.parse().ok()?,
            };
            (address, port)
        } else {
            match text.rsplit_once(':') {
                // A bare IPv6 address has colons but no port.
                Some(_) if text.parse::<IpAddr>().is_ok() => (text, DEFAULT_HOST_PORT),
                Some((name, port)) => (name, port.parse().ok()?),
                None => (text, DEFAULT_HOST_PORT),
            }
        };
        let name = name.trim_end_matches('.').to_ascii_lowercase();
        let valid = name.parse::<IpAddr>().is_ok()
            || (!name.is_empty()
                && name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_')
                }));
        valid.then_some(Self { name, port })
    }

    /// Returns the target's address, if its name is one.
    pub fn address(&self) -> Option<IpAddr> {
        self.name.parse().ok()
    }
}

/// Returns whether `user` is among `users` or a member of one of `groups`.
fn admits(user: &DirectoryUser, groups: &[String], users: &[String]) -> bool {
    users
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&user.name))
        || groups.iter().any(|group| {
            user.groups
                .iter()
                .any(|member_of| group_matches(group, member_of))
        })
}

/// Returns whether the configured `group` names the group the directory
/// reports as `member_of`: by its whole distinguished name, or by the value of
/// its first part, as `QDP Users` for `CN=QDP Users,OU=Groups,DC=corp,…`.
pub fn group_matches(group: &str, member_of: &str) -> bool {
    if group.eq_ignore_ascii_case(member_of) {
        return true;
    }
    let first = member_of.split(',').next().unwrap_or_default();
    first
        .split_once('=')
        .is_some_and(|(_, value)| value.trim().eq_ignore_ascii_case(group))
}

/// Returns the listed hosts `user` may reach, in the configuration's order.
pub fn reachable_hosts<'c>(config: &'c Config, user: &DirectoryUser) -> Vec<&'c HostConfig> {
    config
        .hosts
        .iter()
        .filter(|host| admits(user, &host.groups, &host.users))
        .collect()
}

/// Returns the listed host with ID `id`, if `user` may reach it.
pub fn reachable_host<'c>(
    config: &'c Config,
    user: &DirectoryUser,
    id: &str,
) -> Option<&'c HostConfig> {
    config
        .hosts
        .iter()
        .find(|host| host.id == id)
        .filter(|host| admits(user, &host.groups, &host.users))
}

/// Returns whether `name` matches a route's name `pattern`: the same name, or
/// a name in the domain of a `*.` pattern.
fn name_matches(pattern: &str, name: &str) -> bool {
    let pattern = pattern.trim_end_matches('.').to_ascii_lowercase();
    match pattern.strip_prefix("*.") {
        Some(domain) => name
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.')),
        None => pattern == name,
    }
}

/// Returns whether `route` lets `user` reach `target` at `addresses`, the
/// addresses its name resolves to: the name and every address must be
/// allowed, so that a name resolving outside the route's networks is
/// refused (Section 13.5).
fn route_allows(
    route: &RouteConfig,
    user: &DirectoryUser,
    target: &RoutedTarget,
    addresses: &[IpAddr],
) -> bool {
    let networks: Vec<_> = route
        .networks
        .iter()
        .filter_map(|text| Network::parse(text))
        .collect();
    let name_allowed = route.names.is_empty()
        || route
            .names
            .iter()
            .any(|pattern| name_matches(pattern, &target.name));
    let addresses_allowed = !addresses.is_empty()
        && (networks.is_empty()
            || addresses
                .iter()
                .all(|address| networks.iter().any(|network| network.contains(*address))));
    admits(user, &route.groups, &route.users)
        && route.ports.contains(&target.port)
        && name_allowed
        && addresses_allowed
}

/// Returns whether any route lets `user` reach `target` at `addresses`.
pub fn routes_allow(
    config: &Config,
    user: &DirectoryUser,
    target: &RoutedTarget,
    addresses: &[IpAddr],
) -> bool {
    config
        .routes
        .iter()
        .any(|route| route_allows(route, user, target, addresses))
}

/// Returns the configured assertion groups `user` is in, by the names the
/// configuration gives them (Section 13.7).
pub fn reported_groups(config: &Config, user: &DirectoryUser) -> Vec<String> {
    config
        .assertions
        .groups
        .iter()
        .filter(|group| {
            user.groups
                .iter()
                .any(|member_of| group_matches(group, member_of))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a user in `groups`.
    fn user(name: &str, groups: &[&str]) -> DirectoryUser {
        DirectoryUser {
            name: name.to_owned(),
            display_name: String::new(),
            groups: groups.iter().map(|group| (*group).to_owned()).collect(),
        }
    }

    /// A route for `*.desktops.corp.example.com` in 10.20.0.0/16, for QDP Users.
    fn route() -> RouteConfig {
        RouteConfig {
            names: vec!["*.desktops.corp.example.com".to_owned()],
            networks: vec!["10.20.0.0/16".to_owned()],
            ports: vec![4433],
            groups: vec!["QDP Users".to_owned()],
            users: Vec::new(),
        }
    }

    #[test]
    fn reads_targets() {
        let target = |name: &str, port| {
            Some(RoutedTarget {
                name: name.to_owned(),
                port,
            })
        };
        assert_eq!(RoutedTarget::parse("Build-01"), target("build-01", 4433));
        assert_eq!(
            RoutedTarget::parse("build-01.corp.example.com.:5000"),
            target("build-01.corp.example.com", 5000)
        );
        assert_eq!(RoutedTarget::parse("10.0.4.20"), target("10.0.4.20", 4433));
        assert_eq!(
            RoutedTarget::parse("[fd00::1]:4433"),
            target("fd00::1", 4433)
        );
        assert_eq!(RoutedTarget::parse("fd00::1"), target("fd00::1", 4433));
        for bad in [
            "",
            "host:port",
            "a b",
            "[fd00::1",
            "host/../etc",
            "[fd00::1]x",
        ] {
            assert_eq!(RoutedTarget::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn matches_groups_by_name_or_distinguished_name() {
        let dn = "CN=QDP Users,OU=Groups,DC=corp,DC=example,DC=com";
        assert!(group_matches("QDP Users", dn));
        assert!(group_matches("qdp users", dn));
        assert!(group_matches(dn, dn));
        assert!(!group_matches("Groups", dn));
        assert!(!group_matches("QDP", dn));
    }

    #[test]
    fn routes_need_the_name_and_every_address() {
        let alice = user(
            "alice",
            &["CN=QDP Users,OU=Groups,DC=corp,DC=example,DC=com"],
        );
        let bob = user("bob", &["CN=Sales,OU=Groups,DC=corp,DC=example,DC=com"]);
        let target = RoutedTarget::parse("ws-12.desktops.corp.example.com").unwrap();
        let inside: IpAddr = "10.20.3.4".parse().unwrap();
        let outside: IpAddr = "10.30.0.1".parse().unwrap();
        assert!(route_allows(&route(), &alice, &target, &[inside]));
        assert!(!route_allows(&route(), &bob, &target, &[inside]));
        assert!(!route_allows(&route(), &alice, &target, &[inside, outside]));
        assert!(!route_allows(&route(), &alice, &target, &[]));
        let elsewhere = RoutedTarget::parse("ws-12.corp.example.com").unwrap();
        assert!(!route_allows(&route(), &alice, &elsewhere, &[inside]));
        let other_port = RoutedTarget::parse("ws-12.desktops.corp.example.com:22").unwrap();
        assert!(!route_allows(&route(), &alice, &other_port, &[inside]));
        // The domain itself is not in a `*.` pattern, nor are look-alikes.
        assert!(!name_matches(
            "*.desktops.corp.example.com",
            "desktops.corp.example.com"
        ));
        assert!(!name_matches(
            "*.desktops.corp.example.com",
            "evildesktops.corp.example.com"
        ));
        assert!(name_matches(
            "*.desktops.corp.example.com",
            "a.b.desktops.corp.example.com"
        ));
        // A route of networks only takes any name that resolves into them.
        let networks_only = RouteConfig {
            names: Vec::new(),
            ..route()
        };
        assert!(route_allows(&networks_only, &alice, &elsewhere, &[inside]));
    }
}
