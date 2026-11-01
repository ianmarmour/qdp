//! The host as a service of the system's own kind:
//!
//! - Linux: a systemd unit that runs `qdp serve --source console` as root
//!   from boot, without anyone logged in, streaming whatever the graphics
//!   card shows, the login screen included, so that devices can log in and
//!   unlock as at the machine.
//! - macOS: a LaunchAgent that runs `qdp serve` in the user's session from
//!   login, where macOS lets it capture the screen once allowed to.

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

/// The service on macOS: a LaunchAgent in the user's session.
#[cfg(target_os = "macos")]
mod platform {
    use std::fs;
    use std::io::ErrorKind;
    use std::path::PathBuf;
    use std::process::Command;

    use super::launch_agent::{AgentPaths, LABEL, agent_plist};
    use crate::args::ServiceAction;

    /// The launchd command.
    const LAUNCHCTL: &str = "launchctl";

    /// Where a user's agents are, under their home.
    const AGENTS_DIRECTORY: &str = "Library/LaunchAgents";

    /// Where the agent keeps the host's key and paired devices, under the
    /// home.
    const STATE_DIRECTORY: &str = "Library/Application Support/qdp";

    /// The agent's log, under the home.
    const LOG_FILE: &str = "Library/Logs/qdp.log";

    /// Carries out `qdp service`.
    pub(super) fn run(action: ServiceAction) -> Result<(), String> {
        match action {
            ServiceAction::Install { options } => install(&options),
            ServiceAction::Uninstall => uninstall(),
            ServiceAction::Status => status(),
        }
    }

    /// Writes the agent, running this command with `options`, and starts it
    /// now and at every login.
    fn install(options: &[String]) -> Result<(), String> {
        let home = home()?;
        let paths = AgentPaths {
            command: std::env::current_exe()
                .and_then(|path| path.canonicalize())
                .map_err(|error| format!("cannot find this command: {error}"))?,
            state: home.join(STATE_DIRECTORY),
            log: home.join(LOG_FILE),
        };
        for directory in [Some(paths.state.as_path()), paths.log.parent()]
            .into_iter()
            .flatten()
        {
            fs::create_dir_all(directory)
                .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        }
        let plist = agent_file(&home);
        if let Some(directory) = plist.parent() {
            fs::create_dir_all(directory)
                .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        }
        fs::write(&plist, agent_plist(&paths, options))
            .map_err(|error| format!("cannot write {}: {error}", plist.display()))?;
        let domain = gui_domain()?;
        // An agent installed before is replaced.
        let _ = launchctl(&["bootout", &format!("{domain}/{LABEL}")]);
        launchctl(&["bootstrap", &domain, &plist.to_string_lossy()])?;
        println!(
            "The QDP host runs as a LaunchAgent now and at every login. macOS asks to allow\n\
             Screen Recording and Accessibility for qdp the first time a device connects;\n\
             allow both under System Settings, Privacy & Security. Its key and paired devices\n\
             are in {}, and its log in {}.",
            paths.state.display(),
            paths.log.display()
        );
        Ok(())
    }

    /// Stops the agent and removes it. The host's key stays.
    fn uninstall() -> Result<(), String> {
        let home = home()?;
        let domain = gui_domain()?;
        // An agent that is not running has nothing to stop.
        let _ = launchctl(&["bootout", &format!("{domain}/{LABEL}")]);
        let plist = agent_file(&home);
        match fs::remove_file(&plist) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot remove {}: {error}", plist.display())),
        }
        println!(
            "The agent is removed. The host's key and paired devices stay in {}.",
            home.join(STATE_DIRECTORY).display()
        );
        Ok(())
    }

    /// Shows the agent's state as launchd describes it.
    fn status() -> Result<(), String> {
        let domain = gui_domain()?;
        let status = Command::new(LAUNCHCTL)
            .args(["print", &format!("{domain}/{LABEL}")])
            .status()
            .map_err(|error| format!("cannot run {LAUNCHCTL}: {error}"))?;
        if !status.success() {
            return Err("the agent is not installed: run `qdp service install`".to_owned());
        }
        Ok(())
    }

    /// Returns the user's home.
    fn home() -> Result<PathBuf, String> {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_owned())
    }

    /// Returns where the agent's file is.
    fn agent_file(home: &std::path::Path) -> PathBuf {
        home.join(AGENTS_DIRECTORY).join(format!("{LABEL}.plist"))
    }

    /// Returns launchd's domain of the user's login session.
    fn gui_domain() -> Result<String, String> {
        let output = Command::new("id")
            .arg("-u")
            .output()
            .map_err(|error| format!("cannot find the user's ID: {error}"))?;
        let user = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !output.status.success() || user.is_empty() {
            return Err("cannot find the user's ID".to_owned());
        }
        Ok(format!("gui/{user}"))
    }

    /// Runs `launchctl` with `arguments`, failing with what it said.
    fn launchctl(arguments: &[&str]) -> Result<(), String> {
        let output = Command::new(LAUNCHCTL)
            .args(arguments)
            .output()
            .map_err(|error| format!("cannot run {LAUNCHCTL}: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{LAUNCHCTL} {} failed: {}",
                arguments.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

/// The LaunchAgent's property list, which macOS's agent is made of; apart
/// from the platform so that its tests run everywhere.
#[cfg(any(target_os = "macos", test))]
mod launch_agent {
    use std::path::{Path, PathBuf};

    /// The agent's label, which names it to launchd.
    pub(super) const LABEL: &str = "io.github.ianmarmour.qdp";

    /// The start of every property list.
    const PLIST_HEADER: &str = concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?>"#,
        "\n",
        r#"<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">"#,
        "\n",
    );

    /// Where the agent's files are.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct AgentPaths {
        /// The qdp command it runs.
        pub command: PathBuf,
        /// Where it keeps the host's key and paired devices.
        pub state: PathBuf,
        /// Its log.
        pub log: PathBuf,
    }

    /// Returns the agent's property list: run `qdp serve` with `options`
    /// after its own at login, in the user's graphical session, again if it
    /// fails, and unthrottled, as the screen's user is.
    pub(super) fn agent_plist(paths: &AgentPaths, options: &[String]) -> String {
        let path = |path: &Path| path.to_string_lossy().into_owned();
        let mut arguments = vec![
            path(&paths.command),
            "serve".to_owned(),
            "--identity".to_owned(),
            path(&paths.state),
        ];
        arguments.extend(options.iter().cloned());
        let arguments: String = arguments
            .iter()
            .map(|argument| format!("\t\t<string>{}</string>\n", escape(argument)))
            .collect();
        let log = escape(&path(&paths.log));
        format!(
            "{PLIST_HEADER}<plist version=\"1.0\">\n<dict>\n\
             \t<key>Label</key>\n\t<string>{LABEL}</string>\n\
             \t<key>ProgramArguments</key>\n\t<array>\n{arguments}\t</array>\n\
             \t<key>RunAtLoad</key>\n\t<true/>\n\
             \t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n\
             \t<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>\n\
             \t<key>ProcessType</key>\n\t<string>Interactive</string>\n\
             \t<key>StandardOutPath</key>\n\t<string>{log}</string>\n\
             \t<key>StandardErrorPath</key>\n\t<string>{log}</string>\n\
             </dict>\n</plist>\n"
        )
    }

    /// Escapes text for a property list's XML.
    fn escape(text: &str) -> String {
        let mut escaped = String::with_capacity(text.len());
        for c in text.chars() {
            match c {
                '&' => escaped.push_str("&amp;"),
                '<' => escaped.push_str("&lt;"),
                '>' => escaped.push_str("&gt;"),
                '"' => escaped.push_str("&quot;"),
                '\'' => escaped.push_str("&apos;"),
                _ => escaped.push(c),
            }
        }
        escaped
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The agent runs qdp serve with its key's directory and the options
        /// given, escaped, in the user's session, logging to its file.
        #[test]
        fn writes_the_agent() {
            let paths = AgentPaths {
                command: PathBuf::from("/Users/ian/qdp/qdp"),
                state: PathBuf::from("/Users/ian/Library/Application Support/qdp"),
                log: PathBuf::from("/Users/ian/Library/Logs/qdp.log"),
            };
            let options = ["--token".to_owned(), "a<b&c".to_owned()];
            let plist = agent_plist(&paths, &options);
            assert!(
                plist.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist")
            );
            assert!(plist.contains(
                "\t\t<string>/Users/ian/qdp/qdp</string>\n\t\t<string>serve</string>\n\
                 \t\t<string>--identity</string>\n\
                 \t\t<string>/Users/ian/Library/Application Support/qdp</string>\n\
                 \t\t<string>--token</string>\n\t\t<string>a&lt;b&amp;c</string>\n"
            ));
            assert!(plist.contains("<string>io.github.ianmarmour.qdp</string>"));
            assert!(plist.contains("<key>LimitLoadToSessionType</key>\n\t<string>Aqua</string>"));
            assert!(plist.contains("<key>ProcessType</key>\n\t<string>Interactive</string>"));
            assert!(plist.contains("<string>/Users/ian/Library/Logs/qdp.log</string>"));
            assert!(plist.ends_with("</dict>\n</plist>\n"));
        }

        /// XML's special characters are escaped.
        #[test]
        fn escapes_xml() {
            assert_eq!(escape("a&b<c>d\"e'f"), "a&amp;b&lt;c&gt;d&quot;e&apos;f");
        }
    }
}

/// The service elsewhere, which only Linux and macOS have so far.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use crate::args::ServiceAction;

    /// Carries out `qdp service`; this system cannot yet.
    pub(super) fn run(_action: ServiceAction) -> Result<(), String> {
        Err("qdp service is only available on Linux and macOS so far".to_owned())
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
