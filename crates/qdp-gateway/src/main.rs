//! `qdp-gateway`: the QDP gateway (Section 13 of the specification), which lets
//! users outside an organization's network reach QDP hosts inside it. It
//! checks each user against the organization's directory, LDAP or Active
//! Directory, then a TOTP code from their authenticator app, and tunnels
//! their QDP sessions, end to end, to the hosts its policy lets them reach.
//!
//! This is its command line; the gateway itself is the library of this crate.

use std::io::IsTerminal;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use qdp::auth::totp::TotpSecret;

use std::sync::Arc;

use qdp::gateway::GatewayErrorCode;
use qdp_gateway::config::Config;
use qdp_gateway::directory::{Directory, DirectoryUser, LdapDirectory};
use qdp_gateway::keys::GatewayKeys;
use qdp_gateway::policy::{self, RoutedTarget};
use qdp_gateway::server::Gateway;
use qdp_gateway::totp_store::TotpStore;

/// Where the configuration is read from by default.
const DEFAULT_CONFIG: &str = "/etc/qdp-gateway/gateway.toml";

/// The environment variable `login` reads the user's password from, as
/// `qdp connect --user` does.
const PASSWORD_VARIABLE: &str = "QDP_PASSWORD";

/// The usage text.
const USAGE: &str = "\
Usage:
  qdp-gateway serve [--config FILE]
                          Serve users: sign them in with the directory and
                          TOTP, and tunnel their sessions to the hosts they
                          may reach
  qdp-gateway keys [--config FILE]
                          Print the pin clients pin the gateway by, and the
                          assertion key hosts trust it by
  qdp-gateway check [--config FILE]
                          Check the configuration, and that the gateway's own
                          directory account can bind
  qdp-gateway login USER [TARGET] [--config FILE]
                          Check USER's password, from QDP_PASSWORD, with the
                          directory, and show who they are, which hosts they
                          may reach and the groups assertions report; with
                          TARGET, a host's ID or its name or address inside
                          the network, whether they may open it
  qdp-gateway totp enroll USER [--config FILE]
                          Make USER a new TOTP secret, replacing any they had,
                          and print the otpauth:// URI that adds it to an
                          authenticator app, such as Google Authenticator,
                          Microsoft Authenticator or Authy (qrencode -t
                          ansiutf8 URI shows it as a QR code)
  qdp-gateway totp remove USER [--config FILE]
                          Forget USER's TOTP secret, so that they enroll again
  qdp-gateway totp list [--config FILE]
                          List the users with a TOTP secret
  qdp-gateway help        Show this help

The configuration is read from /etc/qdp-gateway/gateway.toml unless --config
names another file.";

/// What the command line asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// Serve users.
    Serve,
    /// Print the gateway's pin and assertion key.
    Keys,
    /// Check the configuration and the directory account.
    Check,
    /// Check a user's sign-in with the directory, and whether they may open
    /// a target.
    Login {
        /// The user.
        user: String,
        /// A host they would open, by ID or name.
        target: Option<String>,
    },
    /// Make a user a new TOTP secret.
    Enroll(String),
    /// Forget a user's TOTP secret.
    Remove(String),
    /// List the users with a TOTP secret.
    List,
    /// Show the usage text.
    Help,
}

/// The command line: a command and the configuration file.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Invocation {
    /// The command.
    command: Command,
    /// The configuration file.
    config: PathBuf,
}

/// Parses the command line, without the program name.
fn parse_args(arguments: impl IntoIterator<Item = String>) -> Result<Invocation, String> {
    let mut config = PathBuf::from(DEFAULT_CONFIG);
    let mut words = Vec::new();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--config" => {
                config = arguments.next().ok_or("--config needs a file")?.into();
            }
            other if other.starts_with("--") => return Err(format!("unknown option: {other}")),
            _ => words.push(argument),
        }
    }
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    let command = match words.as_slice() {
        ["serve"] => Command::Serve,
        ["keys"] => Command::Keys,
        ["check"] => Command::Check,
        ["login", user] => Command::Login {
            user: (*user).to_owned(),
            target: None,
        },
        ["login", user, target] => Command::Login {
            user: (*user).to_owned(),
            target: Some((*target).to_owned()),
        },
        ["totp", "enroll", user] => Command::Enroll((*user).to_owned()),
        ["totp", "remove", user] => Command::Remove((*user).to_owned()),
        ["totp", "list"] => Command::List,
        [] | ["help"] => Command::Help,
        _ => return Err(format!("unknown command: {}", words.join(" "))),
    };
    Ok(Invocation { command, config })
}

fn main() -> ExitCode {
    // Plain logs where no one reads colours, as in the system's journal.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
    let result = parse_args(std::env::args().skip(1)).and_then(run);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("qdp-gateway: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Runs a command.
fn run(invocation: Invocation) -> Result<(), String> {
    if invocation.command == Command::Help {
        println!("{USAGE}");
        return Ok(());
    }
    let config = Config::load(&invocation.config).map_err(|error| error.to_string())?;
    match invocation.command {
        Command::Serve => serve(config),
        Command::Keys => keys(&config),
        Command::Check => check(&config),
        Command::Login { user, target } => login(&config, &user, target.as_deref()),
        Command::Enroll(user) => enroll(&config, &user),
        Command::Remove(user) => remove(&config.state_directory, &user),
        Command::List => list(&config.state_directory),
        Command::Help => Ok(()),
    }
}

/// Runs `future` to completion on a runtime of its own.
fn block_on<T>(future: impl Future<Output = T>) -> Result<T, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the runtime: {error}"))?;
    Ok(runtime.block_on(future))
}

/// Serves users until interrupted.
fn serve(config: Config) -> Result<(), String> {
    let directory = LdapDirectory::new(&config.directory).map_err(|error| error.to_string())?;
    let keys =
        GatewayKeys::load(&config).map_err(|error| format!("cannot load the keys: {error}"))?;
    let totp = TotpStore::load(&config.state_directory).map_err(|error| error.to_string())?;
    let listen = config.listen;
    block_on(async move {
        let gateway = Arc::new(Gateway::new(config, directory, keys, totp));
        let endpoint = gateway
            .endpoint(listen)
            .map_err(|error| format!("cannot listen on {listen}: {error}"))?;
        tracing::info!(address = %listen, "serving");
        let serving = tokio::spawn(Arc::clone(&gateway).serve(endpoint.clone()));
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("shutting down");
        }
        endpoint.close(
            quinn::VarInt::from_u64(GatewayErrorCode::SHUTDOWN.0.into_inner())
                .unwrap_or(quinn::VarInt::MAX),
            b"the gateway is shutting down",
        );
        endpoint.wait_idle().await;
        serving.abort();
        Ok(())
    })?
}

/// Prints the gateway's pin and assertion key.
fn keys(config: &Config) -> Result<(), String> {
    let keys =
        GatewayKeys::load(config).map_err(|error| format!("cannot load the keys: {error}"))?;
    let assertion: String = keys
        .assertion_public_key()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!("Clients pin the gateway with: {}", keys.tls_pin().to_hex());
    println!("Hosts trust its assertions with: {assertion}");
    Ok(())
}

/// Checks that the directory account binds.
fn check(config: &Config) -> Result<(), String> {
    let directory = LdapDirectory::new(&config.directory).map_err(|error| error.to_string())?;
    block_on(directory.check())?.map_err(|error| error.to_string())?;
    println!(
        "The configuration is valid, and {} binds to {}. {} hosts and {} routes are configured.",
        config.directory.bind_dn,
        config.directory.url,
        config.hosts.len(),
        config.routes.len()
    );
    Ok(())
}

/// Checks `user`'s password with the directory, and shows what they may
/// reach, and whether they may open `target`.
fn login(config: &Config, user: &str, target: Option<&str>) -> Result<(), String> {
    let password = std::env::var(PASSWORD_VARIABLE)
        .map_err(|_| format!("set {PASSWORD_VARIABLE} to {user}'s password"))?;
    let directory = LdapDirectory::new(&config.directory).map_err(|error| error.to_string())?;
    let found =
        block_on(directory.authenticate(user, &password))?.map_err(|error| error.to_string())?;
    println!("{} ({})", found.name, found.display_name);
    for group in &found.groups {
        println!("  in {group}");
    }
    let hosts = policy::reachable_hosts(config, &found);
    if hosts.is_empty() {
        println!("May reach no listed host.");
    }
    for host in hosts {
        println!("May reach {} ({}) at {}", host.name, host.id, host.address);
    }
    if config.assertions.enabled {
        let groups = policy::reported_groups(config, &found);
        println!("Assertions report the groups: {}", groups.join(", "));
    }
    if let Some(target) = target {
        println!("{}", open_verdict(config, &found, target)?);
    }
    let store = TotpStore::load(&config.state_directory).map_err(|error| error.to_string())?;
    if store.get(&found.name).is_none() {
        println!(
            "Has no TOTP secret yet: qdp-gateway totp enroll {}",
            found.name
        );
    }
    Ok(())
}

/// Returns whether `user` may open `target`, a listed host's ID or a name or
/// address that a route allows, resolving the name as the gateway does.
fn open_verdict(config: &Config, user: &DirectoryUser, target: &str) -> Result<String, String> {
    if let Some(host) = policy::reachable_host(config, user, target) {
        return Ok(format!("May open {target}, the listed host {}", host.name));
    }
    let Some(routed) = RoutedTarget::parse(target) else {
        return Ok(format!(
            "May not open {target}, which is no listed host or name"
        ));
    };
    let addresses: Vec<IpAddr> = match routed.address() {
        Some(address) => vec![address],
        None => block_on(tokio::net::lookup_host((routed.name.as_str(), routed.port)))?
            .map(|addresses| addresses.map(|address| address.ip()).collect())
            .unwrap_or_default(),
    };
    let shown: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
    let verdict = if policy::routes_allow(config, user, &routed, &addresses) {
        "May open"
    } else {
        "May not open"
    };
    let at = if shown.is_empty() {
        "no address".to_owned()
    } else {
        shown.join(", ")
    };
    Ok(format!(
        "{verdict} {}:{}, at {at}",
        routed.name, routed.port
    ))
}

/// Makes `user` a new TOTP secret and prints the URI that enrolls it.
fn enroll(config: &Config, user: &str) -> Result<(), String> {
    let mut store = TotpStore::load(&config.state_directory).map_err(|error| error.to_string())?;
    let secret = TotpSecret::generate().map_err(|error| error.to_string())?;
    let uri = secret.enrollment_uri(config.issuer(), &user.to_lowercase());
    store
        .set(user, secret)
        .map_err(|error| format!("cannot keep the secret: {error}"))?;
    println!("{uri}");
    Ok(())
}

/// Forgets `user`'s TOTP secret.
fn remove(state_directory: &Path, user: &str) -> Result<(), String> {
    let mut store = TotpStore::load(state_directory).map_err(|error| error.to_string())?;
    if !store.remove(user).map_err(|error| error.to_string())? {
        return Err(format!("{user} has no TOTP secret"));
    }
    println!("Forgot {user}'s TOTP secret.");
    Ok(())
}

/// Lists the users with a TOTP secret.
fn list(state_directory: &Path) -> Result<(), String> {
    let store = TotpStore::load(state_directory).map_err(|error| error.to_string())?;
    for user in store.users() {
        println!("{user}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parses a command line written as one string.
    fn parse(line: &str) -> Result<Invocation, String> {
        parse_args(line.split_whitespace().map(str::to_owned))
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse("").unwrap().command, Command::Help);
        assert_eq!(
            parse("check").unwrap().config,
            PathBuf::from(DEFAULT_CONFIG)
        );
        assert_eq!(
            parse("totp enroll alice --config gw.toml").unwrap(),
            Invocation {
                command: Command::Enroll("alice".to_owned()),
                config: PathBuf::from("gw.toml"),
            }
        );
        assert_eq!(
            parse("login bob ws-12.corp.example.com").unwrap().command,
            Command::Login {
                user: "bob".to_owned(),
                target: Some("ws-12.corp.example.com".to_owned()),
            }
        );
        assert_eq!(parse("totp list").unwrap().command, Command::List);
        assert!(parse("totp").is_err());
        assert!(parse("check --verbose").is_err());
        assert!(parse("check --config").is_err());
    }
}
