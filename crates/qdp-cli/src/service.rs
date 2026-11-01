//! The host as a system service, which serves from boot without anyone
//! logged in: on Linux a systemd unit that runs `qdp serve --source console`
//! as root, streaming whatever the graphics card shows, the login screen
//! included, so that devices can log in and unlock as at the machine.

use crate::args::ServiceAction;

/// Carries out `qdp service`.
pub fn run(action: ServiceAction) -> Result<(), String> {
    platform::run(action)
}

/// The service on Linux: a systemd unit, the package's own, which
/// `qdp service install` copies to point at this command and its options.
#[cfg(target_os = "linux")]
mod platform {
    use std::fs::{self, OpenOptions};
    use std::io::{ErrorKind, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::process::Command;

    use crate::args::ServiceAction;

    /// The unit the package installs, whose command line this copies.
    const PACKAGED_UNIT: &str = include_str!("../../../packaging/linux/qdp.service");

    /// The command the package's unit runs.
    const PACKAGED_COMMAND: &str = "/usr/bin/qdp";

    /// The start of the unit's command line.
    const EXEC_START: &str = "ExecStart=";

    /// The unit's name.
    const UNIT: &str = "qdp.service";

    /// Where `qdp service install` writes the unit; units here take the
    /// place of the package's.
    const UNIT_PATH: &str = "/etc/systemd/system/qdp.service";

    /// Where the package installs its unit.
    const PACKAGE_UNIT_PATH: &str = "/usr/lib/systemd/system/qdp.service";

    /// Where the service keeps the host's key and paired devices.
    const STATE_DIRECTORY: &str = "/var/lib/qdp";

    /// The unit's permissions: its options may hold a token, so only root
    /// reads it.
    const UNIT_MODE: u32 = 0o600;

    /// The systemd command.
    const SYSTEMCTL: &str = "systemctl";

    /// Carries out `qdp service`.
    pub(super) fn run(action: ServiceAction) -> Result<(), String> {
        match action {
            ServiceAction::Install { options } => install(&options),
            ServiceAction::Uninstall => uninstall(),
            ServiceAction::Status => status(),
        }
    }

    /// Writes the unit, running this command with `options`, and starts the
    /// service now and at every boot.
    fn install(options: &[String]) -> Result<(), String> {
        let command = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .map_err(|error| format!("cannot find this command: {error}"))?;
        let unit = unit(&command.to_string_lossy(), options)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(UNIT_MODE)
            .open(UNIT_PATH)
            .map_err(|error| match error.kind() {
                ErrorKind::PermissionDenied => {
                    "installing the service needs root: run it with sudo".to_owned()
                }
                _ => format!("cannot write {UNIT_PATH}: {error}"),
            })?;
        file.write_all(unit.as_bytes())
            .map_err(|error| format!("cannot write {UNIT_PATH}: {error}"))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", UNIT])?;
        println!(
            "The QDP host runs as a service now and at every boot, streaming the console.\n\
             Its key and paired devices are in {STATE_DIRECTORY}: list them with\n\
             `sudo qdp clients --identity {STATE_DIRECTORY}`. Pairing codes show in\n\
             `journalctl -u qdp -f`."
        );
        Ok(())
    }

    /// Stops the service and removes the unit written by `install`. The
    /// host's key stays, for a later install.
    fn uninstall() -> Result<(), String> {
        // A service that is not installed has nothing to stop.
        let _ = systemctl(&["disable", "--now", UNIT]);
        match fs::remove_file(UNIT_PATH) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) if error.kind() == ErrorKind::PermissionDenied => {
                return Err("removing the service needs root: run it with sudo".to_owned());
            }
            Err(error) => return Err(format!("cannot remove {UNIT_PATH}: {error}")),
        }
        systemctl(&["daemon-reload"])?;
        if Path::new(PACKAGE_UNIT_PATH).exists() {
            println!("The service is stopped and off; the package's unit stays, off.");
        } else {
            println!("The service is removed.");
        }
        println!("The host's key and paired devices stay in {STATE_DIRECTORY}.");
        Ok(())
    }

    /// Shows the service's state and its latest log.
    fn status() -> Result<(), String> {
        let status = Command::new(SYSTEMCTL)
            .args(["status", "--no-pager", UNIT])
            .status()
            .map_err(|error| format!("cannot run {SYSTEMCTL}: {error}"))?;
        // `systemctl status` fails for a service that is stopped, which is
        // an answer, not an error; one that is unknown is.
        const UNKNOWN_UNIT: i32 = 4;
        if status.code() == Some(UNKNOWN_UNIT) {
            return Err("the service is not installed: run `sudo qdp service install`".to_owned());
        }
        Ok(())
    }

    /// Runs `systemctl` with `arguments`, failing with what it said.
    fn systemctl(arguments: &[&str]) -> Result<(), String> {
        let output = Command::new(SYSTEMCTL)
            .args(arguments)
            .output()
            .map_err(|error| format!("cannot run {SYSTEMCTL}: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{SYSTEMCTL} {} failed: {}",
                arguments.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Returns the package's unit with its command line running `command`
    /// and passing `options` on to `qdp serve` after its own.
    pub(super) fn unit(command: &str, options: &[String]) -> Result<String, String> {
        let mut found = false;
        let lines: Vec<String> = PACKAGED_UNIT
            .lines()
            .map(|line| match line.strip_prefix(EXEC_START) {
                Some(arguments) => {
                    found = true;
                    let arguments = arguments
                        .strip_prefix(PACKAGED_COMMAND)
                        .unwrap_or(arguments);
                    let mut line = format!("{EXEC_START}{}{arguments}", quote(command));
                    for option in options {
                        line.push(' ');
                        line.push_str(&quote(option));
                    }
                    line
                }
                None => line.to_owned(),
            })
            .collect();
        if !found {
            return Err("the service's unit has no command line".to_owned());
        }
        Ok(lines.join("\n") + "\n")
    }

    /// Quotes `argument` for a unit's command line, where `%` begins a
    /// specifier and `$` a variable unless doubled, and spaces, quotes and
    /// backslashes need quoting.
    pub(super) fn quote(argument: &str) -> String {
        let escaped = argument.replace('%', "%%").replace('$', "$$");
        let plain = !escaped.is_empty()
            && escaped
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-$".contains(c));
        if plain {
            return escaped;
        }
        let mut quoted = String::with_capacity(escaped.len() + 2);
        quoted.push('"');
        for c in escaped.chars() {
            if c == '"' || c == '\\' {
                quoted.push('\\');
            }
            quoted.push(c);
        }
        quoted.push('"');
        quoted
    }
}

/// The service elsewhere, which only Linux has so far.
#[cfg(not(target_os = "linux"))]
mod platform {
    use crate::args::ServiceAction;

    /// Carries out `qdp service`; this system cannot yet.
    pub(super) fn run(_action: ServiceAction) -> Result<(), String> {
        Err("qdp service is only available on Linux so far".to_owned())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::platform::{quote, unit};

    /// Plain arguments stay as they are; others are quoted, and specifiers
    /// and variables are kept from expanding.
    #[test]
    fn quotes_arguments() {
        assert_eq!(quote("--pairing"), "--pairing");
        assert_eq!(quote("/usr/bin/qdp"), "/usr/bin/qdp");
        assert_eq!(quote("100%"), "100%%");
        assert_eq!(quote("$HOME"), "$$HOME");
        assert_eq!(quote("/opt/my qdp/qdp"), "\"/opt/my qdp/qdp\"");
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(quote(""), "\"\"");
    }

    /// The unit runs this command with the package's arguments, then the
    /// options given.
    #[test]
    fn writes_the_command_line() {
        let options = [
            "--pairing".to_owned(),
            "off".to_owned(),
            "--token".to_owned(),
            "a b".to_owned(),
        ];
        let written = unit("/home/ian/qdp/target/release/qdp", &options).unwrap();
        let line = written
            .lines()
            .find(|line| line.starts_with("ExecStart="))
            .unwrap();
        assert_eq!(
            line,
            "ExecStart=/home/ian/qdp/target/release/qdp serve --source console \
             --identity ${STATE_DIRECTORY} --pairing off --token \"a b\""
        );
        assert!(written.contains("StateDirectory=qdp"));
        assert!(written.ends_with('\n'));
    }
}
