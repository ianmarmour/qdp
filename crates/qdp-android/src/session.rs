//! A session with a host, run on a thread of its own while the app shows it.
//!
//! The session reconnects as the desktop client does: a connection that is
//! lost is resumed with the host's resumption token while the token is valid
//! (Section 4.4.7).

use std::future::Future;
use std::net::SocketAddr;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use qdp_codec::CodecPreference;
use qdp_quic::{ClientEndpoint, HostVerification, KeyPin};
use qdp_runtime::{
    ClientAudio, ClientCredentials, ClientOptions, DisplaySize, InputHandle, InputQueue,
    Microphone, ResumptionSlot, UserInput, input_channel,
};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::cursor::CursorBitmap;
use crate::events::{Ending, Event};
use crate::settings::{SERVER_NAME, resolve_address};
use crate::sink::{AndroidSink, SharedCursor};

/// The product name the client sends in `CLIENT_HELLO`.
const CLIENT_NAME: &str = concat!("qdp-android/", env!("CARGO_PKG_VERSION"));

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

/// What a session connects with.
#[derive(Debug, Clone)]
pub struct Config {
    /// The host's address or name, with or without a port.
    pub host: String,
    /// The host's key pin.
    pub pin: KeyPin,
    /// The token the host accepts.
    pub token: String,
    /// The display the video is shown on.
    pub display: DisplaySize,
    /// Whether to decode in hardware or software.
    pub decoder: CodecPreference,
    /// Whether the microphone starts on.
    pub microphone: Microphone,
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
    /// The session's thread, until it has been joined.
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Starts a session with `config` on a new thread.
    pub fn start(config: Config) -> std::io::Result<Self> {
        let (input, queue) = input_channel();
        let (sender, receiver) = mpsc::channel();
        let cursor = SharedCursor::default();
        let stop = Arc::new(Notify::new());
        let thread = std::thread::Builder::new()
            .name("qdp-session".to_owned())
            .spawn({
                let cursor = Arc::clone(&cursor);
                let stop = Arc::clone(&stop);
                move || run_thread(&config, queue, sender, cursor, &stop)
            })?;
        Ok(Self {
            events: EventReader {
                events: Arc::new(Mutex::new(receiver)),
                input: input.clone(),
            },
            input,
            cursor,
            stop,
            thread: Some(thread),
        })
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
    pub fn cursor(&self) -> Option<CursorBitmap> {
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
fn run_thread(
    config: &Config,
    input: InputQueue,
    events: Sender<Event>,
    cursor: SharedCursor,
    stop: &Notify,
) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("qdp-runtime")
        .build();
    let ending = match runtime {
        Ok(runtime) => {
            let mut sink = AndroidSink::new(events.clone(), cursor);
            runtime.block_on(receive(config, &mut sink, stop.notified(), input, &events))
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
/// completes.
async fn receive(
    config: &Config,
    sink: &mut AndroidSink,
    stop: impl Future<Output = ()>,
    mut input: InputQueue,
    events: &Sender<Event>,
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
    let endpoint = match ClientEndpoint::bind(local) {
        Ok(endpoint) => endpoint,
        Err(error) => return Ending::Failed(format!("cannot open a socket: {error}")),
    };
    let resumption = ResumptionSlot::default();
    let sessions = async {
        let mut credentials = ClientCredentials::Token(config.token.as_bytes().to_vec());
        // While resuming: when the token in hand expires.
        let mut deadline = None;
        let mut delay = FIRST_RECONNECT_DELAY;
        loop {
            let options = ClientOptions {
                client_name: CLIENT_NAME.to_owned(),
                credentials: credentials.clone(),
                display: config.display,
                decoder_threads: 0,
                decoder: config.decoder,
                decoder_process: None,
                keyboard_info: None,
                resumption: resumption.clone(),
            };
            let attempt = connect_once(
                address, config, &endpoint, &options, sink, &mut input, events,
            );
            let error = match attempt.await {
                Ok(()) => return Ending::Normal,
                Err(Failure::Fatal(error)) => return Ending::Failed(error),
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

/// Why a connection ended.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Failure {
    /// The connection could not be made, or was lost: reconnecting may help.
    Lost(String),
    /// The session failed for another reason, which ends it.
    Fatal(String),
}

/// Connects to the host once and runs a session with `options` until it
/// ends.
async fn connect_once(
    address: SocketAddr,
    config: &Config,
    endpoint: &ClientEndpoint,
    options: &ClientOptions,
    sink: &mut AndroidSink,
    input: &mut InputQueue,
    events: &Sender<Event>,
) -> Result<(), Failure> {
    let _ = events.send(Event::Connecting);
    let connection = endpoint
        .connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![config.pin]),
        )
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
        client_audio(config.microphone),
    )
    .await
    .map_err(|error| {
        let message = format!("session failed: {error}");
        if error.is_connection_lost() {
            Failure::Lost(message)
        } else {
            Failure::Fatal(message)
        }
    })?;
    info!(
        frames = summary.frames_presented,
        decode_failures = summary.decode_failures,
        "disconnected"
    );
    Ok(())
}

/// Returns the device's speakers and microphone.
#[cfg(target_os = "android")]
fn client_audio(microphone: Microphone) -> ClientAudio {
    qdp_desktop::client_audio(microphone)
}

/// Returns no audio devices: only Android's are used.
#[cfg(not(target_os = "android"))]
fn client_audio(microphone: Microphone) -> ClientAudio {
    let _ = microphone;
    ClientAudio::default()
}
