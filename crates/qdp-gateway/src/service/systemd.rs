//! systemd units on Linux. A program's package installs its unit, which
//! `install` copies to `/etc/systemd/system`, where it takes the place of the
//! package's, with the command line pointed at the program that runs it and
//! given options of its own.

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::Start;

/// The start of a unit's command line.
const EXEC_START: &str = "ExecStart=";

/// Where units taking the place of packages' are written.
const UNIT_DIRECTORY: &str = "/etc/systemd/system";

/// Where packages install their units.
const PACKAGE_UNIT_DIRECTORY: &str = "/usr/lib/systemd/system";

/// Where a program running from elsewhere, such as a build in a home
/// directory, is installed for its service: SELinux, on Fedora among others,
/// keeps services from running programs in home directories.
const INSTALL_DIRECTORY: &str = "/usr/local/bin";

/// The directories of programs that services may run as they are.
const SYSTEM_PROGRAM_DIRECTORIES: [&str; 2] = ["/usr/", "/opt/"];

/// An installed program's permissions.
const PROGRAM_MODE: u32 = 0o755;

/// A written unit's permissions: its options may hold a secret, so only root
/// reads it.
const UNIT_MODE: u32 = 0o600;

/// The systemd command.
const SYSTEMCTL: &str = "systemctl";

/// The exit status of `systemctl status` for a unit it does not know.
const UNKNOWN_UNIT: i32 = 4;

/// The message of a step that needs root.
const NEEDS_ROOT: &str = "this needs root: run it with sudo";

/// A program's systemd service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemdService {
    /// The unit's name, such as `qdp.service`.
    pub unit: &'static str,
    /// The package's unit, whose command line `install` points at the
    /// program.
    pub packaged_unit: &'static str,
    /// The program the package's unit runs, such as `/usr/bin/qdp`.
    pub packaged_program: &'static str,
    /// The program's file name, such as `qdp`.
    pub program: &'static str,
}

impl SystemdService {
    /// Writes the unit, running the program with `options` after the
    /// package's own, and turns it on, starting it as `start` says.
    pub fn install(&self, options: &[String], start: Start) -> Result<(), String> {
        let program = self.service_program()?;
        let unit = self.unit_text(&program.to_string_lossy(), options)?;
        let path = Path::new(UNIT_DIRECTORY).join(self.unit);
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(UNIT_MODE)
            .open(&path)
            .map_err(|error| failure(&path, &error))?;
        file.write_all(unit.as_bytes())
            .map_err(|error| failure(&path, &error))?;
        systemctl(&["daemon-reload"])?;
        match start {
            Start::Now => systemctl(&["enable", "--now", self.unit]),
            Start::Later => systemctl(&["enable", self.unit]),
        }
    }

    /// Stops the service and removes the unit `install` wrote, leaving the
    /// package's, if any, off. Returns whether the package's unit stays.
    pub fn uninstall(&self) -> Result<bool, String> {
        // A service that is not installed has nothing to stop.
        let _ = systemctl(&["disable", "--now", self.unit]);
        let path = Path::new(UNIT_DIRECTORY).join(self.unit);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(failure(&path, &error)),
        }
        systemctl(&["daemon-reload"])?;
        Ok(Path::new(PACKAGE_UNIT_DIRECTORY).join(self.unit).exists())
    }

    /// Shows the service's state and latest log. Fails if it is not
    /// installed.
    pub fn status(&self) -> Result<(), String> {
        let status = Command::new(SYSTEMCTL)
            .args(["status", "--no-pager", self.unit])
            .status()
            .map_err(|error| format!("cannot run {SYSTEMCTL}: {error}"))?;
        // `systemctl status` fails for a service that is stopped, which is an
        // answer, not an error; one that is unknown is.
        if status.code() == Some(UNKNOWN_UNIT) {
            return Err(format!("{} is not installed", self.unit));
        }
        Ok(())
    }

    /// Returns the program the service runs: this one if it is among the
    /// system's programs, as a package's is, and otherwise a copy of it
    /// installed there, replacing the copy an earlier install made.
    fn service_program(&self) -> Result<PathBuf, String> {
        let current = std::env::current_exe()
            .and_then(|path| path.canonicalize())
            .map_err(|error| format!("cannot find this program: {error}"))?;
        let path = current.to_string_lossy();
        if SYSTEM_PROGRAM_DIRECTORIES
            .iter()
            .any(|directory| path.starts_with(directory))
        {
            return Ok(current);
        }
        let installed = Path::new(INSTALL_DIRECTORY).join(self.program);
        let installing = Path::new(INSTALL_DIRECTORY).join(format!(".{}.installing", self.program));
        // A new file takes the directory's security label, which a moved one
        // would not; the rename replaces the old copy at once, so that a
        // running service keeps its own until it restarts.
        fs::copy(&current, &installing).map_err(|error| failure(&installing, &error))?;
        fs::set_permissions(&installing, fs::Permissions::from_mode(PROGRAM_MODE))
            .map_err(|error| failure(&installing, &error))?;
        fs::rename(&installing, &installed).map_err(|error| failure(&installed, &error))?;
        println!(
            "Installed this program as {}, which the service runs.",
            installed.display()
        );
        Ok(installed)
    }

    /// Returns the package's unit with its command line running `program`
    /// and passing `options` after its own arguments.
    pub fn unit_text(&self, program: &str, options: &[String]) -> Result<String, String> {
        let mut found = false;
        let lines: Vec<String> = self
            .packaged_unit
            .lines()
            .map(|line| match line.strip_prefix(EXEC_START) {
                Some(arguments) => {
                    found = true;
                    let arguments = arguments
                        .strip_prefix(self.packaged_program)
                        .unwrap_or(arguments);
                    let mut line = format!("{EXEC_START}{}{arguments}", quote(program));
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
            return Err(format!("{} has no command line", self.unit));
        }
        Ok(lines.join("\n") + "\n")
    }
}

/// Makes the system user `name`, with a group of the same name and `home` as
/// its home, unless it exists, with `systemd-sysusers` as packages do.
pub fn ensure_system_user(name: &str, description: &str, home: &str) -> Result<(), String> {
    let line = format!("u {name} - \"{description}\" {home}");
    run("systemd-sysusers", &["--inline", &line])
}

/// Quotes `argument` for a unit's command line, where `%` begins a specifier
/// and `$` a variable unless doubled, and spaces, quotes and backslashes need
/// quoting.
pub fn quote(argument: &str) -> String {
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

/// Runs `systemctl` with `arguments`, failing with what it said.
pub fn systemctl(arguments: &[&str]) -> Result<(), String> {
    run(SYSTEMCTL, arguments)
}

/// Runs `program` with `arguments`, failing with what it said.
fn run(program: &str, arguments: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{program} {} failed: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Returns the message of a failure to write `path`.
fn failure(path: &Path, error: &std::io::Error) -> String {
    match error.kind() {
        ErrorKind::PermissionDenied => NEEDS_ROOT.to_owned(),
        _ => format!("cannot write {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit as packages install them.
    const UNIT: &str = "[Service]\nExecStart=/usr/bin/qdp-gateway serve --config /etc/qdp-gateway/gateway.toml\nRestart=on-failure\n";

    /// The service of [`UNIT`].
    const SERVICE: SystemdService = SystemdService {
        unit: "qdp-gateway.service",
        packaged_unit: UNIT,
        packaged_program: "/usr/bin/qdp-gateway",
        program: "qdp-gateway",
    };

    #[test]
    fn points_the_unit_at_the_program_with_options() {
        let options = ["--config".to_owned(), "/srv/a b$%.toml".to_owned()];
        let unit = SERVICE
            .unit_text("/usr/local/bin/qdp-gateway", &options)
            .unwrap();
        assert_eq!(
            unit,
            "[Service]\nExecStart=/usr/local/bin/qdp-gateway serve --config /etc/qdp-gateway/gateway.toml --config \"/srv/a b$$%%.toml\"\nRestart=on-failure\n"
        );
        let without = SystemdService {
            packaged_unit: "[Service]\n",
            ..SERVICE
        };
        assert!(without.unit_text("/usr/bin/qdp-gateway", &[]).is_err());
    }

    #[test]
    fn quotes_arguments() {
        assert_eq!(quote("plain-1.0"), "plain-1.0");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote("with space"), "\"with space\"");
        assert_eq!(quote("50%"), "50%%");
        assert_eq!(quote("$HOME"), "$$HOME");
        assert_eq!(quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
