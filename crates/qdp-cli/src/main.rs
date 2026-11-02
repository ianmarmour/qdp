//! The `qdp` command: serve this machine's desktop, or connect to a host, with the
//! Quick Desktop Protocol.
//!
//! `qdp serve` streams the screen, or a test pattern; `qdp connect` shows the
//! stream in a window, with the host's cursor as its pointer, or with
//! `--headless` only logs statistics and can write it to a YUV4MPEG2 file.

mod app_events;
mod args;
#[cfg(feature = "window")]
mod color;
mod discover;
mod gateway;
#[cfg(feature = "window")]
mod identity;
mod paired;
mod service;
mod sink;
#[cfg(feature = "window")]
mod window;

use std::io::{IsTerminal, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use qdp::discovery::{PairingOffer, ServiceRecord};
use qdp_codec::{CodecPreference, DecoderCommand};
use qdp_desktop::{
    Advertisement, Advertising, DisplayMatching, FrameMemory, HostPrivacy, Notification,
    ScreenOptions, Urgency,
};
use qdp_quic::{ClientEndpoint, HostEndpoint, HostVerification, KeyPin, TlsIdentity};
use qdp_runtime::{
    Admission, CODE_LIFETIME, ClientAudio, ClientCredentials, ClientOptions, ClientSummary,
    ClipboardDirections, DesktopLock, DisplaySize, EncoderTuning, FrameSink, FrameSource,
    GatewayFailure, GatewayRoute, GatewayTrust, HostAudio, HostAuthenticator, HostDesktop,
    HostOptions, InputQueue, OpenedDesktop, Pairing, PairingEvent, PairingOutcome, PairingPrompt,
    ResumptionSlot, RuntimeError, SharedClipboard, SignInPrompt, TestPattern, Tunnel, open_tunnel,
};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::{debug, error, info, warn};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use qdp_app::events::{CodeRequest, Event};

use crate::app_events::EventLines;
use crate::args::{
    AdvertiseMode, Audio, Command, ConnectArgs, DECODER_COMMAND, DecoderPlacement, Destination,
    GatewayArgs, GpuCapture, HostTrust, LockPolicy, PairedAction, PairingMode, Presentation,
    Privacy, ServeArgs, SourceKind, USAGE, format_pin, parse_args,
};
use crate::gateway::GatewaySessions;
use crate::identity::Role;
use crate::paired::{Paired, PairedFile};
use crate::sink::CliSink;

/// The environment variable that holds the password for `--user`.
const PASSWORD_VARIABLE: &str = "QDP_PASSWORD";

/// The environment variable `qdp connect` reads a token from when given no
/// `--token`, so that apps keep it out of the command line.
const TOKEN_VARIABLE: &str = "QDP_TOKEN";

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
/// before the client closes it. It normally ends at the next frame, saying
/// goodbye to the host.
#[cfg(feature = "window")]
const CLOSE_GRACE: Duration = Duration::from_millis(500);

/// The reason a client gives the host when the viewer stops the session.
const VIEWER_CLOSED: &str = "viewer closed";

/// How long the host waits, when it shuts down, for its sessions to return
/// the client's devices to rest and for the clients to learn why the
/// connections closed.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// How long the runtime waits, once the program's work is done, for its
/// blocking tasks before the program exits without them.
const RUNTIME_SHUTDOWN: Duration = Duration::from_secs(1);

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
        Command::Paired {
            identity,
            role,
            action,
        } => paired(&identity, role, action),
        Command::Service(action) => service::run(action),
        Command::Serve(args) => {
            adopt_session_display();
            run_async(serve(args))
        }
        Command::Connect(args) => connect(args),
        Command::Discover(mode) => run_async(discover::discover(mode)),
        Command::Decoder => qdp_codec::serve_decoder_on_stdio()
            .map_err(|error| format!("the decoder process failed: {error}")),
        Command::SessionHelper => qdp_desktop::run_session_helper(),
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
        // Colours only for a terminal, not in a service's journal.
        .with_ansi(std::io::stderr().is_terminal())
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

/// Names the session's Wayland display for the host when its environment
/// names none, as when it is started over SSH, so that the clipboard reaches
/// the compositor rather than an X server that is not there.
#[cfg(target_os = "linux")]
fn adopt_session_display() {
    // SAFETY: called before the runtime starts, while this is the only
    // thread.
    #[allow(unsafe_code)]
    let adopted = unsafe { qdp_desktop::adopt_wayland_display() };
    if let Some(name) = adopted {
        info!(socket = name, "using the session's Wayland display");
    }
}

/// Does nothing: only Linux sessions are found through the environment.
#[cfg(not(target_os = "linux"))]
fn adopt_session_display() {}

/// Runs `future` on a multi-threaded runtime. Blocking work still running
/// once it completes, such as a session's screen capture still waiting for
/// the user to choose a monitor, is left behind after [`RUNTIME_SHUTDOWN`]
/// rather than waited for, so that the program always exits.
fn run_async<T>(future: impl Future<Output = Result<T, String>>) -> Result<T, String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the runtime: {error}"))?;
    let result = runtime.block_on(future);
    runtime.shutdown_timeout(RUNTIME_SHUTDOWN);
    result
}

/// Prints the host's key pin, creating the identity if needed.
fn print_pin(directory: &std::path::Path, role: Role) -> CliResult {
    let identity = identity::load_or_create(directory, role)
        .map_err(|error| format!("cannot load the identity: {error}"))?;
    let pin = identity.key_pin().map_err(|error| error.to_string())?;
    println!("{}", format_pin(pin));
    Ok(())
}

/// Returns the token in `QDP_TOKEN`, if it holds one.
fn token_from_environment() -> Option<String> {
    std::env::var(TOKEN_VARIABLE)
        .ok()
        .filter(|token| !token.is_empty())
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
    if args.source == SourceKind::Console {
        // The console's session's notifications and clipboard, rather than
        // this process's own session's, which a service has none of.
        qdp_desktop::follow_console_session();
    }
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
    if !args.gateway_keys.is_empty() {
        let mut trust = GatewayTrust::new(pin);
        for &key in &args.gateway_keys {
            trust.add_key(key);
        }
        for group in &args.gateway_groups {
            trust.require_group(group.clone());
        }
        authenticator.trust_gateways(trust);
        info!(
            gateways = args.gateway_keys.len(),
            "users the gateways vouch for may connect"
        );
    }
    let paired = PairedFile::clients(&args.identity);
    for client in paired
        .load()
        .map_err(|error| format!("cannot read the paired clients: {error}"))?
    {
        authenticator.add_client_key(client.pin);
    }
    if args.pairing == PairingMode::On {
        authenticator.enable_pairing(Pairing::new(
            pin,
            pairing_listener(paired, pairing_notifier()),
        ));
        info!("new clients may pair: the code to type on them appears here");
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
    let advertising = match args.advertise {
        AdvertiseMode::On => advertise(&options.host_name, address, pin, args.pairing).await,
        AdvertiseMode::Off => None,
    };
    let (width, height) = args.size;
    let source_kind = args.source;
    let clipboard_directions = args.clipboard;
    let audio_setting = args.audio;
    let lock_policy = args.lock;
    let privacy = args.privacy;
    let match_display = args.match_display;
    let screen = ScreenOptions {
        matching: match_display,
        frames: frame_memory(args.gpu_capture, args.encoder).await,
    };
    let state_directory = Arc::new(args.identity.clone());
    if match_display == DisplayMatching::On {
        // A host that stopped without putting the monitor back left its
        // settings saved.
        let directory = Arc::clone(&state_directory);
        tokio::task::spawn_blocking(move || qdp_desktop::restore_monitor_settings(&directory))
            .await
            .map_err(|error| error.to_string())?;
    }
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
                let local = LocalUser::new(privacy);
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
                            Audio::On => host_audio(source_kind),
                            Audio::Off => HostAudio::default(),
                        },
                        open: Box::new({
                            let local = local.clone();
                            move || {
                                Box::pin(open_desktop(
                                    source_kind,
                                    (width, height),
                                    state_directory,
                                    lock_policy,
                                    screen,
                                    local,
                                    peer,
                                ))
                            }
                        }),
                    };
                    let result = qdp_runtime::serve(connection, ticket, &options, desktop).await;
                    local.ended().await;
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
    if let Some(advertising) = advertising {
        advertising.withdraw().await;
    }
    // The sessions return their devices to rest before the host exits
    // (Section 4.7), and the clients learn why the connections closed. A
    // second Ctrl-C stops waiting for them; those still running are ended
    // as the set of sessions is dropped.
    let ended = async { while sessions.join_next().await.is_some() {} };
    tokio::select! {
        ended = tokio::time::timeout(SHUTDOWN_GRACE, ended) => if ended.is_err() {
            warn!("sessions did not end in time");
        },
        _ = tokio::signal::ctrl_c() => warn!("interrupted again; not waiting for the sessions"),
    }
    drop(sessions);
    if match_display == DisplayMatching::On {
        // A session that did not end in time left the monitor changed.
        let directory = Arc::clone(&state_directory);
        let restore =
            tokio::task::spawn_blocking(move || qdp_desktop::restore_monitor_settings(&directory));
        if tokio::time::timeout(SHUTDOWN_GRACE, restore).await.is_err() {
            warn!("the monitor's own settings were not restored in time");
        }
    }
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, endpoint.wait_idle()).await;
    Ok(())
}

/// Advertises the host named `name`, listening on `address` with key `pin`,
/// on the local network, and logs why if it cannot.
async fn advertise(
    name: &str,
    address: SocketAddr,
    pin: KeyPin,
    pairing: PairingMode,
) -> Option<Advertising> {
    let advertisement = Advertisement {
        name: name.to_owned(),
        address,
        record: ServiceRecord::new(
            pin.0,
            match pairing {
                PairingMode::On => PairingOffer::Offered,
                PairingMode::Off => PairingOffer::NotOffered,
            },
        ),
    };
    match qdp_desktop::advertise(advertisement).await {
        Ok(advertising) => Some(advertising),
        Err(error) => {
            warn!(%error, "not advertising on the local network; clients connect by address");
            None
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
/// when it streams the screen, the lock that `lock` asks for, and what
/// `local` shows the host's user of the client at `peer` streaming it.
async fn open_desktop(
    kind: SourceKind,
    size: (usize, usize),
    state_directory: Arc<PathBuf>,
    lock: LockPolicy,
    screen: ScreenOptions,
    local: LocalUser,
    peer: SocketAddr,
) -> Result<OpenedDesktop, RuntimeError> {
    let (source, streamed) = open_source(kind, size, state_directory, screen)
        .await
        .map_err(RuntimeError::Capture)?;
    if streamed == Streamed::Screen {
        local
            .streaming(peer)
            .await
            .map_err(|error| RuntimeError::Capture(format!("privacy was asked for: {error}")))?;
    }
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

/// How long the notice that a device streams this desktop may stay at
/// most; it is taken away when the session ends.
const SESSION_NOTICE_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// The notice, shown while a session streams this machine's screen, that
/// a device sees and controls it, so that whoever is at the machine knows
/// (Section 11).
#[derive(Debug, Clone, Default)]
struct SessionNotice(Arc<Mutex<Option<Notification>>>);

impl SessionNotice {
    /// Shows that the client at `peer` streams this desktop, if the
    /// desktop shows notifications.
    async fn show(&self, peer: SocketAddr) {
        let shown = qdp_desktop::notify(
            "This computer is being streamed",
            &format!(
                "The device at {} sees this screen and controls it. Ending the stream \
                 on the device, or stopping qdp serve, ends it.",
                peer.ip()
            ),
            SESSION_NOTICE_LIFETIME,
            Urgency::Critical,
        )
        .await;
        match shown {
            Ok(notification) => {
                *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(notification);
            }
            Err(error) => debug!(%error, "the stream is not shown as a notification"),
        }
    }

    /// Takes the notice away, once the session ended.
    async fn close(&self) {
        let shown = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(notification) = shown {
            notification.close().await;
        }
    }
}

/// What the host's user sees of a session that streams the screen (Section
/// 11.4): the notice that a device streams it, and, where `--privacy`
/// asks for it, the displays covered and the host's own input held back.
#[derive(Debug, Clone)]
struct LocalUser {
    /// The notice.
    notice: SessionNotice,
    /// Whether privacy is asked for.
    privacy: Privacy,
    /// Privacy, while on.
    covered: Arc<Mutex<Option<HostPrivacy>>>,
}

impl LocalUser {
    /// Returns what the host's user sees of a session, covered as
    /// `privacy` asks.
    fn new(privacy: Privacy) -> Self {
        Self {
            notice: SessionNotice::default(),
            privacy,
            covered: Arc::default(),
        }
    }

    /// Covers the displays if privacy is asked for, and tells the host's
    /// user that the client at `peer` streams the screen.
    ///
    /// # Errors
    ///
    /// Fails if privacy is asked for and cannot be had, so that the session
    /// does not stream an uncovered desktop.
    async fn streaming(&self, peer: SocketAddr) -> Result<(), String> {
        if self.privacy == Privacy::On {
            let covered = qdp_desktop::host_privacy()?;
            *self.covered.lock().unwrap_or_else(PoisonError::into_inner) = Some(covered);
        }
        self.notice.show(peer).await;
        Ok(())
    }

    /// Uncovers the displays and takes the notice away, once the session
    /// ended.
    async fn ended(&self) {
        *self.covered.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.notice.close().await;
    }
}

/// Locks this machine's desktop, or says why it cannot.
fn lock_desktop() {
    if let Err(error) = qdp_desktop::lock_desktop() {
        warn!(%error, "cannot lock the desktop");
    }
}

/// Returns the audio devices of a session streaming `kind`: those of the
/// desktop session shown on the console when it streams the console, as
/// a host running outside any session must reach them, and otherwise this
/// session's own.
#[cfg(target_os = "linux")]
fn host_audio(kind: SourceKind) -> HostAudio {
    if kind != SourceKind::Console {
        return qdp_desktop::host_audio();
    }
    match qdp_desktop::seat_session() {
        Some(session) => {
            info!(
                uid = session.uid,
                "streaming the sound of the console's desktop session"
            );
            qdp_desktop::host_audio_in(&session)
        }
        None => {
            info!("no one is logged in on the console; the session streams no sound");
            HostAudio::default()
        }
    }
}

/// Returns the audio devices of a session: this session's own, as only
/// Linux streams the console.
#[cfg(not(target_os = "linux"))]
fn host_audio(_kind: SourceKind) -> HostAudio {
    qdp_desktop::host_audio()
}

/// What a session streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Streamed {
    /// The screen.
    Screen,
    /// The test pattern.
    TestPattern,
}

/// Opens what a session streams: the screen, which takes the client's
/// display and delivers frames as `screen` says, the console, or a `size`
/// test pattern. Capturing the screen can wait for the user to choose a
/// monitor, and the console for a display to show a picture, so they open
/// on a blocking thread.
async fn open_source(
    kind: SourceKind,
    size: (usize, usize),
    state_directory: Arc<PathBuf>,
    screen: ScreenOptions,
) -> Result<(Box<dyn FrameSource>, Streamed), String> {
    let test_pattern = || {
        let pattern = Box::new(TestPattern::new(size.0, size.1)) as Box<dyn FrameSource>;
        (pattern, Streamed::TestPattern)
    };
    if kind == SourceKind::TestPattern {
        return Ok(test_pattern());
    }
    if kind == SourceKind::Console {
        let console = tokio::task::spawn_blocking(move || qdp_desktop::open_console(screen))
            .await
            .map_err(|error| error.to_string())??;
        return Ok((console, Streamed::Screen));
    }
    let screen =
        tokio::task::spawn_blocking(move || qdp_desktop::open_screen(&state_directory, screen))
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

/// Returns where captured screens deliver their frames: in GPU memory when
/// `gpu_capture` allows and the encoder `preference` opens reads frames
/// there, which finding out opens once, and in memory otherwise.
#[cfg(target_os = "linux")]
async fn frame_memory(gpu_capture: GpuCapture, preference: CodecPreference) -> FrameMemory {
    if gpu_capture == GpuCapture::Off {
        return FrameMemory::System;
    }
    let takes_dma_bufs =
        tokio::task::spawn_blocking(move || qdp_codec::encoder_takes_dma_bufs(preference))
            .await
            .unwrap_or(false);
    if takes_dma_bufs {
        info!("screen frames go to the encoder in GPU memory where the desktop allows");
        FrameMemory::Gpu
    } else {
        info!("screen frames go to the encoder through memory");
        FrameMemory::System
    }
}

/// Returns where captured screens deliver their frames: in memory, the only
/// place this platform's encoders read them.
#[cfg(not(target_os = "linux"))]
async fn frame_memory(_gpu_capture: GpuCapture, _preference: CodecPreference) -> FrameMemory {
    FrameMemory::System
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

/// How long the notification that a device paired stays.
const PAIRED_NOTICE_LIFETIME: Duration = Duration::from_secs(5);

/// Shows each pairing request's code as a desktop notification while the
/// code is good, so that the host's user sees it without the terminal, and
/// says when a device paired. Returns where the pairing events go.
fn pairing_notifier() -> mpsc::UnboundedSender<PairingEvent> {
    let (sender, mut events) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        // One request at a time is shown; its code ends with it.
        let mut shown: Option<Notification> = None;
        while let Some(event) = events.recv().await {
            if let Some(notification) = shown.take() {
                notification.close().await;
            }
            let (summary, body, lifetime, urgency) = match event {
                PairingEvent::Requested {
                    device_name, code, ..
                } => (
                    format!("Pair {device_name}?"),
                    format!(
                        "Type {code} on {device_name} to pair it with this computer. \
                         The code expires in {} seconds.",
                        CODE_LIFETIME.as_secs()
                    ),
                    CODE_LIFETIME,
                    Urgency::Critical,
                ),
                PairingEvent::Ended {
                    device_name,
                    outcome: PairingOutcome::Paired { .. },
                } => (
                    format!("Paired with {device_name}"),
                    "It connects without a code from now on.".to_owned(),
                    PAIRED_NOTICE_LIFETIME,
                    Urgency::Normal,
                ),
                PairingEvent::Ended { .. } => continue,
            };
            match qdp_desktop::notify(&summary, &body, lifetime, urgency).await {
                Ok(notification) if urgency == Urgency::Critical => shown = Some(notification),
                Ok(_) => {}
                Err(error) => debug!(%error, "not shown as a notification"),
            }
        }
    });
    sender
}

/// Logs a host's pairing requests, with the code to type, keeps the clients
/// that pair in `paired`, and passes the events on to `notifier`.
fn pairing_listener(
    paired: PairedFile,
    notifier: mpsc::UnboundedSender<PairingEvent>,
) -> impl Fn(PairingEvent) + Send + Sync + 'static {
    move |event| {
        let _ = notifier.send(event.clone());
        log_pairing(&paired, event);
    }
}

/// Logs a pairing event, keeping a client that paired in `paired`.
fn log_pairing(paired: &PairedFile, event: PairingEvent) {
    match event {
        PairingEvent::Requested {
            device_name,
            address,
            code,
        } => warn!(
            device = device_name,
            %address,
            "to pair this device, type {code} on it; the code expires in {} seconds",
            CODE_LIFETIME.as_secs()
        ),
        PairingEvent::Ended {
            device_name,
            outcome: PairingOutcome::Paired { client_key },
        } => {
            let client = Paired {
                pin: client_key,
                label: device_name.clone(),
            };
            match paired.add(&client) {
                Ok(()) => info!(
                    device = device_name,
                    "paired; `qdp clients remove` forgets it"
                ),
                Err(error) => warn!(%error, "paired, but it cannot be kept for the next run"),
            }
        }
        PairingEvent::Ended {
            device_name,
            outcome: PairingOutcome::Failed,
        } => warn!(
            device = device_name,
            "pairing failed: another code was typed"
        ),
        PairingEvent::Ended {
            device_name,
            outcome: PairingOutcome::Abandoned,
        } => info!(device = device_name, "pairing abandoned"),
    }
}

/// Runs `qdp clients` or `qdp hosts`: lists or forgets the devices that
/// the identity of `role` in `identity` paired with.
fn paired(identity: &Path, role: Role, action: PairedAction) -> CliResult {
    let (paired, kind) = match role {
        Role::Host => (PairedFile::clients(identity), "client"),
        Role::Client => (PairedFile::hosts(identity), "host"),
    };
    match action {
        PairedAction::List => {
            let devices = paired
                .load()
                .map_err(|error| format!("cannot read the paired {kind}s: {error}"))?;
            if devices.is_empty() {
                match role {
                    Role::Host => println!("No client has paired with this host."),
                    Role::Client => println!("This client has not paired with a host."),
                }
            }
            for device in devices {
                println!("{}  {}", format_pin(device.pin), device.label);
            }
        }
        PairedAction::Remove(which) => {
            let removed = paired
                .remove(&which)
                .map_err(|error| format!("cannot change the paired {kind}s: {error}"))?;
            if removed.is_empty() {
                return Err(format!(
                    "no paired {kind} is labelled {which} or has a pin starting with it"
                ));
            }
            for device in removed {
                match role {
                    Role::Host => println!(
                        "Forgot {}; a running host forgets it once restarted.",
                        device.label
                    ),
                    Role::Client => println!("Forgot {}.", device.label),
                }
            }
        }
    }
    Ok(())
}

/// Runs `qdp connect`: shows the host's stream until the frame limit, an
/// interrupt, the window closing, or the host closing.
fn connect(args: ConnectArgs) -> CliResult {
    let events = EventLines::new(args.events);
    let result = connect_with_sink(args);
    events.ended(&result);
    result
}

/// Connects as [`connect`] does, writing frames to the output file if one
/// is asked for.
fn connect_with_sink(args: ConnectArgs) -> CliResult {
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
    let display = match args.resolution {
        Some((width, height)) => DisplaySize {
            width,
            height,
            ..DEFAULT_DISPLAY
        },
        None => DEFAULT_DISPLAY,
    };
    let result = run_async(receive(args, display, &mut sink, interrupted(), None));
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
    let resolution = args.resolution;
    let slot = FrameSlot::default();
    let close = Arc::new(CloseSignal::default());
    let title = format!("QDP: {}", args.destination.label());
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
        resolution,
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

/// Connects to the host, directly or through a gateway, and presents its
/// stream in `sink`, shown on `display`, until the sink stops, the host
/// closes, or `stop` completes, sending the user's input from `input`, if
/// given. A connection that is lost is resumed with the host's resumption
/// token, if it gave one, while the token is valid (Section 4.4.7); through
/// a gateway, by a new tunnel, which the gateway's session token opens
/// without asking the user again. Returns the session's summary, or `None`
/// if `stop` cut it off.
async fn receive(
    args: &ConnectArgs,
    display: DisplaySize,
    sink: &mut dyn FrameSink,
    stop: impl Future<Output = ()>,
    mut input: Option<InputQueue>,
) -> Result<Option<ClientSummary>, String> {
    let pairs = args.trust == HostTrust::Pair;
    let events = EventLines::new(args.events);
    events.send(&Event::Connecting);
    let gateway = match &args.destination {
        Destination::Gateway(gateway) => Some(gateway),
        Destination::Direct(_) => None,
    };
    // Through a gateway, --user names the gateway's user, for whom the
    // gateway vouches to the host.
    let host_user = args.user.as_ref().filter(|_| gateway.is_none());
    // A token from the environment stays out of the command line, which
    // other users can read.
    let token = args.token.clone().or_else(|| {
        (!pairs && host_user.is_none())
            .then(token_from_environment)
            .flatten()
    });
    // The client presents its certificate only when it authenticates with
    // it, so that other hosts do not learn its key, and always through a
    // gateway, which binds the host's assertion to it.
    let certificate = token.is_none() && host_user.is_none();
    let identity = if pairs || certificate || gateway.is_some() {
        Some(
            identity::load_or_create(&args.identity, Role::Client)
                .map_err(|error| format!("cannot load the client identity: {error}"))?,
        )
    } else {
        None
    };
    let client_key = identity
        .as_ref()
        .map(|identity| identity.key_pin().map_err(|error| error.to_string()))
        .transpose()?;
    if let Some(pin) = client_key
        && certificate
        && !pairs
        && gateway.is_none()
    {
        info!(
            pin = format_pin(pin),
            "authenticating with this client's key; hosts accept it with --client-key {}",
            format_pin(pin)
        );
    }
    let hosts = PairedFile::hosts(&args.identity);
    let host_label = args.destination.label();
    // How the host is verified, unless a gateway knows its key.
    let mut verification = match args.trust {
        HostTrust::Pinned(pin) => Some(HostVerification::PinnedKeys(vec![pin])),
        HostTrust::Pair => Some(HostVerification::Pairing),
        HostTrust::Remembered => match hosts
            .find(&host_label)
            .map_err(|error| format!("cannot read the paired hosts: {error}"))?
        {
            Some(host) => Some(HostVerification::PinnedKeys(vec![host.pin])),
            None if gateway.is_some() => None,
            None => {
                return Err(format!(
                    "this client has not paired with {host_label}: pass --pair to pair \
                     with it, or --pin with its key pin"
                ));
            }
        },
    };
    let credentials = match (&token, host_user, client_key) {
        (Some(token), _, _) => ClientCredentials::Token(token.as_bytes().to_vec()),
        (None, Some(user), _) => ClientCredentials::Password {
            username: user.clone(),
            password: password()?,
        },
        (None, None, Some(client_key)) if pairs => ClientCredentials::Pairing {
            device_name: args.device_name.clone().unwrap_or_else(hostname),
            client_key,
        },
        (None, None, _) => ClientCredentials::Certificate,
    };
    // A direct connection's endpoint serves every attempt; a gateway's
    // tunnel, one each.
    let direct = match args.destination {
        Destination::Direct(address) => {
            let local = if address.is_ipv4() {
                SocketAddr::from(([0, 0, 0, 0], 0))
            } else {
                SocketAddr::from(([0u16; 8], 0))
            };
            let mut endpoint = ClientEndpoint::bind(local).map_err(|error| error.to_string())?;
            if let Some(identity) = identity.as_ref().filter(|_| pairs || certificate) {
                endpoint.set_identity(identity.clone_key());
            }
            Some((Arc::new(endpoint), address))
        }
        Destination::Gateway(_) => None,
    };
    // The endpoint of the attempt under way, which closes when the viewer
    // stops.
    let current = Mutex::new(direct.as_ref().map(|(endpoint, _)| Arc::clone(endpoint)));
    // An app reading the events asks its user itself.
    let sign_in = if events.enabled() {
        events.sign_in_prompt()
    } else {
        gateway::terminal_prompt(args.user.clone(), password().ok())
    };
    // The host's key, once pairing succeeded; later connections pin it.
    let paired_host = Arc::new(Mutex::new(None));
    let pairing_prompt = pairs.then(|| prompt_for_code(hosts, host_label, &paired_host, events));
    let resumption = ResumptionSlot::default();
    let sessions = async {
        let mut credentials = credentials;
        // While resuming: when the token in hand expires.
        let mut deadline = None;
        let mut delay = FIRST_RECONNECT_DELAY;
        loop {
            let attempt = match (&direct, gateway, &identity) {
                (Some((endpoint, address)), _, _) => Ok(Attempt {
                    endpoint: Arc::clone(endpoint),
                    address: *address,
                    server_name: args.server_name.clone(),
                    verification: verification.clone(),
                    credentials: credentials.clone(),
                }),
                (None, Some(gateway), Some(identity)) => {
                    let resuming = deadline.is_some();
                    through_gateway(
                        gateway,
                        identity,
                        &GatewaySessions::new(&args.identity),
                        &sign_in,
                        Attempt::assertion_replaces(&credentials, resuming),
                    )
                    .await
                    .map(|tunnel| Attempt::through(tunnel, &verification, &credentials, resuming))
                }
                _ => return Err("a gateway connection needs the client's identity".to_owned()),
            };
            let error = match attempt {
                Ok(attempt) => {
                    *current.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(Arc::clone(&attempt.endpoint));
                    let options = ClientOptions {
                        client_name: CLIENT_NAME.to_owned(),
                        credentials: attempt.credentials.clone(),
                        display,
                        decoder_threads: 0,
                        decoder: args.decoder,
                        decoder_process: decoder_process(args)?,
                        keyboard_info: qdp_desktop::keyboard_info(),
                        resumption: resumption.clone(),
                        pairing_prompt: pairing_prompt.clone(),
                    };
                    match connect_once(args, &attempt, &options, sink, input.as_mut()).await {
                        Ok(summary) => return Ok(summary),
                        Err(Failure::Fatal(error)) => return Err(error),
                        Err(Failure::Lost(error)) => error,
                    }
                }
                Err(Failure::Fatal(error)) => return Err(error),
                Err(Failure::Lost(error)) => error,
            };
            // A client that paired authenticates again with its key, and
            // pins the host's.
            let host_key = paired_host
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(host_key) = host_key {
                credentials = ClientCredentials::Certificate;
                verification = Some(HostVerification::PinnedKeys(vec![host_key]));
            }
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
            events.send(&Event::Reconnecting {
                delay,
                error: error.clone(),
            });
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(MAX_RECONNECT_DELAY);
        }
    };
    // The session is dropped, with its streams, only at the end of this
    // block, after the connection was closed.
    let summary = {
        let mut sessions = std::pin::pin!(sessions);
        tokio::select! {
            result = &mut sessions => result.map(Some)?,
            () = stop => {
                // The viewer stopped before the session ended on its own:
                // the connection closes normally first, so that the host
                // knows the client left rather than the network dropping,
                // and leaves the desktop unlocked (Section 11.4).
                if let Some(endpoint) = current.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                    endpoint.close(VIEWER_CLOSED);
                }
                None
            }
        }
    };
    let endpoint = current
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(endpoint) = endpoint {
        endpoint.wait_idle().await;
    }
    Ok(summary)
}

/// Where one attempt connects, and how it verifies and authenticates to the
/// host.
struct Attempt {
    /// The endpoint it connects from: the client's own, or a tunnel's.
    endpoint: Arc<ClientEndpoint>,
    /// The host's address, or the tunnel's.
    address: SocketAddr,
    /// The name sent in the TLS handshake.
    server_name: String,
    /// How the host is verified, if known.
    verification: Option<HostVerification>,
    /// What the client authenticates with.
    credentials: ClientCredentials,
}

impl Attempt {
    /// Returns whether a gateway's assertion takes the place of
    /// `credentials`: those of a client that would present its key alone,
    /// unless it resumes a session.
    fn assertion_replaces(credentials: &ClientCredentials, resuming: bool) -> bool {
        !resuming && *credentials == ClientCredentials::Certificate
    }

    /// Returns the attempt through `tunnel`: verifying the host by the key
    /// the gateway knows, unless the client pairs or the gateway knows none,
    /// and authenticating with the gateway's assertion where it replaces
    /// `credentials`.
    fn through(
        tunnel: Tunnel,
        verification: &Option<HostVerification>,
        credentials: &ClientCredentials,
        resuming: bool,
    ) -> Self {
        let verification = match (verification, tunnel.host_pin) {
            (Some(HostVerification::Pairing), _) => Some(HostVerification::Pairing),
            (_, Some(pin)) => Some(HostVerification::PinnedKeys(vec![pin])),
            (known, None) => known.clone(),
        };
        let credentials = match tunnel.assertion {
            Some(assertion) if Self::assertion_replaces(credentials, resuming) => {
                ClientCredentials::Token(assertion)
            }
            _ => credentials.clone(),
        };
        Self {
            endpoint: Arc::new(tunnel.endpoint),
            address: tunnel.address,
            server_name: tunnel.server_name,
            verification,
            credentials,
        }
    }
}

/// Signs in to `gateway` as the client with `identity`, with its session
/// token from `sessions` or else what `prompt` asks, keeps the token it
/// gives, and opens the tunnel to the host. Says so if the host needs the
/// gateway's assertion, `wants_assertion`, and the gateway gave none.
async fn through_gateway(
    gateway: &GatewayArgs,
    identity: &TlsIdentity,
    sessions: &GatewaySessions,
    prompt: &SignInPrompt,
    wants_assertion: bool,
) -> Result<Tunnel, Failure> {
    let verification = match gateway.pin {
        Some(pin) => HostVerification::PinnedKeys(vec![pin]),
        None => HostVerification::WebPki(Arc::new(system_roots())),
    };
    let route = GatewayRoute {
        address: gateway.address,
        server_name: gateway.name.clone(),
        verification,
        target: gateway.target.clone(),
    };
    let label = gateway.label();
    let saved = sessions.token(&label);
    let tunnel = open_tunnel(&route, identity, CLIENT_NAME, saved.as_deref(), prompt)
        .await
        .map_err(|error| match error {
            GatewayFailure::Unreachable(_) => Failure::Lost(error.to_string()),
            GatewayFailure::Refused(_) | GatewayFailure::Cancelled => {
                Failure::Fatal(error.to_string())
            }
        })?;
    info!(
        gateway = label,
        host = gateway.target,
        "opened through the gateway"
    );
    if let Some(token) = &tunnel.session_token
        && let Err(error) = sessions.keep(&label, &token.token, token.lifetime)
    {
        warn!(%error, "signed in, but the gateway's session cannot be kept for later");
    }
    if wants_assertion && tunnel.assertion.is_none() {
        // A gateway vouches only to hosts whose keys it knows (Section 13.7).
        warn!(
            host = gateway.target,
            "the gateway does not vouch for you to this host, as it does not know the host's \
             key: authenticate with --token, or pair with --pair"
        );
    }
    Ok(tunnel)
}

/// Returns the system's certificate authorities, which verify a gateway
/// given without its pin.
fn system_roots() -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    let found = rustls_native_certs::load_native_certs();
    for error in &found.errors {
        debug!(%error, "a certificate authority of the system cannot be read");
    }
    roots.add_parsable_certificates(found.certs);
    roots
}

/// Returns the prompt with which a client pairs: it asks on the terminal, or
/// the app reading `events`, for the code the host shows, and once the host
/// proved it showed it, keeps the host's key in `hosts`, labelled
/// `host_label`, and in `paired_host` for the connections that follow.
fn prompt_for_code(
    hosts: PairedFile,
    host_label: String,
    paired_host: &Arc<Mutex<Option<KeyPin>>>,
    events: EventLines,
) -> PairingPrompt {
    let paired_host = Arc::clone(paired_host);
    // Whether a code was asked for before, which the app tells the user did
    // not work.
    let asked = Arc::new(AtomicBool::new(false));
    PairingPrompt::new(
        move || {
            let asked = Arc::clone(&asked);
            Box::pin(async move {
                tokio::task::spawn_blocking(move || {
                    if !events.enabled() {
                        return read_code();
                    }
                    let request = if asked.swap(true, Ordering::Relaxed) {
                        CodeRequest::Retry
                    } else {
                        CodeRequest::First
                    };
                    events.ask_code(request)
                })
                .await
                .ok()
                .flatten()
            })
        },
        move |host_key| {
            events.send(&Event::Paired(host_key));
            let host = Paired {
                pin: host_key,
                label: host_label.clone(),
            };
            match hosts.add(&host) {
                Ok(()) => info!(
                    host = host_label,
                    "paired; later connections to the host need no --pin or credentials"
                ),
                Err(error) => warn!(%error, "paired, but the host cannot be kept for later"),
            }
            *paired_host.lock().unwrap_or_else(PoisonError::into_inner) = Some(host_key);
        },
    )
}

/// Asks on the terminal for the code the host shows, or returns `None` if
/// the terminal closed.
fn read_code() -> Option<String> {
    eprint!("Type the four-digit code the host shows: ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    match std::io::stdin().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_owned()),
    }
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

/// Connects to the host once, as `attempt` says, and runs a session with
/// `options` until it ends.
async fn connect_once(
    args: &ConnectArgs,
    attempt: &Attempt,
    options: &ClientOptions,
    sink: &mut dyn FrameSink,
    input: Option<&mut InputQueue>,
) -> Result<ClientSummary, Failure> {
    let host = args.destination.label();
    let verification = attempt.verification.clone().ok_or_else(|| {
        Failure::Fatal(format!(
            "the gateway does not know the key of {host}: pass --pin with it, or --pair"
        ))
    })?;
    let connection = attempt
        .endpoint
        .connect(attempt.address, &attempt.server_name, verification)
        .await
        .map_err(|error| Failure::Lost(format!("cannot connect to {host}: {error}")))?;
    info!(host, "connected");
    EventLines::new(args.events).send(&Event::Connected);
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
