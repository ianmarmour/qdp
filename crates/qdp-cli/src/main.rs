//! The `qdp` command: serve this machine's desktop, or connect to a host, with the
//! Quick Desktop Protocol.
//!
//! `qdp serve` streams a test pattern until a screen capture backend exists for
//! the platform; `qdp connect` decodes the stream and can write it to a
//! YUV4MPEG2 file until it has a window.

mod args;
mod identity;
mod sink;

use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use qdp_quic::{ClientEndpoint, HostEndpoint, HostVerification};
use qdp_runtime::{
    ClientCredentials, ClientOptions, DisplaySize, EncoderTuning, HostAuthenticator, HostOptions,
    TestPattern,
};
use tracing::{error, info, warn};
use tracing_subscriber::filter::LevelFilter;

use crate::args::{Command, ConnectArgs, ServeArgs, USAGE, format_pin, parse_args};
use crate::sink::CliSink;

/// The environment variable that holds the password for `--user`.
const PASSWORD_VARIABLE: &str = "QDP_PASSWORD";

/// The environment variable that sets the log level.
const LOG_VARIABLE: &str = "QDP_LOG";

/// The exit code for a command line the program does not understand.
const USAGE_EXIT_CODE: u8 = 2;

/// The client name sent in `CLIENT_HELLO`.
const CLIENT_NAME: &str = concat!("qdp/", env!("CARGO_PKG_VERSION"));

/// The display size the client requests until it has a window to measure.
const DEFAULT_DISPLAY: DisplaySize = DisplaySize {
    width: 1920,
    height: 1080,
    refresh_millihertz: 60_000,
};

/// An error that ends the program, with a message for the user.
type CliResult = Result<(), String>;

fn main() -> ExitCode {
    init_logging();
    let command = match parse_args(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("error: {error}\n\n{USAGE}");
            return ExitCode::from(USAGE_EXIT_CODE);
        }
    };
    let result = match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Command::Pin { identity } => print_pin(&identity),
        Command::Serve(args) => run_async(serve(args)),
        Command::Connect(args) => run_async(connect(args)),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            error!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Sets up logging to standard error at the level `QDP_LOG` names, `info` by
/// default.
fn init_logging() {
    let level = std::env::var(LOG_VARIABLE)
        .ok()
        .and_then(|level| level.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::INFO);
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .init();
}

/// Runs `future` on a multi-threaded runtime.
fn run_async(future: impl Future<Output = CliResult>) -> CliResult {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the runtime: {error}"))?
        .block_on(future)
}

/// Prints the host's key pin, creating the identity if needed.
fn print_pin(directory: &std::path::Path) -> CliResult {
    let identity = identity::load_or_create(directory)
        .map_err(|error| format!("cannot load the host identity: {error}"))?;
    let pin = identity.key_pin().map_err(|error| error.to_string())?;
    println!("{}", format_pin(pin));
    Ok(())
}

/// Returns the password from `QDP_PASSWORD`.
fn password() -> Result<String, String> {
    std::env::var(PASSWORD_VARIABLE).map_err(|_| {
        format!("--user needs the password in the {PASSWORD_VARIABLE} environment variable")
    })
}

/// Runs `qdp serve`: accepts clients until interrupted, serving each one in its
/// own task.
async fn serve(args: ServeArgs) -> CliResult {
    let identity = identity::load_or_create(&args.identity)
        .map_err(|error| format!("cannot load the host identity: {error}"))?;
    let pin = identity.key_pin().map_err(|error| error.to_string())?;

    let mut authenticator = HostAuthenticator::new().map_err(|error| error.to_string())?;
    for token in &args.tokens {
        authenticator.add_token(token.as_bytes().to_vec());
    }
    if let Some(user) = &args.user {
        authenticator
            .add_password(user, &password()?)
            .map_err(|error| error.to_string())?;
    }
    let options = Arc::new(HostOptions {
        host_name: hostname(),
        authenticator,
        frame_rate: args.fps,
        encoder: EncoderTuning {
            speed: args.speed,
            quantizer: args.quantizer,
            threads: 0,
        },
    });

    let endpoint = HostEndpoint::bind(args.listen, identity)
        .map_err(|error| format!("cannot listen on {}: {error}", args.listen))?;
    let address = endpoint
        .local_address()
        .map_err(|error| error.to_string())?;
    info!(%address, pin = format_pin(pin), "serving; clients connect with --pin {}", format_pin(pin));
    let (width, height) = args.size;

    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(connection) = incoming else {
                    return Ok(());
                };
                let options = Arc::clone(&options);
                tokio::spawn(async move {
                    let connection = match connection {
                        Ok(connection) => connection,
                        Err(error) => {
                            warn!(%error, "connection failed");
                            return;
                        }
                    };
                    let peer = connection.quic().remote_address();
                    info!(%peer, "client connected");
                    let source = Box::new(TestPattern::new(width, height));
                    match qdp_runtime::serve(connection, &options, source).await {
                        Ok(summary) => info!(
                            %peer,
                            frames = summary.frames_sent,
                            key_frames = summary.key_frames_sent,
                            retransmissions = summary.retransmissions,
                            input_reports = summary.input_reports,
                            "client disconnected"
                        ),
                        Err(error) => warn!(%peer, %error, "session ended with an error"),
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => {
                info!("shutting down");
                endpoint.close();
                return Ok(());
            }
        }
    }
}

/// Returns this machine's name for `HOST_HELLO`, or an empty name if unknown.
fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|name| name.trim().to_owned())
        })
        .unwrap_or_default()
}

/// Runs `qdp connect`: connects to the host and receives until the frame limit,
/// an interrupt, or the host closing.
async fn connect(args: ConnectArgs) -> CliResult {
    let credentials = match (&args.token, &args.user) {
        (Some(token), _) => ClientCredentials::Token(token.as_bytes().to_vec()),
        (None, Some(user)) => ClientCredentials::Password {
            username: user.clone(),
            password: password()?,
        },
        (None, None) => return Err("connect needs --token or --user".to_owned()),
    };
    let local = if args.address.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    };
    let endpoint = ClientEndpoint::bind(local).map_err(|error| error.to_string())?;
    let connection = endpoint
        .connect(
            args.address,
            &args.server_name,
            HostVerification::PinnedKeys(vec![args.pin]),
        )
        .await
        .map_err(|error| format!("cannot connect to {}: {error}", args.address))?;
    info!(address = %args.address, "connected");

    let options = ClientOptions {
        client_name: CLIENT_NAME.to_owned(),
        credentials,
        display: DEFAULT_DISPLAY,
        decoder_threads: 0,
    };
    let mut sink = CliSink::new(args.output.as_deref(), args.frames)
        .map_err(|error| format!("cannot create the output file: {error}"))?;
    let result = tokio::select! {
        result = qdp_runtime::run(connection, &options, &mut sink) => result.map(Some),
        _ = tokio::signal::ctrl_c() => Ok(None),
    };
    sink.finish()
        .map_err(|error| format!("cannot write the output file: {error}"))?;
    match result {
        Ok(Some(summary)) => {
            info!(
                frames = summary.frames_presented,
                decode_failures = summary.decode_failures,
                "disconnected"
            );
        }
        Ok(None) => info!(frames = sink.frames(), "interrupted"),
        Err(error) => return Err(format!("session failed: {error}")),
    }
    endpoint.wait_idle().await;
    Ok(())
}
