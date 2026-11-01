//! A session with a host, run on a thread of its own while the app shows it.
//!
//! The session reconnects as the desktop client does: a connection that is
//! lost is resumed with the host's resumption token while the token is valid
//! (Section 4.4.7). A session that pairs (Section 4.4.9) asks the app for the
//! code the host shows, and again, on a new connection, when a code did not
//! work; once paired it authenticates with the device's key.

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use qdp_codec::CodecPreference;
use qdp_quic::{ClientEndpoint, HostVerification, IdentityFiles, KeyPin, TlsIdentity};
use qdp_runtime::{
    ClientAudio, ClientCredentials, ClientOptions, DisplaySize, InputHandle, InputQueue,
    Microphone, PairingPrompt, ResumptionSlot, RuntimeError, UserInput, input_channel,
};
use tokio::sync::{Notify, oneshot};
use tracing::{info, warn};

use qdp::message::CursorImage;

use crate::events::{CodeRequest, Ending, Event};
use crate::settings::{SERVER_NAME, resolve_address};
use crate::sink::{AppSink, Presenter, SharedCursor};


/// The wait before reconnecting the first time after a connection is lost.
const FIRST_RECONNECT_DELAY: Duration = Duration::from_millis(500);

/// The longest wait between attempts to reconnect.
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(8);

/// The reason the client gives the host when the user ends the session.
const VIEWER_CLOSED: &str = "viewer closed";

/// How long a session that the user ended waits for its connection to
/// close, which the app waits for before its surface goes.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

/// How often a wait for the app's next event looks for gamepad rumble.
const FEEDBACK_POLL: Duration = Duration::from_millis(10);

/// Why a session that pairs failed when the host refused before showing a
/// code.
const PAIRING_REFUSED: &str = "the host is not pairing now: it pairs with one device at a time, \
    and waits a few minutes after wrong codes";

/// How a session authenticates with the host and recognizes its key.
#[derive(Clone, PartialEq, Eq)]
pub enum Authentication {
    /// The host's key is pinned, and the device presents a token.
    Token {
        /// The host's key pin.
        pin: KeyPin,
        /// The token the host accepts.
        token: String,
    },
    /// A host the device paired with: its key is pinned, and the device
    /// presents its own key (Section 4.4.2).
    Paired {
        /// The host's key pin.
        pin: KeyPin,
    },
    /// Pairing with the code the host shows (Section 4.4.9), after which
    /// the session goes on as with a paired host.
    Pair {
        /// The device's name, which the host shows with the code.
        device_name: String,
    },
}

/// Hides the token.
impl fmt::Debug for Authentication {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Token { pin, .. } => formatter
                .debug_struct("Token")
                .field("pin", pin)
                .finish_non_exhaustive(),
            Self::Paired { pin } => formatter.debug_struct("Paired").field("pin", pin).finish(),
            Self::Pair { device_name } => formatter
                .debug_struct("Pair")
                .field("device_name", device_name)
                .finish(),
        }
    }
}

/// What a session connects with.
#[derive(Debug, Clone)]
pub struct Config {
    /// The product name the client sends in `CLIENT_HELLO`, such as
    /// `qdp-android/0.1.0`.
    pub client_name: String,
    /// The host's address or name, with or without a port.
    pub host: String,
    /// How the session authenticates.
    pub authentication: Authentication,
    /// The directory the device's key is kept in.
    pub identity_directory: PathBuf,
    /// The display the video is shown on.
    pub display: DisplaySize,
    /// Whether to decode in hardware or software.
    pub decoder: CodecPreference,
    /// Whether the microphone starts on.
    pub microphone: Microphone,
    /// Opens the device's speakers and microphone, the microphone capturing
    /// only while the state given says it is on.
    pub client_audio: fn(Microphone) -> ClientAudio,
}

/// A running session.
#[derive(Debug)]
pub struct Session {
    /// Sends the user's input.
    input: InputHandle,
    /// Reads the session's events.
    events: EventReader,
    /// The host's cursor.
    cursor: SharedCursor,
    /// Asks the session to end.
    stop: Arc<Notify>,
    /// Where the code the app is asked for goes, while it is asked for.
    code: CodeSlot,
    /// The session's thread, until it has been joined.
    thread: Option<JoinHandle<()>>,
}

/// Where the user's answer to a request for a pairing code goes: the code,
/// or `None` if they declined.
type CodeSlot = Arc<Mutex<Option<oneshot::Sender<Option<String>>>>>;

impl Session {
    /// Starts a session with `config` on a new thread, showing its video
    /// with the presenter `presenter` makes there.
    pub fn start<P, F>(config: Config, presenter: F) -> std::io::Result<Self>
    where
        P: Presenter,
        F: FnOnce() -> P + Send + 'static,
    {
        let (input, queue) = input_channel();
        let (sender, receiver) = mpsc::channel();
        let cursor = SharedCursor::default();
        let stop = Arc::new(Notify::new());
        let code = CodeSlot::default();
        let thread = std::thread::Builder::new()
            .name("qdp-session".to_owned())
            .spawn({
                let cursor = Arc::clone(&cursor);
                let stop = Arc::clone(&stop);
                let code = Arc::clone(&code);
                move || run_thread(&config, presenter, queue, sender, cursor, &stop, code)
            })?;
        Ok(Self {
            events: EventReader {
                events: Arc::new(Mutex::new(receiver)),
                input: input.clone(),
            },
            input,
            cursor,
            stop,
            code,
            thread: Some(thread),
        })
    }

    /// Answers the session's request for the pairing code with `code`, or
    /// declines with `None`. Does nothing while no code is asked for.
    pub fn answer_pairing(&self, code: Option<String>) {
        let sender = self
            .code
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(code);
        }
    }

    /// Sends one of the user's inputs.
    pub fn send(&self, input: UserInput) {
        self.input.send(input);
    }

    /// Returns a reader of the session's events, for a thread that waits
    /// for them.
    pub fn events(&self) -> EventReader {
        self.events.clone()
    }

    /// Returns the host's cursor image, or `None` while it is hidden.
    pub fn cursor(&self) -> Option<CursorImage> {
        self.cursor
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Ends the session, returning once its thread has finished.
    pub fn stop(mut self) {
        self.finish();
    }

    /// Asks the session to end and waits for its thread.
    fn finish(&mut self) {
        self.stop.notify_one();
        let Some(thread) = self.thread.take() else {
            return;
        };
        if thread.join().is_err() {
            warn!("the session thread panicked");
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Reads a session's events.
#[derive(Debug, Clone)]
pub struct EventReader {
    /// The events, which one thread at a time waits for.
    events: Arc<Mutex<Receiver<Event>>>,
    /// The session's input, whose gamepad feedback becomes rumble events.
    input: InputHandle,
}

impl EventReader {
    /// Returns the next event, waiting up to `timeout` for one, or `None` if
    /// none came. Once the session has ended and its events were all read,
    /// it waits out `timeout` too, so that a caller polling in a loop does
    /// not spin.
    pub fn next(&self, timeout: Duration) -> Option<Event> {
        let events = self.events.lock().unwrap_or_else(PoisonError::into_inner);
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(feedback) = self.input.try_gamepad_feedback() {
                return Some(Event::Rumble {
                    slot: feedback.slot,
                    low: feedback.output.low_frequency_rumble,
                    high: feedback.output.high_frequency_rumble,
                });
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match events.recv_timeout(left.min(FEEDBACK_POLL)) {
                Ok(event) => return Some(event),
                Err(RecvTimeoutError::Timeout) if left > FEEDBACK_POLL => {}
                Err(RecvTimeoutError::Timeout) => return None,
                Err(RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(left);
                    return None;
                }
            }
        }
    }
}

/// Runs a session with `config` on the session's thread, telling the app
/// through `events` how it went, until it ends or `stop` is notified.
fn run_thread<P: Presenter>(
    config: &Config,
    presenter: impl FnOnce() -> P,
    input: InputQueue,
    events: Sender<Event>,
    cursor: SharedCursor,
    stop: &Notify,
    code: CodeSlot,
) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("qdp-runtime")
        .build();
    let ending = match runtime {
        Ok(runtime) => {
            let mut sink = AppSink::new(presenter(), events.clone(), cursor);
            runtime.block_on(receive(
                config,
                &mut sink,
                stop.notified(),
                input,
                &events,
                code,
            ))
        }
        Err(error) => Ending::Failed(format!("cannot start the session's runtime: {error}")),
    };
    match &ending {
        Ending::Normal => info!("session ended"),
        Ending::Failed(error) => warn!(%error, "session failed"),
    }
    let _ = events.send(Event::Ended(ending));
}

/// Connects to the host and shows its stream in `sink`, sending the user's
/// input from `input`, until the host closes, the session fails, or `stop`
/// completes. A pairing code the app is asked for comes through `code`.
async fn receive<P: Presenter>(
    config: &Config,
    sink: &mut AppSink<P>,
    stop: impl Future<Output = ()>,
    mut input: InputQueue,
    events: &Sender<Event>,
    code: CodeSlot,
) -> Ending {
    let host = config.host.clone();
    let address = match tokio::task::spawn_blocking(move || resolve_address(&host)).await {
        Ok(Ok(address)) => address,
        Ok(Err(error)) => return Ending::Failed(error),
        Err(error) => return Ending::Failed(format!("cannot resolve the host: {error}")),
    };
    let local = if address.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    };
    let mut endpoint = match ClientEndpoint::bind(local) {
        Ok(endpoint) => endpoint,
        Err(error) => return Ending::Failed(format!("cannot open a socket: {error}")),
    };
    // The device presents its key to every host, so that one that paired
    // with it, or was given its pin, recognizes it.
    let directory = config.identity_directory.clone();
    let identity = tokio::task::spawn_blocking(move || {
        TlsIdentity::load_or_generate(&directory, IdentityFiles::CLIENT)
    })
    .await;
    let client_key = match identity {
        Ok(Ok((identity, _))) => match identity.key_pin() {
            Ok(pin) => {
                endpoint.set_identity(identity);
                pin
            }
            Err(error) => return Ending::Failed(format!("the device's key is unusable: {error}")),
        },
        Ok(Err(error)) => return Ending::Failed(format!("cannot keep the device's key: {error}")),
        Err(error) => return Ending::Failed(format!("cannot load the device's key: {error}")),
    };
    let resumption = ResumptionSlot::default();
    let pairing = Pairing::default();
    let sessions = async {
        let (mut credentials, mut verification) = match &config.authentication {
            Authentication::Token { pin, token } => (
                ClientCredentials::Token(token.as_bytes().to_vec()),
                HostVerification::PinnedKeys(vec![*pin]),
            ),
            Authentication::Paired { pin } => (
                ClientCredentials::Certificate,
                HostVerification::PinnedKeys(vec![*pin]),
            ),
            Authentication::Pair { device_name } => (
                ClientCredentials::Pairing {
                    device_name: device_name.clone(),
                    client_key,
                },
                HostVerification::Pairing,
            ),
        };
        // While resuming: when the token in hand expires.
        let mut deadline = None;
        let mut delay = FIRST_RECONNECT_DELAY;
        loop {
            let pairs = matches!(credentials, ClientCredentials::Pairing { .. });
            pairing.asked.store(false, Ordering::Relaxed);
            let options = ClientOptions {
                client_name: config.client_name.clone(),
                credentials: credentials.clone(),
                display: config.display,
                decoder_threads: 0,
                decoder: config.decoder,
                decoder_process: None,
                keyboard_info: None,
                resumption: resumption.clone(),
                pairing_prompt: pairs.then(|| pairing.prompt(&code, events)),
            };
            let attempt = connect_once(
                address,
                &verification,
                (config.client_audio)(config.microphone),
                &endpoint,
                &options,
                sink,
                &mut input,
                events,
            );
            let result = attempt.await;
            // Once paired, the session goes on with the device's key, which
            // the host now accepts.
            if let Some(host_key) = pairing.host() {
                credentials = ClientCredentials::Certificate;
                verification = HostVerification::PinnedKeys(vec![host_key]);
            }
            let error = match result {
                Ok(()) => return Ending::Normal,
                Err(Failure::Cancelled) => return Ending::Normal,
                Err(Failure::Rejected(error)) if pairs && pairing.host().is_none() => {
                    if !pairing.asked.load(Ordering::Relaxed) {
                        return Ending::Failed(PAIRING_REFUSED.to_owned());
                    }
                    // The code did not work: the host shows a new one for
                    // the next connection.
                    info!(%error, "pairing failed; asking for the new code");
                    pairing.wrong_code.store(true, Ordering::Relaxed);
                    continue;
                }
                Err(Failure::Rejected(error) | Failure::Fatal(error)) => {
                    return Ending::Failed(error);
                }
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
                return Ending::Failed(error);
            }
            warn!(%error, retry_in = ?delay, "connection lost; reconnecting");
            let _ = events.send(Event::Reconnecting { delay, error });
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(MAX_RECONNECT_DELAY);
        }
    };
    // The session is dropped, with its streams, only at the end of this
    // block, after the connection was closed.
    let ending = {
        let mut sessions = std::pin::pin!(sessions);
        tokio::select! {
            ending = &mut sessions => ending,
            () = stop => {
                // The user ended the session: the connection closes normally
                // first, so that the host knows the client left rather than
                // the network dropping (Section 11.4).
                endpoint.close(VIEWER_CLOSED);
                Ending::Normal
            }
        }
    };
    if tokio::time::timeout(CLOSE_TIMEOUT, endpoint.wait_idle())
        .await
        .is_err()
    {
        warn!("the connection did not close in time");
    }
    ending
}

/// A session's pairing: whether the app was asked for a code on the
/// current connection, whether the last code did not work, and the host's
/// key once paired.
#[derive(Debug, Clone, Default)]
struct Pairing {
    /// Whether the current connection asked for a code.
    asked: Arc<AtomicBool>,
    /// Whether the last code did not work.
    wrong_code: Arc<AtomicBool>,
    /// The host's key, once paired.
    host: Arc<Mutex<Option<KeyPin>>>,
}

impl Pairing {
    /// Returns the host's key, once paired.
    fn host(&self) -> Option<KeyPin> {
        *self.host.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns the prompt that asks the app for the code through `events`,
    /// the answer coming through `code`, and tells it the host's key once
    /// paired.
    fn prompt(&self, code: &CodeSlot, events: &Sender<Event>) -> PairingPrompt {
        let ask = {
            let pairing = self.clone();
            let code = Arc::clone(code);
            let events = events.clone();
            move || {
                pairing.asked.store(true, Ordering::Relaxed);
                let request = if pairing.wrong_code.load(Ordering::Relaxed) {
                    CodeRequest::Retry
                } else {
                    CodeRequest::First
                };
                let (sender, receiver) = oneshot::channel();
                *code.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
                let _ = events.send(Event::PairingCode(request));
                let answer: qdp_runtime::CodeFuture =
                    Box::pin(async move { receiver.await.ok().flatten() });
                answer
            }
        };
        let paired = {
            let host = Arc::clone(&self.host);
            let events = events.clone();
            move |host_key| {
                info!(?host_key, "paired");
                *host.lock().unwrap_or_else(PoisonError::into_inner) = Some(host_key);
                let _ = events.send(Event::Paired(host_key));
            }
        };
        PairingPrompt::new(ask, paired)
    }
}

/// Why a connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// The connection could not be made, or was lost: reconnecting may help.
    Lost(String),
    /// The host did not accept the client's credentials, or its code.
    Rejected(String),
    /// The user declined to type the pairing code.
    Cancelled,
    /// The session failed for another reason, which ends it.
    Fatal(String),
}

/// Connects to the host once, verifying its key as `verification` says,
/// and runs a session with `options` until it ends.
#[allow(clippy::too_many_arguments)]
async fn connect_once<P: Presenter>(
    address: SocketAddr,
    verification: &HostVerification,
    audio: ClientAudio,
    endpoint: &ClientEndpoint,
    options: &ClientOptions,
    sink: &mut AppSink<P>,
    input: &mut InputQueue,
    events: &Sender<Event>,
) -> Result<(), Failure> {
    let _ = events.send(Event::Connecting);
    let connection = endpoint
        .connect(address, SERVER_NAME, verification.clone())
        .await
        .map_err(|error| Failure::Lost(format!("cannot connect to {address}: {error}")))?;
    info!(%address, "connected");
    let _ = events.send(Event::Connected);
    let summary = qdp_runtime::run(
        connection,
        options,
        sink,
        Some(input),
        None,
        audio,
    )
    .await
    .map_err(|error| {
        let message = format!("session failed: {error}");
        match error {
            RuntimeError::PairingCancelled => Failure::Cancelled,
            RuntimeError::AuthenticationFailed(_) => Failure::Rejected(message),
            error if error.is_connection_lost() => Failure::Lost(message),
            _ => Failure::Fatal(message),
        }
    })?;
    info!(
        frames = summary.frames_presented,
        decode_failures = summary.decode_failures,
        "disconnected"
    );
    Ok(())
}
