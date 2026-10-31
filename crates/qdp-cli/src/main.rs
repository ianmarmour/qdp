//! The `qdp` command: serve this machine's desktop, or connect to a host, with the
//! Quick Desktop Protocol.
//!
//! `qdp serve` streams the screen, or a test pattern; `qdp connect` shows the
//! stream in a window, with the host's cursor as its pointer, or with
//! `--headless` only logs statistics and can write it to a YUV4MPEG2 file.

mod args;
#[cfg(feature = "window")]
mod color;
#[cfg(feature = "window")]
mod cursor;
mod identity;
#[cfg(feature = "window")]
mod keymap;
mod sink;
#[cfg(feature = "window")]
mod window;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use qdp_codec::DecoderCommand;
use qdp_quic::{ClientEndpoint, HostEndpoint, HostVerification};
use qdp_runtime::{
    Admission, ClientAudio, ClientCredentials, ClientOptions, ClientSummary, ClipboardDirections,
    DesktopLock, DisplaySize, EncoderTuning, FrameSink, FrameSource, HostAudio, HostAuthenticator,
    HostDesktop, HostOptions, InputQueue, OpenedDesktop, ResumptionSlot, RuntimeError,
    SharedClipboard, TestPattern,
};
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::args::{
    Audio, Command, ConnectArgs, DECODER_COMMAND, DecoderPlacement, LockPolicy, Presentation,
    ServeArgs, SourceKind, USAGE, format_pin, parse_args,
};
use crate::identity::Role;
use crate::sink::CliSink;

/// The environment variable that holds the password for `--user`.
const PASSWORD_VARIABLE: &str = "QDP_PASSWORD";

/// The environment variable that sets the log level.
const LOG_VARIABLE: &str = "QDP_LOG";

/// The log target of the D-Bus library the screen capture portal uses.
const ZBUS_TARGET: &str = "zbus";

/// The exit code for a command line the program does not understand.
const USAGE_EXIT_CODE: u8 = 2;

/// The client name sent in `CLIENT_HELLO`.
const CLIENT_NAME: &str = concat!("qdp/", env!("CARGO_PKG_VERSION"));

/// The display a client without a window requests, and a window's until it
/// has measured its own.
const DEFAULT_DISPLAY: DisplaySize = DisplaySize {
    width: 1920,
    height: 1080,
    refresh_millihertz: 60_000,
    scale_percent: DisplaySize::UNSCALED,
};

/// The name of the thread that runs the session while the window runs on the
/// main thread.
#[cfg(feature = "window")]
const SESSION_THREAD_NAME: &str = "qdp-session";

/// How long a client waits before its first attempt to resume a lost
/// connection.
const FIRST_RECONNECT_DELAY: Duration = Duration::from_millis(500);

/// The longest a client waits between attempts to resume a lost
/// connection.
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(8);

/// How long the session waits for the window to measure its display.
#[cfg(feature = "window")]
const DISPLAY_WAIT: Duration = Duration::from_secs(5);

/// How long the session may take to end on its own after the window closes,
/// before it is cut off. It normally ends at the next frame, saying goodbye to
/// the host.
#[cfg(feature = "window")]
const CLOSE_GRACE: Duration = Duration::from_millis(500);

/// How long the host waits, when it shuts down, for its sessions to return
/// the client's devices to rest and for the clients to learn why the
/// connections closed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

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
        Command::Pin { identity, role } => print_pin(&identity, role),
        Command::Serve(args) => run_async(serve(args)),
        Command::Connect(args) => connect(args),
        Command::Decoder => qdp_codec::serve_decoder_on_stdio()
            .map_err(|error| format!("the decoder process failed: {error}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            error!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Sets up logging to standard error as `QDP_LOG` asks, `info` by default:
/// a level, or a level followed by levels for some targets, such as
/// `info,qdp::window=debug`. D-Bus warnings from zbus, which reports internal
/// details of portal calls, are shown only when they are errors, unless
/// `QDP_LOG` names zbus itself.
fn init_logging() {
    let filter = log_filter(std::env::var(LOG_VARIABLE).ok().as_deref());
    let most = filter
        .iter()
        .map(|(_, level)| level)
        .chain(filter.default_level())
        .max()
        .unwrap_or(LevelFilter::INFO);
    tracing_subscriber::fmt()
        .with_max_level(most)
        .with_writer(std::io::stderr)
        .finish()
        .with(filter)
        .init();
}

/// Returns the log filter `QDP_LOG` asks for, or `info` if it is unset or
/// cannot be read.
fn log_filter(setting: Option<&str>) -> Targets {
    let filter = setting
        .and_then(|setting| setting.parse::<Targets>().ok())
        .unwrap_or_default();
    let default = filter.default_level().unwrap_or(LevelFilter::INFO);
    let filter = filter.with_default(default);
    if filter.iter().any(|(target, _)| target == ZBUS_TARGET) {
        filter
    } else {
        filter.with_target(ZBUS_TARGET, default.min(LevelFilter::ERROR))
    }
}

/// Runs `future` on a multi-threaded runtime.
fn run_async<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the runtime: {error}"))?
        .block_on(future)
}

/// Prints the host's key pin, creating the identity if needed.
fn print_pin(directory: &std::path::Path, role: Role) -> CliResult {
    let identity = identity::load_or_create(directory, role)
        .map_err(|error| format!("cannot load the identity: {error}"))?;
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
    let identity = identity::load_or_create(&args.identity, Role::Host)
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
    for &pin in &args.client_keys {
        authenticator.add_client_key(pin);
    }
    authenticator.set_resumption_lifetime(args.resumption);
    let options = Arc::new(HostOptions {
        host_name: hostname(),
        authenticator,
        frame_rate: args.fps,
        encoder: EncoderTuning {
            preference: args.encoder,
            speed: args.speed,
            quantizer: args.quantizer,
            threads: 0,
        },
        passthrough: args.passthrough,
    });

    let endpoint = HostEndpoint::bind(args.listen, identity)
        .map_err(|error| format!("cannot listen on {}: {error}", args.listen))?;
    let address = endpoint
        .local_address()
        .map_err(|error| error.to_string())?;
    info!(%address, pin = format_pin(pin), "serving; clients connect with --pin {}", format_pin(pin));
    let (width, height) = args.size;
    let source_kind = args.source;
    let clipboard_directions = args.clipboard;
    let audio_setting = args.audio;
    let lock_policy = args.lock;
    let state_directory = Arc::new(args.identity.clone());
    // What the host can inject is found once, before any client connects,
    // so that no input device exists for a client that has not
    // authenticated; each session opens its own injector once its client
    // has (Section 11.2).
    let input_classes =
        qdp_runtime::input_classes(&*qdp_desktop::input_injector(1, 1), args.passthrough);

    let admission = Admission::default();
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else {
                    break;
                };
                let peer = incoming.remote_address();
                // Under load, clients prove their address before taking a
                // place (Section 11.7).
                if admission.busy() && !incoming.address_validated() {
                    incoming.retry();
                    continue;
                }
                let ticket = match admission.admit(peer.ip()) {
                    Ok(ticket) => ticket,
                    Err(refusal) => {
                        debug!(%peer, %refusal, "connection refused");
                        incoming.refuse();
                        continue;
                    }
                };
                let options = Arc::clone(&options);
                let state_directory = Arc::clone(&state_directory);
                sessions.spawn(async move {
                    let connection = match incoming.establish().await {
                        Ok(connection) => connection,
                        Err(error) => {
                            warn!(%peer, %error, "connection failed");
                            return;
                        }
                    };
                    info!(%peer, "client connected");
                    let desktop = HostDesktop {
                        input_classes,
                        clipboard: shared_clipboard(clipboard_directions),
                        audio: match audio_setting {
                            Audio::On => qdp_desktop::host_audio(),
                            Audio::Off => HostAudio::default(),
                        },
                        open: Box::new(move || {
                            Box::pin(open_desktop(
                                source_kind,
                                (width, height),
                                state_directory,
                                lock_policy,
                            ))
                        }),
                    };
                    let result = qdp_runtime::serve(connection, ticket, &options, desktop).await;
                    match result {
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
                // Forget the sessions that have ended.
                while sessions.try_join_next().is_some() {}
            }
            _ = tokio::signal::ctrl_c() => {
                info!("shutting down");
                endpoint.close();
                break;
            }
        }
    }
    // The sessions return their devices to rest before the host exits
    // (Section 4.7), and the clients learn why the connections closed.
    let ended = async { while sessions.join_next().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_GRACE, ended).await.is_err() {
        warn!("sessions did not end in time");
    }
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, endpoint.wait_idle()).await;
    Ok(())
}

/// Returns the client's audio devices, as the user asked for them.
fn client_audio(args: &ConnectArgs) -> ClientAudio {
    let devices = qdp_desktop::client_audio(args.microphone);
    match args.audio {
        Audio::On => devices,
        Audio::Off => ClientAudio {
            speakers: None,
            ..devices
        },
    }
}

/// Opens the system clipboard to share `directions`, if it is shared at all
/// and can be opened.
fn shared_clipboard(directions: Option<ClipboardDirections>) -> Option<SharedClipboard> {
    let directions = directions?;
    let clipboard = qdp_desktop::system_clipboard()?;
    Some(SharedClipboard {
        clipboard,
        directions,
    })
}

/// Opens a session's desktop once its client has authenticated: what it
/// streams, as [`open_source`] opens it, an input injector for it, and,
/// when it streams the screen, the lock that `lock` asks for.
async fn open_desktop(
    kind: SourceKind,
    size: (usize, usize),
    state_directory: Arc<PathBuf>,
    lock: LockPolicy,
) -> Result<OpenedDesktop, RuntimeError> {
    let (source, streamed) = open_source(kind, size, state_directory)
        .await
        .map_err(RuntimeError::Capture)?;
    let (width, height) = source.size();
    let injector = qdp_desktop::input_injector(width as u32, height as u32);
    let lock = match (streamed, lock) {
        (Streamed::Screen, LockPolicy::Lock) => Some(Box::new(lock_desktop) as DesktopLock),
        (Streamed::TestPattern, _) | (_, LockPolicy::Leave) => None,
    };
    Ok(OpenedDesktop {
        source,
        injector,
        lock,
    })
}

/// Locks this machine's desktop, or says why it cannot.
fn lock_desktop() {
    if let Err(error) = qdp_desktop::lock_desktop() {
        warn!(%error, "cannot lock the desktop");
    }
}

/// What a session streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Streamed {
    /// The screen.
    Screen,
    /// The test pattern.
    TestPattern,
}

/// Opens what a session streams: the screen, or a `size` test pattern.
/// Capturing the screen can wait for the user to choose a monitor, so it runs
/// on a blocking thread.
async fn open_source(
    kind: SourceKind,
    size: (usize, usize),
    state_directory: Arc<PathBuf>,
) -> Result<(Box<dyn FrameSource>, Streamed), String> {
    let test_pattern = || {
        let pattern = Box::new(TestPattern::new(size.0, size.1)) as Box<dyn FrameSource>;
        (pattern, Streamed::TestPattern)
    };
    if kind == SourceKind::TestPattern {
        return Ok(test_pattern());
    }
    let screen = tokio::task::spawn_blocking(move || qdp_desktop::open_screen(&state_directory))
        .await
        .map_err(|error| error.to_string())?;
    match (screen, kind) {
        (Ok(screen), _) => Ok((screen, Streamed::Screen)),
        (Err(error), SourceKind::Auto) => {
            warn!(%error, "streaming a test pattern instead of the screen");
            Ok(test_pattern())
        }
        (Err(error), _) => Err(error),
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

/// Runs `qdp connect`: shows the host's stream until the frame limit, an
/// interrupt, the window closing, or the host closing.
fn connect(args: ConnectArgs) -> CliResult {
    let sink = CliSink::new(args.output.as_deref(), args.frames)
        .map_err(|error| format!("cannot create the output file: {error}"))?;
    match args.presentation {
        Presentation::Headless => connect_headless(&args, sink),
        #[cfg(feature = "window")]
        Presentation::Window => connect_in_window(args, sink),
        #[cfg(not(feature = "window"))]
        Presentation::Window => {
            Err("this qdp was built without the window feature; pass --headless".to_owned())
        }
    }
}

/// Receives the stream without a window, until the frame limit, an interrupt,
/// or the host closing.
fn connect_headless(args: &ConnectArgs, mut sink: CliSink) -> CliResult {
    let result = run_async(receive(
        args,
        DEFAULT_DISPLAY,
        &mut sink,
        interrupted(),
        None,
    ));
    finish(&mut sink, result)
}

/// Shows the stream in a window. The window runs on this thread, which must be
/// the main thread, and the session on another.
#[cfg(feature = "window")]
fn connect_in_window(args: ConnectArgs, sink: CliSink) -> CliResult {
    use crate::window::{
        CloseSignal, FrameSlot, WindowApp, WindowChoices, WindowMessage, WindowSink,
    };

    let event_loop = window::event_loop()?;
    let (input, input_queue) = qdp_runtime::input_channel();
    let passthrough = args.passthrough.clone();
    let touchpad = args.touchpad;
    let slot = FrameSlot::default();
    let close = Arc::new(CloseSignal::default());
    let title = format!("QDP: {}", args.address);
    let microphone = args.microphone;
    let proxy = event_loop.create_proxy();
    let mut sink = WindowSink::new(sink, slot.clone(), proxy.clone(), Arc::clone(&close));
    let (display_sender, display) = tokio::sync::oneshot::channel();
    let session_close = Arc::clone(&close);
    let session = std::thread::Builder::new()
        .name(SESSION_THREAD_NAME.to_owned())
        .spawn(move || {
            let stop = async {
                tokio::select! {
                    () = interrupted() => {}
                    () = window_closed(&session_close) => {}
                }
            };
            let result = run_async(async {
                // The window measures the display once it opens.
                let display = tokio::time::timeout(DISPLAY_WAIT, display)
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(DEFAULT_DISPLAY);
                receive(&args, display, &mut sink, stop, Some(input_queue)).await
            });
            let result = finish(sink.inner_mut(), result);
            // The window may have closed already, so nobody may listen.
            let _ = proxy.send_event(WindowMessage::SessionEnded);
            result
        })
        .map_err(|error| format!("cannot start the session: {error}"))?;

    let choices = WindowChoices {
        title,
        microphone,
        passthrough: &passthrough,
        touchpad,
    };
    let mut app = WindowApp::new(
        choices,
        slot,
        Arc::clone(&close),
        input,
        event_loop.create_proxy(),
        display_sender,
    );
    let window_result = event_loop
        .run_app(&mut app)
        .map_err(|error| format!("the window failed: {error}"));
    // The window is gone; stop the session if it still runs.
    close.close();
    let session_result = session
        .join()
        .unwrap_or_else(|_| Err("the session thread panicked".to_owned()));
    window_result?;
    if let Some(error) = app.error() {
        return Err(format!(
            "{error}; pass --headless to connect without a window"
        ));
    }
    session_result
}

/// Completes once the window has closed and the session has had
/// [`CLOSE_GRACE`] to end on its own.
#[cfg(feature = "window")]
async fn window_closed(close: &window::CloseSignal) {
    close.closed().await;
    tokio::time::sleep(CLOSE_GRACE).await;
}

/// Completes on Ctrl-C, or never if the interrupt cannot be watched.
async fn interrupted() {
    if tokio::signal::ctrl_c().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Connects to the host and presents its stream in `sink`, shown on
/// `display`, until the sink stops, the host closes, or `stop` completes,
/// sending the user's input from `input`, if given. A connection that is
/// lost is resumed with the host's resumption token, if it gave one, while
/// the token is valid (Section 4.4.7). Returns the session's summary, or
/// `None` if `stop` cut it off.
async fn receive(
    args: &ConnectArgs,
    display: DisplaySize,
    sink: &mut dyn FrameSink,
    stop: impl Future<Output = ()>,
    mut input: Option<InputQueue>,
) -> Result<Option<ClientSummary>, String> {
    let credentials = match (&args.token, &args.user) {
        (Some(token), _) => ClientCredentials::Token(token.as_bytes().to_vec()),
        (None, Some(user)) => ClientCredentials::Password {
            username: user.clone(),
            password: password()?,
        },
        (None, None) => ClientCredentials::Certificate,
    };
    let local = if args.address.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    };
    let mut endpoint = ClientEndpoint::bind(local).map_err(|error| error.to_string())?;
    // The client presents its certificate only when it authenticates with
    // it, so that other hosts do not learn its key.
    if credentials == ClientCredentials::Certificate {
        let identity = identity::load_or_create(&args.identity, Role::Client)
            .map_err(|error| format!("cannot load the client identity: {error}"))?;
        let pin = identity.key_pin().map_err(|error| error.to_string())?;
        info!(
            pin = format_pin(pin),
            "authenticating with this client's key; hosts accept it with --client-key {}",
            format_pin(pin)
        );
        endpoint.set_identity(identity);
    }
    let resumption = ResumptionSlot::default();
    let sessions = async {
        let mut credentials = credentials;
        // While resuming: when the token in hand expires.
        let mut deadline = None;
        let mut delay = FIRST_RECONNECT_DELAY;
        loop {
            let options = ClientOptions {
                client_name: CLIENT_NAME.to_owned(),
                credentials: credentials.clone(),
                display,
                decoder_threads: 0,
                decoder: args.decoder,
                decoder_process: decoder_process(args)?,
                keyboard_info: qdp_desktop::keyboard_info(),
                resumption: resumption.clone(),
            };
            let error = match connect_once(args, &endpoint, &options, sink, input.as_mut()).await {
                Ok(summary) => return Ok(summary),
                Err(Failure::Fatal(error)) => return Err(error),
                Err(Failure::Lost(error)) => error,
            };
            // A session that became active left a fresh token to resume it
            // with; otherwise the token in hand is tried until it expires,
            // since a connection that was never made did not use it.
            let now = Instant::now();
            if let (Some(expires_at), Some(token)) =
                (resumption.expires_at(), resumption.take_credentials(now))
            {
                deadline = Some(expires_at);
                credentials = token;
                delay = FIRST_RECONNECT_DELAY;
            }
            if deadline.is_none_or(|deadline| now + delay >= deadline) {
                return Err(error);
            }
            warn!(%error, retry_in = ?delay, "connection lost; reconnecting");
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(MAX_RECONNECT_DELAY);
        }
    };
    let summary = tokio::select! {
        result = sessions => result.map(Some)?,
        () = stop => None,
    };
    endpoint.wait_idle().await;
    Ok(summary)
}

/// Returns the command that starts a decoder process, if `args` asks for
/// one: this program, run as `qdp decoder`.
fn decoder_process(args: &ConnectArgs) -> Result<Option<DecoderCommand>, String> {
    match args.decoder_process {
        DecoderPlacement::InProcess => Ok(None),
        DecoderPlacement::SeparateProcess => {
            let program = std::env::current_exe()
                .map_err(|error| format!("cannot find this program to run its decoder: {error}"))?;
            Ok(Some(DecoderCommand {
                program,
                args: vec![DECODER_COMMAND.into()],
            }))
        }
    }
}

/// Why a connection ended without a session summary.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// The connection could not be made, or was lost: reconnecting may help.
    Lost(String),
    /// The session failed for another reason, which ends the client.
    Fatal(String),
}

/// Connects to the host once and runs a session with `options` until it
/// ends.
async fn connect_once(
    args: &ConnectArgs,
    endpoint: &ClientEndpoint,
    options: &ClientOptions,
    sink: &mut dyn FrameSink,
    input: Option<&mut InputQueue>,
) -> Result<ClientSummary, Failure> {
    let connection = endpoint
        .connect(
            args.address,
            &args.server_name,
            HostVerification::PinnedKeys(vec![args.pin]),
        )
        .await
        .map_err(|error| Failure::Lost(format!("cannot connect to {}: {error}", args.address)))?;
    info!(address = %args.address, "connected");
    qdp_runtime::run(
        connection,
        options,
        sink,
        input,
        shared_clipboard(args.clipboard),
        client_audio(args),
    )
    .await
    .map_err(|error| {
        let message = format!("session failed: {error}");
        if error.is_connection_lost() {
            Failure::Lost(message)
        } else {
            Failure::Fatal(message)
        }
    })
}

/// Finishes the output file and logs how the session ended.
fn finish(sink: &mut CliSink, result: Result<Option<ClientSummary>, String>) -> CliResult {
    sink.finish()
        .map_err(|error| format!("cannot write the output file: {error}"))?;
    match result? {
        Some(summary) => info!(
            frames = summary.frames_presented,
            decode_failures = summary.decode_failures,
            "disconnected"
        ),
        None => info!(frames = sink.frames(), "stopped"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `QDP_LOG` is a level, optionally followed by levels for some targets,
    /// and zbus keeps to errors unless it is named.
    #[test]
    fn reads_the_log_setting() {
        let level = |filter: &Targets, target: &str| {
            filter
                .iter()
                .find(|(name, _)| *name == target)
                .map(|(_, level)| level)
        };
        let unset = log_filter(None);
        assert_eq!(unset.default_level(), Some(LevelFilter::INFO));
        assert_eq!(level(&unset, ZBUS_TARGET), Some(LevelFilter::ERROR));

        let targeted = log_filter(Some("warn,qdp::window=debug"));
        assert_eq!(targeted.default_level(), Some(LevelFilter::WARN));
        assert_eq!(level(&targeted, "qdp::window"), Some(LevelFilter::DEBUG));
        assert_eq!(level(&targeted, ZBUS_TARGET), Some(LevelFilter::ERROR));

        let zbus = log_filter(Some("info,zbus=debug"));
        assert_eq!(level(&zbus, ZBUS_TARGET), Some(LevelFilter::DEBUG));

        // Only targets: the rest stays at info.
        let only = log_filter(Some("qdp=trace"));
        assert_eq!(only.default_level(), Some(LevelFilter::INFO));
        // Unreadable settings fall back to info.
        assert_eq!(
            log_filter(Some("=")).default_level(),
            Some(LevelFilter::INFO)
        );
    }
}
