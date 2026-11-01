//! The gateway as a service of the system's own kind: a systemd unit on
//! Linux ([`systemd`]), a Windows service on Windows ([`windows`]). macOS has
//! none: the gateway serves an organization's network from a server.

#[cfg(target_os = "linux")]
pub mod systemd;
#[cfg(windows)]
pub mod windows;

/// Whether an installed service starts at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// Now, and at every boot.
    Now,
    /// At the next boot, or when started by hand, as while its configuration
    /// is still to be written.
    Later,
}

use std::path::Path;

#[cfg(any(target_os = "linux", windows))]
use crate::config::Config;
#[cfg(target_os = "linux")]
use crate::locations;

/// The example configuration that installing writes where there is none.
#[cfg(any(target_os = "linux", windows))]
const EXAMPLE_CONFIG: &str = include_str!("../../../../packaging/gateway.toml");

/// Returns the options the service passes to say which configuration to
/// read: none for the default one.
#[cfg(target_os = "linux")]
fn config_options(config: &Path) -> Vec<String> {
    if config == locations::default_config() {
        Vec::new()
    } else {
        vec!["--config".to_owned(), config.display().to_string()]
    }
}

/// Checks the configuration in `config` if there is one, to start the
/// service with it, or says that one is to be written.
#[cfg(any(target_os = "linux", windows))]
fn check_config(config: &Path) -> Result<Option<Config>, String> {
    if config.exists() {
        Config::load(config)
            .map(Some)
            .map_err(|error| format!("{error}; fix it, then install again"))
    } else {
        Ok(None)
    }
}

/// The gateway's systemd service on Linux.
#[cfg(target_os = "linux")]
mod platform {
    use std::fs;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    use std::path::Path;
    use std::process::Command;

    use super::systemd::{SystemdService, ensure_system_user};
    use super::{EXAMPLE_CONFIG, Start, check_config, config_options};

    /// The gateway's service.
    pub(super) const SERVICE: SystemdService = SystemdService {
        unit: "qdp-gateway.service",
        packaged_unit: include_str!("../../../../packaging/linux/qdp-gateway.service"),
        packaged_program: "/usr/bin/qdp-gateway",
        program: "qdp-gateway",
    };

    /// The user and group the service runs as.
    const USER: &str = "qdp-gateway";

    /// The user's description.
    const USER_DESCRIPTION: &str = "QDP gateway";

    /// The service's state directory, the user's home.
    const STATE_DIRECTORY: &str = "/var/lib/qdp-gateway";

    /// The configuration directory's permissions: the gateway's group reads
    /// it, no one else.
    const CONFIG_DIRECTORY_MODE: u32 = 0o750;

    /// The example configuration's permissions.
    const CONFIG_MODE: u32 = 0o640;

    /// Installs the service, and returns what to tell the administrator.
    pub(super) fn install(config: &Path) -> Result<String, String> {
        ensure_system_user(USER, USER_DESCRIPTION, STATE_DIRECTORY)?;
        let start = match check_config(config)? {
            Some(_) => Start::Now,
            None => {
                write_example(config)?;
                Start::Later
            }
        };
        SERVICE.install(&config_options(config), start)?;
        Ok(match start {
            Start::Now => format!(
                "The QDP gateway runs as a service now and at every boot, with {}.\n\
                 Its keys and TOTP secrets are in {STATE_DIRECTORY}; its log is in\n\
                 `journalctl -u qdp-gateway -f`.",
                config.display()
            ),
            Start::Later => format!(
                "The QDP gateway is installed as a service, which starts at boot. Edit {}\n\
                 for your organization, put the directory account's password in the file it\n\
                 names, readable by the group {USER}, check it with `qdp-gateway check`,\n\
                 and start it with `sudo systemctl start qdp-gateway`.",
                config.display()
            ),
        })
    }

    /// Writes the example configuration to `config`, which the gateway's
    /// group may read.
    fn write_example(config: &Path) -> Result<(), String> {
        let failed = |error: std::io::Error| format!("cannot write {}: {error}", config.display());
        if let Some(directory) = config.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(CONFIG_DIRECTORY_MODE)
                .create(directory)
                .map_err(failed)?;
            give_group(directory)?;
        }
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(CONFIG_MODE)
            .open(config)
            .and_then(|mut file| std::io::Write::write_all(&mut file, EXAMPLE_CONFIG.as_bytes()))
            .map_err(failed)?;
        give_group(config)
    }

    /// Gives `path` to the gateway's group.
    fn give_group(path: &Path) -> Result<(), String> {
        let status = Command::new("chgrp")
            .arg(USER)
            .arg(path)
            .status()
            .map_err(|error| format!("cannot run chgrp: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "cannot give {} to the group {USER}",
                path.display()
            ))
        }
    }

    /// Stops and removes the service, and returns what to tell the
    /// administrator.
    pub(super) fn uninstall() -> Result<String, String> {
        let packaged = SERVICE.uninstall()?;
        Ok(format!(
            "The QDP gateway service is {}. Its configuration and keys stay, in\n\
             /etc/qdp-gateway and {STATE_DIRECTORY}.",
            if packaged {
                "stopped and off; the package's unit stays"
            } else {
                "removed"
            }
        ))
    }

    /// Shows the service's state.
    pub(super) fn status() -> Result<(), String> {
        SERVICE
            .status()
            .map_err(|error| format!("{error}: run `sudo qdp-gateway service install`"))
    }
}

/// The gateway's Windows service.
#[cfg(windows)]
mod platform {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;
    use std::sync::mpsc::Receiver;

    use super::windows::{
        Account, WindowsService, allow_inbound_udp, installed_program, remove_firewall_rule,
        restrict_folder, run_as_service,
    };
    use super::{EXAMPLE_CONFIG, Start, check_config};
    use crate::config::Config;
    use crate::locations;
    use crate::server::serve_until;

    /// The gateway's service.
    pub(super) const SERVICE: WindowsService = WindowsService {
        name: "qdp-gateway",
        display_name: "QDP Gateway",
        description: "Signs users in with the organization's directory and a TOTP code, and \
                      tunnels their QDP sessions to the hosts they may reach.",
        program: "qdp-gateway.exe",
        account: Account::LocalService,
    };

    /// The Windows Firewall rule that lets clients reach the gateway.
    const FIREWALL_RULE: &str = "QDP Gateway";

    /// The configuration the running service reads, which its entry point,
    /// called by the system, finds here.
    static CONFIG: OnceLock<PathBuf> = OnceLock::new();

    /// Installs the service, and returns what to tell the administrator.
    pub(super) fn install(config: &Path) -> Result<String, String> {
        let folder = locations::data_folder();
        restrict_folder(&folder, SERVICE.account)?;
        let checked = check_config(config)?;
        let start = match &checked {
            Some(checked) => {
                let program = installed_program(&SERVICE)?;
                allow_inbound_udp(FIREWALL_RULE, &program, checked.listen.port())?;
                Start::Now
            }
            None => {
                std::fs::write(config, EXAMPLE_CONFIG)
                    .map_err(|error| format!("cannot write {}: {error}", config.display()))?;
                Start::Later
            }
        };
        let arguments = ["service", "run", "--config"]
            .into_iter()
            .map(OsString::from)
            .chain([config.as_os_str().to_owned()])
            .collect();
        SERVICE.install(arguments, start)?;
        Ok(match start {
            Start::Now => format!(
                "The QDP gateway runs as a service now and at every boot, with {}.\n\
                 Its keys, TOTP secrets and log are in {}.",
                config.display(),
                locations::data_folder().display()
            ),
            Start::Later => format!(
                "The QDP gateway is installed as a service, which starts at boot. Edit {}\n\
                 for your organization, put the directory account's password in the file it\n\
                 names, check it with `qdp-gateway check`, then install again to open the\n\
                 firewall and start it.",
                config.display()
            ),
        })
    }

    /// Stops and removes the service, and returns what to tell the
    /// administrator.
    pub(super) fn uninstall() -> Result<String, String> {
        SERVICE.uninstall()?;
        remove_firewall_rule(FIREWALL_RULE);
        Ok(format!(
            "The QDP gateway service is removed. Its configuration and keys stay, in {}.",
            locations::data_folder().display()
        ))
    }

    /// Shows the service's state.
    pub(super) fn status() -> Result<(), String> {
        println!("The QDP gateway service is {}.", SERVICE.status()?);
        Ok(())
    }

    /// Runs the gateway as the service, with the configuration in `config`.
    pub(super) fn run(config: PathBuf) -> Result<(), String> {
        CONFIG
            .set(config)
            .map_err(|_| "the service runs already".to_owned())?;
        run_as_service(SERVICE.name, serve_service)
    }

    /// Serves users until the system stops the service.
    fn serve_service(stop: Receiver<()>) -> Result<(), String> {
        let path = CONFIG.get().ok_or("the service has no configuration")?;
        let config = Config::load(path).map_err(|error| error.to_string())?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the runtime: {error}"))?;
        runtime.block_on(serve_until(config, async move {
            // The system asks the service to stop through the channel.
            let _ = tokio::task::spawn_blocking(move || stop.recv()).await;
        }))
    }
}

/// No service elsewhere: the gateway serves an organization from a Linux or
/// Windows server.
#[cfg(not(any(target_os = "linux", windows)))]
mod platform {
    use std::path::Path;

    /// The answer to every service command here.
    const UNSUPPORTED: &str =
        "qdp-gateway runs as a service on Linux and Windows; here, run `qdp-gateway serve`";

    /// Fails: there is no service here.
    pub(super) fn install(_: &Path) -> Result<String, String> {
        Err(UNSUPPORTED.to_owned())
    }

    /// Fails: there is no service here.
    pub(super) fn uninstall() -> Result<String, String> {
        Err(UNSUPPORTED.to_owned())
    }

    /// Fails: there is no service here.
    pub(super) fn status() -> Result<(), String> {
        Err(UNSUPPORTED.to_owned())
    }
}

/// Installs the gateway's service, reading `config`, writing an example there
/// if there is none, and returns what to tell the administrator.
pub fn install(config: &Path) -> Result<String, String> {
    platform::install(config)
}

/// Stops and removes the gateway's service, and returns what to tell the
/// administrator.
pub fn uninstall() -> Result<String, String> {
    platform::uninstall()
}

/// Shows the gateway's service's state.
pub fn status() -> Result<(), String> {
    platform::status()
}

/// Runs the gateway as its Windows service, with the configuration in
/// `config`, as the Service Control Manager starts it.
#[cfg(windows)]
pub fn run(config: std::path::PathBuf) -> Result<(), String> {
    platform::run(config)
}
