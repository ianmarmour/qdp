//! A session with a host, run on a thread of its own while the app shows it.
//!
//! The session reconnects as the desktop client does: a connection that is
//! lost is resumed with the host's resumption token while the token is valid
//! (Section 4.4.7). A session that pairs (Section 4.4.9) asks the app for the
//! code the host shows, and again, on a new connection, when a code did not
//! work; once paired it authenticates with the device's key.
//!
//! A session through a gateway (Section 13) opens a tunnel to the host for
//! each connection, signing in with the session token the app kept from an
//! earlier sign-in, or else asking the app for the user's password and a
//! code from their authenticator app; the gateway's new token goes to the
//! app to keep.

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use qdp::message::KeyboardInfo;
use qdp_codec::CodecPreference;
use qdp_quic::{ClientEndpoint, HostVerification, IdentityFiles, KeyPin, TlsIdentity};
use qdp_runtime::{
    ClientAudio, ClientCredentials, ClientOptions, DisplaySize, GatewayFailure, GatewayRoute,
    InputHandle, InputQueue, Microphone, PairingPrompt, ResumptionSlot, RuntimeError,
    SharedClipboard, SignInAnswer, SignInPrompt, UserInput, gateway_verification, input_channel,
    open_tunnel,
};
use tokio::sync::{Notify, oneshot};
use tracing::{info, warn};

use qdp::message::CursorImage;

use crate::events::{CodeRequest, Ending, Event, GatewayToken};
use crate::settings::{SERVER_NAME, gateway_server_name, resolve_address, resolve_address_on};
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

/// Why a session through a gateway failed when the gateway gave no
/// assertion for the host.
const NOT_VOUCHED: &str = "the gateway does not vouch for you to this computer, as it does not \
    know its key; ask its administrator to list it on the gateway";

/// Why a session failed when neither the app nor the gateway knew the
/// host's key.
const UNKNOWN_HOST_KEY: &str = "the gateway does not know this computer's key, so it cannot be \
    verified; add it with its key pin, or ask the gateway's administrator to list it";

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
    /// Through a gateway, which vouches for the user to the host with an
    /// identity assertion and knows the host's key (Section 13.7).
    Gateway,
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
            Self::Gateway => formatter.write_str("Gateway"),
        }
    }
}

/// A gateway a session reaches its host through (Section 13).
#[derive(Clone, PartialEq, Eq)]
pub struct GatewayConfig {
    /// Its address: a name or IP address, with an optional port (443 by
    /// default).
    pub address: String,
    /// Its key pin, if the user set one; otherwise the system's certificate
    /// authorities verify it.
    pub pin: Option<KeyPin>,
    /// The session token the app kept from an earlier sign-in, if any.
    pub session_token: Option<Vec<u8>>,
}

/// Hides the session token.
impl fmt::Debug for GatewayConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GatewayConfig")
            .field("address", &self.address)
            .field("pin", &self.pin)
            .finish_non_exhaustive()
    }
}

/// What a session connects with.
#[derive(Debug, Clone)]
pub struct Config {
    /// The product name the client sends in `CLIENT_HELLO`, such as
    /// `qdp-android/0.1.0`.
    pub client_name: String,
    /// The host's address or name, with or without a port; through a
    /// gateway, its ID there or its name inside the network.
    pub host: String,
    /// The gateway the host is reached through, if any.
    pub gateway: Option<GatewayConfig>,
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
    /// Opens the clipboard shared with the host for each connection, where
    /// the app shares one (Section 10).
    pub clipboard: Option<fn() -> Option<SharedClipboard>>,
    /// Returns the keyboard layout the host is told of, where the app knows
    /// it, so that the host types the same characters (Section 8.4.1).
    pub keyboard_info: Option<fn() -> Option<KeyboardInfo>>,
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
    /// Where the answer to a gateway's sign-in goes, while it is asked for.
    sign_in: SignInSlot,
    /// The session's thread, until it has been joined.
    thread: Option<JoinHandle<()>>,
}

/// Where the user's answer to a request for a pairing code goes: the code,
/// or `None` if they declined.
type CodeSlot = Arc<Mutex<Option<oneshot::Sender<Option<String>>>>>;

/// Where the user's answer to a gateway's sign-in goes: their credentials or
/// code, or `None` if they declined.
type SignInSlot = Arc<Mutex<Option<oneshot::Sender<Option<SignInAnswer>>>>>;

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
        let sign_in = SignInSlot::default();
        let thread = std::thread::Builder::new()
            .name("qdp-session".to_owned())
            .spawn({
                let cursor = Arc::clone(&cursor);
                let stop = Arc::clone(&stop);
                let prompts = Prompts {
                    code: Arc::clone(&code),
                    sign_in: Arc::clone(&sign_in),
                };
                move || run_thread(&config, presenter, queue, sender, cursor, &stop, prompts)
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
            sign_in,
            thread: Some(thread),
        })
    }

    /// Answers the gateway's request for the user's credentials or code
    /// with `answer`, or declines with `None`. Does nothing while nothing
    /// is asked for.
    pub fn answer_sign_in(&self, answer: Option<SignInAnswer>) {
        let sender = self
            .sign_in
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(answer);
        }
    }

    /// Returns a handle that sends the user's input to the session from any
    /// thread, as [`Session::send`] does, for capture that runs on threads
    /// of its own, such as gamepads'.
    pub fn input(&self) -> InputHandle {
        self.input.clone()
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

    /// Returns the host's cursor as the session keeps it, for a presenter
    /// that draws it.
    pub fn shared_cursor(&self) -> SharedCursor {
        Arc::clone(&self.cursor)
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

/// Where the app's answers to what a session asks go.
#[derive(Debug, Clone)]
struct Prompts {
    /// A pairing code.
    code: CodeSlot,
    /// A gateway's sign-in.
    sign_in: SignInSlot,
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
    prompts: Prompts,
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
                prompts,
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

/// Connects to the host, directly or through a gateway, and shows its stream
/// in `sink`, sending the user's input from `input`, until the host closes,
/// the session fails, or `stop` completes. The app's answers to what the
/// session asks come through `prompts`.
async fn receive<P: Presenter>(
    config: &Config,
    sink: &mut AppSink<P>,
    stop: impl Future<Output = ()>,
    mut input: InputQueue,
    events: &Sender<Event>,
    prompts: Prompts,
) -> Ending {
    // The device presents its key to every host, so that one that paired
    // with it, or was given its pin, recognizes it, and to the gateway,
    // which binds its assertions to it.
    let directory = config.identity_directory.clone();
    let identity = tokio::task::spawn_blocking(move || {
        TlsIdentity::load_or_generate(&directory, IdentityFiles::CLIENT)
    })
    .await;
    let identity = match identity {
        Ok(Ok((identity, _))) => identity,
        Ok(Err(error)) => return Ending::Failed(format!("cannot keep the device's key: {error}")),
        Err(error) => return Ending::Failed(format!("cannot load the device's key: {error}")),
    };
    let client_key = match identity.key_pin() {
        Ok(pin) => pin,
        Err(error) => return Ending::Failed(format!("the device's key is unusable: {error}")),
    };
    let route = match &config.gateway {
        Some(gateway) => match gateway_route(gateway, &config.host).await {
            Ok(route) => Route::Gateway(route),
            Err(error) => return Ending::Failed(error),
        },
        None => match direct_route(&config.host, &identity).await {
            Ok(route) => route,
            Err(error) => return Ending::Failed(error),
        },
    };
    // The endpoint of the connection under way, which closes when the user
    // ends the session.
    let current = Mutex::new(match &route {
        Route::Direct { endpoint, .. } => Some(Arc::clone(endpoint)),
        Route::Gateway(_) => None,
    });
    let sign_in = sign_in_prompt(&prompts.sign_in, events);
    let resumption = ResumptionSlot::default();
    let pairing = Pairing::default();
    let sessions = async {
        let (mut credentials, mut verification) = match &config.authentication {
            Authentication::Token { pin, token } => (
                ClientCredentials::Token(token.as_bytes().to_vec()),
                Some(HostVerification::PinnedKeys(vec![*pin])),
            ),
            Authentication::Paired { pin } => (
                ClientCredentials::Certificate,
                Some(HostVerification::PinnedKeys(vec![*pin])),
            ),
            Authentication::Pair { device_name } => (
                ClientCredentials::Pairing {
                    device_name: device_name.clone(),
                    client_key,
                },
                Some(HostVerification::Pairing),
            ),
            // The gateway's assertion and the host's pin it knows take
            // their places for each connection.
            Authentication::Gateway => (ClientCredentials::Certificate, None),
        };
        let mut session_token = config
            .gateway
            .as_ref()
            .and_then(|gateway| gateway.session_token.clone());
        // While resuming: when the token in hand expires.
        let mut deadline = None;
        let mut delay = FIRST_RECONNECT_DELAY;
        loop {
            let pairs = matches!(credentials, ClientCredentials::Pairing { .. });
            pairing.asked.store(false, Ordering::Relaxed);
            let attempt = match &route {
                Route::Direct { endpoint, address } => Ok(Attempt {
                    endpoint: Arc::clone(endpoint),
                    address: *address,
                    server_name: SERVER_NAME.to_owned(),
                    verification: verification.clone(),
                    credentials: credentials.clone(),
                }),
                Route::Gateway(gateway) => {
                    let tunnel = open_tunnel(
                        gateway,
                        &identity,
                        &config.client_name,
                        session_token.as_deref(),
                        &sign_in,
                    )
                    .await;
                    match tunnel {
                        Ok(tunnel) => {
                            if let Some(token) = &tunnel.session_token {
                                session_token = Some(token.token.clone());
                                let _ = events.send(Event::GatewaySession {
                                    token: GatewayToken(token.token.clone()),
                                    lifetime: token.lifetime,
                                });
                            }
                            let host_verification = match (&verification, tunnel.host_pin) {
                                (Some(HostVerification::Pairing), _) => {
                                    Some(HostVerification::Pairing)
                                }
                                (_, Some(pin)) => Some(HostVerification::PinnedKeys(vec![pin])),
                                (known, None) => known.clone(),
                            };
                            // The assertion vouches for the user, unless
                            // the session resumes with the host's token.
                            let vouched = config.authentication == Authentication::Gateway
                                && credentials == ClientCredentials::Certificate;
                            let attempt_credentials = match tunnel.assertion {
                                Some(assertion) if vouched => ClientCredentials::Token(assertion),
                                None if vouched => {
                                    return Ending::Failed(NOT_VOUCHED.to_owned());
                                }
                                _ => credentials.clone(),
                            };
                            Ok(Attempt {
                                endpoint: Arc::new(tunnel.endpoint),
                                address: tunnel.address,
                                server_name: tunnel.server_name,
                                verification: host_verification,
                                credentials: attempt_credentials,
                            })
                        }
                        Err(GatewayFailure::Cancelled) => return Ending::Normal,
                        Err(GatewayFailure::Refused(error)) => return Ending::Failed(error),
                        Err(GatewayFailure::Unreachable(error)) => {
                            Err(format!("cannot reach the gateway: {error}"))
                        }
                    }
                }
            };
            let result = match attempt {
                Ok(attempt) => {
                    let Some(host_verification) = attempt.verification.clone() else {
                        return Ending::Failed(UNKNOWN_HOST_KEY.to_owned());
                    };
                    *current.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(Arc::clone(&attempt.endpoint));
                    let options = ClientOptions {
                        client_name: config.client_name.clone(),
                        credentials: attempt.credentials.clone(),
                        display: config.display,
                        decoder_threads: 0,
                        decoder: config.decoder,
                        decoder_process: None,
                        keyboard_info: config
                            .keyboard_info
                            .and_then(|keyboard_info| keyboard_info()),
                        resumption: resumption.clone(),
                        pairing_prompt: pairs.then(|| pairing.prompt(&prompts.code, events)),
                    };
                    connect_once(
                        attempt.address,
                        &attempt.server_name,
                        &host_verification,
                        (config.client_audio)(config.microphone),
                        config.clipboard.and_then(|open| open()),
                        &attempt.endpoint,
                        &options,
                        sink,
                        &mut input,
                        events,
                    )
                    .await
                }
                Err(error) => Err(Failure::Lost(error)),
            };
            // Once paired, the session goes on with the device's key, which
            // the host now accepts.
            if let Some(host_key) = pairing.host() {
                credentials = ClientCredentials::Certificate;
                verification = Some(HostVerification::PinnedKeys(vec![host_key]));
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
                if let Some(endpoint) = current.lock().unwrap_or_else(PoisonError::into_inner).as_ref() {
                    endpoint.close(VIEWER_CLOSED);
                }
                Ending::Normal
            }
        }
    };
    let endpoint = current
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(endpoint) = endpoint
        && tokio::time::timeout(CLOSE_TIMEOUT, endpoint.wait_idle())
            .await
            .is_err()
    {
        warn!("the connection did not close in time");
    }
    ending
}

/// How a session reaches its host.
#[derive(Debug)]
enum Route {
    /// Directly, from this endpoint, at this address.
    Direct {
        /// The endpoint every connection is made from.
        endpoint: Arc<ClientEndpoint>,
        /// The host's address.
        address: SocketAddr,
    },
    /// Through a gateway, a tunnel for each connection.
    Gateway(GatewayRoute),
}

/// Where one connection is made, and how it verifies and authenticates to
/// the host.
#[derive(Debug)]
struct Attempt {
    /// The endpoint it is made from: the session's own, or a tunnel's.
    endpoint: Arc<ClientEndpoint>,
    /// The host's address, or the tunnel's.
    address: SocketAddr,
    /// The name sent in the TLS handshake.
    server_name: String,
    /// How the host is verified, if its key is known.
    verification: Option<HostVerification>,
    /// What the client authenticates with.
    credentials: ClientCredentials,
}

/// Returns the route to `host` directly: its address, resolved, and an
/// endpoint presenting `identity`.
async fn direct_route(host: &str, identity: &TlsIdentity) -> Result<Route, String> {
    let host = host.to_owned();
    let address = tokio::task::spawn_blocking(move || resolve_address(&host))
        .await
        .map_err(|error| format!("cannot resolve the host: {error}"))??;
    let local = if address.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    };
    let mut endpoint =
        ClientEndpoint::bind(local).map_err(|error| format!("cannot open a socket: {error}"))?;
    endpoint.set_identity(identity.clone_key());
    Ok(Route::Direct {
        endpoint: Arc::new(endpoint),
        address,
    })
}

/// Returns the route to `target` through `gateway`: the gateway's address,
/// resolved, its name and how it is verified.
async fn gateway_route(gateway: &GatewayConfig, target: &str) -> Result<GatewayRoute, String> {
    let text = gateway.address.clone();
    let address = tokio::task::spawn_blocking(move || {
        resolve_address_on(&text, qdp::gateway::DEFAULT_GATEWAY_PORT)
    })
    .await
    .map_err(|error| format!("cannot resolve the gateway: {error}"))??;
    Ok(GatewayRoute {
        address,
        server_name: gateway_server_name(&gateway.address),
        verification: gateway_verification(gateway.pin).map_err(|error| error.to_string())?,
        target: target.trim().to_owned(),
    })
}

/// Returns the prompt that asks the app what a gateway's sign-in needs
/// through `events`, the answer coming through `slot`.
fn sign_in_prompt(slot: &SignInSlot, events: &Sender<Event>) -> SignInPrompt {
    let (slot, events) = (Arc::clone(slot), events.clone());
    SignInPrompt::new(move |request| {
        let (sender, receiver) = oneshot::channel();
        *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        let _ = events.send(Event::sign_in(request));
        Box::pin(async move { receiver.await.ok().flatten() })
    })
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
/// and runs a session with `options`, sharing `clipboard` if there is one,
/// until it ends.
#[allow(clippy::too_many_arguments)]
async fn connect_once<P: Presenter>(
    address: SocketAddr,
    server_name: &str,
    verification: &HostVerification,
    audio: ClientAudio,
    clipboard: Option<SharedClipboard>,
    endpoint: &ClientEndpoint,
    options: &ClientOptions,
    sink: &mut AppSink<P>,
    input: &mut InputQueue,
    events: &Sender<Event>,
) -> Result<(), Failure> {
    let _ = events.send(Event::Connecting);
    let connection = endpoint
        .connect(address, server_name, verification.clone())
        .await
        .map_err(|error| Failure::Lost(format!("cannot connect to {address}: {error}")))?;
    info!(%address, "connected");
    let _ = events.send(Event::Connected);
    let summary = qdp_runtime::run(connection, options, sink, Some(input), clipboard, audio)
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
