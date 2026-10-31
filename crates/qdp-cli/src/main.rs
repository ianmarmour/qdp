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

use qdp_quic::{ClientEndpoint, HostEndpoint, HostVerification};
use qdp_runtime::{
    ClientAudio, ClientCredentials, ClientOptions, ClientSummary, ClipboardDirections, DisplaySize,
    EncoderTuning, FrameSink, FrameSource, HostAudio, HostAuthenticator, HostOptions, InputQueue,
    SharedClipboard, TestPattern,
};
use tracing::{error, info, warn};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use qdp_desktop::Microphone;

use crate::args::{
    Audio, Command, ConnectArgs, Presentation, ServeArgs, SourceKind, USAGE, format_pin, parse_args,
};
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

/// The display size the client requests until it has a window to measure.
const DEFAULT_DISPLAY: DisplaySize = DisplaySize {
    width: 1920,
    height: 1080,
    refresh_millihertz: 60_000,
};

/// The name of the thread that runs the session while the window runs on the
/// main thread.
#[cfg(feature = "window")]
const SESSION_THREAD_NAME: &str = "qdp-session";

/// How long the session may take to end on its own after the window closes,
/// before it is cut off. It normally ends at the next frame, saying goodbye to
/// the host.
#[cfg(feature = "window")]
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

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
        Command::Connect(args) => connect(args),
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
/// default. D-Bus warnings from zbus, which reports internal details of
/// portal calls, are shown only when they are errors.
fn init_logging() {
    let level = std::env::var(LOG_VARIABLE)
        .ok()
        .and_then(|level| level.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::INFO);
    let filter = Targets::new()
        .with_default(level)
        .with_target(ZBUS_TARGET, level.min(LevelFilter::ERROR));
    tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(std::io::stderr)
        .finish()
        .with(filter)
        .init();
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
    let state_directory = Arc::new(args.identity.clone());

    loop {
        tokio::select! {
            incoming = endpoint.accept() => {
                let Some(connection) = incoming else {
                    return Ok(());
                };
                let options = Arc::clone(&options);
                let state_directory = Arc::clone(&state_directory);
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
                    let source = match open_source(source_kind, (width, height), state_directory).await {
                        Ok(source) => source,
                        Err(error) => {
                            warn!(%peer, %error, "cannot capture the screen");
                            return;
                        }
                    };
                    let (source_width, source_height) = source.size();
                    let injector =
                        qdp_desktop::input_injector(source_width as u32, source_height as u32);
                    let clipboard = shared_clipboard(clipboard_directions);
                    let audio = match audio_setting {
                        Audio::On => qdp_desktop::host_audio(),
                        Audio::Off => HostAudio::default(),
                    };
                    match qdp_runtime::serve(connection, &options, source, injector, clipboard, audio).await {
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

/// Returns the client's audio devices, as the user asked for them.
fn client_audio(args: &ConnectArgs) -> ClientAudio {
    let devices = qdp_desktop::client_audio(args.microphone);
    match args.audio {
        Audio::On => devices,
        Audio::Off => ClientAudio {
            speakers: None,
            microphone: devices.microphone,
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

/// Opens what a session streams: the screen, or a `size` test pattern.
/// Capturing the screen can wait for the user to choose a monitor, so it runs
/// on a blocking thread.
async fn open_source(
    kind: SourceKind,
    size: (usize, usize),
    state_directory: Arc<PathBuf>,
) -> Result<Box<dyn FrameSource>, String> {
    let test_pattern = || Box::new(TestPattern::new(size.0, size.1)) as Box<dyn FrameSource>;
    if kind == SourceKind::TestPattern {
        return Ok(test_pattern());
    }
    let screen = tokio::task::spawn_blocking(move || qdp_desktop::open_screen(&state_directory))
        .await
        .map_err(|error| error.to_string())?;
    match (screen, kind) {
        (Ok(screen), _) => Ok(screen),
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
    let result = run_async(receive(args, &mut sink, interrupted(), None));
    finish(&mut sink, result)
}

/// Shows the stream in a window. The window runs on this thread, which must be
/// the main thread, and the session on another.
#[cfg(feature = "window")]
fn connect_in_window(args: ConnectArgs, sink: CliSink) -> CliResult {
    use crate::window::{CloseSignal, FrameSlot, WindowApp, WindowMessage, WindowSink};

    let event_loop = window::event_loop()?;
    let (input, input_queue) = qdp_runtime::input_channel();
    let passthrough = args.passthrough.clone();
    let touchpad = args.touchpad;
    let slot = FrameSlot::default();
    let close = Arc::new(CloseSignal::default());
    // The title shows while the microphone is on (Section 7.7).
    let title = match args.microphone {
        Microphone::On => format!("QDP: {} (microphone on)", args.address),
        Microphone::Off => format!("QDP: {}", args.address),
    };
    let proxy = event_loop.create_proxy();
    let mut sink = WindowSink::new(sink, Arc::clone(&slot), proxy.clone(), Arc::clone(&close));
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
            let result = run_async(receive(&args, &mut sink, stop, Some(input_queue)));
            let result = finish(sink.inner_mut(), result);
            // The window may have closed already, so nobody may listen.
            let _ = proxy.send_event(WindowMessage::SessionEnded);
            result
        })
        .map_err(|error| format!("cannot start the session: {error}"))?;

    let mut app = WindowApp::new(
        title,
        slot,
        Arc::clone(&close),
        input,
        event_loop.create_proxy(),
        &passthrough,
        touchpad,
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

/// Connects to the host and presents its stream in `sink` until the sink stops,
/// the host closes, or `stop` completes, sending the user's input from
/// `input`, if given. Returns the session's summary, or `None` if `stop` cut
/// it off.
async fn receive(
    args: &ConnectArgs,
    sink: &mut dyn FrameSink,
    stop: impl Future<Output = ()>,
    input: Option<InputQueue>,
) -> Result<Option<ClientSummary>, String> {
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
        decoder: args.decoder,
        keyboard_info: qdp_desktop::keyboard_info(),
    };
    let result = tokio::select! {
        result = qdp_runtime::run(
            connection,
            &options,
            sink,
            input,
            shared_clipboard(args.clipboard),
            client_audio(args),
        ) => {
            result.map(Some)
        }
        () = stop => Ok(None),
    };
    let summary = result.map_err(|error| format!("session failed: {error}"))?;
    endpoint.wait_idle().await;
    Ok(summary)
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
