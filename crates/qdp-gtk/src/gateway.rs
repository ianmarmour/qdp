//! The organization's gateway the app may reach computers through
//! (Section 13 of the specification), as the user set it in the
//! preferences, and the computers they added to reach through it. `qdp
//! connect --gateway` signs in, asking the app for the user's password and
//! a code from their authenticator app, and keeps the session that lets it
//! sign in again without asking.
//!
//! The gateway is kept in the user's configuration, a `key=value` line each
//! of its address, its key pin, if given, and the user's name; the
//! computers in the app's directory, a line each with what the gateway
//! knows the computer as, a tab and its name for users.

use std::fs;
use std::io;
use std::path::Path;

use crate::key_pin::KeyPin;

/// A gateway the user set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gateway {
    /// Its address: a name or IP address, with an optional port.
    pub address: String,
    /// Its key pin, for a gateway whose certificate the system's
    /// certificate authorities do not vouch for.
    pub pin: Option<KeyPin>,
    /// The user's name in the organization's directory, if set.
    pub user: Option<String>,
}

impl Gateway {
    /// Reads the gateway in `path`, or `None` if there is none.
    pub fn load(path: &Path) -> Option<Self> {
        let text = fs::read_to_string(path).ok()?;
        let mut address = None;
        let mut pin = None;
        let mut user = None;
        for line in text.lines() {
            match line.split_once('=') {
                Some(("address", value)) if !value.trim().is_empty() => {
                    address = Some(value.trim().to_owned());
                }
                Some(("pin", value)) => pin = KeyPin::parse(value.trim()),
                Some(("user", value)) if !value.trim().is_empty() => {
                    user = Some(value.trim().to_owned());
                }
                _ => {}
            }
        }
        Some(Self {
            address: address?,
            pin,
            user,
        })
    }

    /// Writes the gateway to `path`.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        let mut text = format!("address={}\n", self.address);
        if let Some(pin) = &self.pin {
            text.push_str(&format!("pin={pin}\n"));
        }
        if let Some(user) = &self.user {
            text.push_str(&format!("user={user}\n"));
        }
        fs::write(path, text)
    }

    /// Forgets the gateway in `path`.
    pub fn remove(path: &Path) -> io::Result<()> {
        match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Returns the options of `qdp connect` that reach a computer through
    /// the gateway.
    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = vec!["--gateway".to_owned(), self.address.clone()];
        if let Some(pin) = &self.pin {
            arguments.extend(["--gateway-pin".to_owned(), pin.as_str().to_owned()]);
        }
        if let Some(user) = &self.user {
            arguments.extend(["--user".to_owned(), user.clone()]);
        }
        arguments
    }
}

/// A computer reached through the gateway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayHost {
    /// What the gateway knows it as: its ID there, or its name or address
    /// inside the organization's network.
    pub target: String,
    /// Its name for users.
    pub name: String,
}

/// Returns the computers reached through the gateway kept in `path`, most
/// recently added first; none if there is no file.
pub fn hosts(path: &Path) -> Vec<GatewayHost> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut hosts: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let (target, name) = line.split_once('\t')?;
            (!target.is_empty() && !name.is_empty()).then(|| GatewayHost {
                target: target.to_owned(),
                name: name.to_owned(),
            })
        })
        .collect();
    hosts.reverse();
    hosts
}

/// Keeps `host` in `path`, in place of one with its target.
pub fn add_host(path: &Path, host: &GatewayHost) -> io::Result<()> {
    let mut hosts = hosts(path);
    hosts.retain(|kept| kept.target != host.target);
    hosts.insert(0, host.clone());
    save_hosts(path, &hosts)
}

/// Forgets the computer with `target` in `path`.
pub fn remove_host(path: &Path, target: &str) -> io::Result<()> {
    let mut hosts = hosts(path);
    hosts.retain(|kept| kept.target != target);
    save_hosts(path, &hosts)
}

/// Writes `hosts`, most recent first as given, to `path`.
fn save_hosts(path: &Path, hosts: &[GatewayHost]) -> io::Result<()> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory)?;
    }
    // The file keeps them oldest first, as the other files of hosts do.
    let text: String = hosts
        .iter()
        .rev()
        .map(|host| {
            let clean = |text: &str| text.replace(['\t', '\n', '\r'], " ");
            format!("{}\t{}\n", clean(&host.target), clean(&host.name))
        })
        .collect();
    fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_gateway_and_its_options() {
        let directory =
            std::env::temp_dir().join(format!("qdp-gtk-gateway-{}", std::process::id()));
        let path = directory.join("gateway");
        assert_eq!(Gateway::load(&path), None);
        let pin = KeyPin::parse(&"ab".repeat(32)).unwrap();
        let gateway = Gateway {
            address: "gw.corp.example.com".to_owned(),
            pin: Some(pin.clone()),
            user: Some("alice".to_owned()),
        };
        gateway.save(&path).unwrap();
        assert_eq!(Gateway::load(&path), Some(gateway.clone()));
        assert_eq!(
            gateway.arguments(),
            [
                "--gateway",
                "gw.corp.example.com",
                "--gateway-pin",
                pin.as_str(),
                "--user",
                "alice"
            ]
        );
        let bare = Gateway {
            pin: None,
            user: None,
            ..gateway
        };
        assert_eq!(bare.arguments(), ["--gateway", "gw.corp.example.com"]);
        Gateway::remove(&path).unwrap();
        Gateway::remove(&path).unwrap();
        assert_eq!(Gateway::load(&path), None);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn keeps_the_computers_reached_through_it() {
        let directory =
            std::env::temp_dir().join(format!("qdp-gtk-gateway-hosts-{}", std::process::id()));
        let path = directory.join("gateway-hosts");
        assert_eq!(hosts(&path), []);
        let host = |target: &str, name: &str| GatewayHost {
            target: target.to_owned(),
            name: name.to_owned(),
        };
        add_host(&path, &host("build-01", "Build machine")).unwrap();
        add_host(&path, &host("desk-12.corp.example.com", "Desk")).unwrap();
        add_host(&path, &host("build-01", "Builder")).unwrap();
        assert_eq!(
            hosts(&path),
            [
                host("build-01", "Builder"),
                host("desk-12.corp.example.com", "Desk")
            ]
        );
        remove_host(&path, "build-01").unwrap();
        assert_eq!(hosts(&path), [host("desk-12.corp.example.com", "Desk")]);
        let _ = fs::remove_dir_all(directory);
    }
}
