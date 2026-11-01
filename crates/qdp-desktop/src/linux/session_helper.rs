//! A helper in a person's desktop session, for a host running outside any
//! session, as the system service that streams the console does. The
//! session's notification service and its compositor's clipboard serve only
//! the session's own user, so the host starts `qdp session-helper` as that
//! user, in the session's environment, and asks it over a pipe: to show and
//! take away notifications, to read and replace the clipboard's text, to
//! bring a monitor to the client's display through the compositor's display
//! settings, and back, and to tell where a monitor is among the desktop's.
//!
//! The host follows the session shown on `seat0`, starting a helper for its
//! user when one is needed, and another when someone else logs in.

use std::collections::HashMap;
use std::io::{self, BufReader, BufWriter, ErrorKind, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use qdp_runtime::{Clipboard, LayoutRect, MonitorPlacement, WantedDisplay};
use tracing::{debug, info};

use super::seat::{SeatSession, seat_session};
use crate::{SESSION_HELPER_COMMAND, SystemClipboard, Urgency};

/// The most bytes a message carries: more than any text the clipboard
/// shares.
const MAX_MESSAGE: u32 = 16 << 20;

/// The start of a Wayland compositor's socket's name in the runtime
/// directory.
const WAYLAND_SOCKET_PREFIX: &str = "wayland-";

/// The end of the lock files beside Wayland sockets.
const LOCK_SUFFIX: &str = ".lock";

/// The session bus's socket in the runtime directory.
const BUS_SOCKET: &str = "bus";

/// The commands' directories the helper looks in, as a login's are.
const HELPER_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

/// The variable that sets the log's levels, which the helper keeps.
const LOG_VARIABLE: &str = "QDP_LOG";

/// What separates a message's fields.
const FIELD_SEPARATOR: u8 = 0;

/// How long a helper has to finish, putting back a monitor it changed,
/// once its input closes, before it is ended.
const HELPER_FINISH: Duration = Duration::from_secs(3);

/// How often an ending helper is looked at.
const HELPER_FINISH_POLL: Duration = Duration::from_millis(50);

/// The file in the session's runtime directory where a monitor's own
/// settings are kept while the helper changes them, so that a helper that
/// ended without putting them back has the next one put them back.
#[cfg(feature = "screen-capture")]
const MONITOR_SETTINGS_FILE: &str = "qdp-monitor-settings";

/// What a message asks or answers: its first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    /// Show a notification: its number, urgency, lifetime in milliseconds,
    /// summary and body, apart.
    Notify,
    /// Take away the notification of the number given.
    Close,
    /// Read the clipboard's text.
    GetText,
    /// Replace the clipboard's text with the text given.
    SetText,
    /// Bring a monitor to a display: its name, width, height, refresh rate
    /// in millihertz and scale in percent, apart; done with the width and
    /// height it took.
    MatchDisplay,
    /// Put back the monitor's own settings.
    RestoreDisplay,
    /// Tell where the monitor named is among the desktop's: done with its
    /// rectangle and the desktop's, in the desktop's coordinates.
    Layout,
    /// Done, with the clipboard's text when it was read.
    Done,
    /// The clipboard holds no text.
    Empty,
    /// Failed, saying why.
    Failed,
}

impl Kind {
    /// Every kind with its byte.
    const BYTES: [(Self, u8); 10] = [
        (Self::Notify, b'n'),
        (Self::Close, b'c'),
        (Self::GetText, b'g'),
        (Self::SetText, b's'),
        (Self::MatchDisplay, b'm'),
        (Self::RestoreDisplay, b'r'),
        (Self::Layout, b'l'),
        (Self::Done, b'd'),
        (Self::Empty, b'e'),
        (Self::Failed, b'f'),
    ];

    /// Returns the kind's byte.
    fn byte(self) -> u8 {
        Self::BYTES
            .iter()
            .find(|(kind, _)| *kind == self)
            .map_or(0, |&(_, byte)| byte)
    }

    /// Returns the kind of byte `byte`, if it is one.
    fn of(byte: u8) -> Option<Self> {
        Self::BYTES
            .iter()
            .find(|&&(_, known)| known == byte)
            .map(|&(kind, _)| kind)
    }
}

/// Writes a message: its kind's byte, its payload's length and the payload.
fn write_message(writer: &mut impl Write, kind: Kind, payload: &[u8]) -> io::Result<()> {
    let length = u32::try_from(payload.len())
        .ok()
        .filter(|&length| length <= MAX_MESSAGE)
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidInput, "the message is too long"))?;
    writer.write_all(&[kind.byte()])?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(payload)?;
    writer.flush()
}

/// Reads a message.
fn read_message(reader: &mut impl Read) -> io::Result<(Kind, Vec<u8>)> {
    let mut header = [0u8; 5];
    reader.read_exact(&mut header)?;
    let kind = Kind::of(header[0])
        .ok_or_else(|| io::Error::new(ErrorKind::InvalidData, "an unknown message"))?;
    let length = u32::from_le_bytes([header[1], header[2], header[3], header[4]]);
    if length > MAX_MESSAGE {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "the message is too long",
        ));
    }
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload)?;
    Ok((kind, payload))
}

/// A notification to show, as a message carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct NotificationRequest {
    /// Its number, by which it is taken away.
    number: u64,
    /// How much it needs the user.
    urgency: Urgency,
    /// How long it stays at most.
    lifetime: Duration,
    /// Its title.
    summary: String,
    /// Its text.
    body: String,
}

impl NotificationRequest {
    /// Returns the message's payload. The texts' separators are dropped
    /// from them.
    fn encode(&self) -> Vec<u8> {
        let urgency = match self.urgency {
            Urgency::Normal => "normal",
            Urgency::Critical => "critical",
        };
        let fields = [
            self.number.to_string(),
            urgency.to_owned(),
            self.lifetime.as_millis().to_string(),
            self.summary.replace(char::from(FIELD_SEPARATOR), ""),
            self.body.replace(char::from(FIELD_SEPARATOR), ""),
        ];
        fields.join("\0").into_bytes()
    }

    /// Reads a message's payload.
    fn decode(payload: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(payload).ok()?;
        let mut fields = text.split(char::from(FIELD_SEPARATOR));
        let number = fields.next()?.parse().ok()?;
        let urgency = match fields.next()? {
            "normal" => Urgency::Normal,
            "critical" => Urgency::Critical,
            _ => return None,
        };
        let lifetime = Duration::from_millis(fields.next()?.parse().ok()?);
        let summary = fields.next()?.to_owned();
        let body = fields.next()?.to_owned();
        Some(Self {
            number,
            urgency,
            lifetime,
            summary,
            body,
        })
    }
}

/// A monitor to bring to a display, as a message carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MatchRequest {
    /// The monitor's name, such as `DP-3`.
    monitor: String,
    /// The display to bring it to.
    wanted: WantedDisplay,
}

impl MatchRequest {
    /// Returns the message's payload. The name's separators are dropped.
    fn encode(&self) -> Vec<u8> {
        let fields = [
            self.monitor.replace(char::from(FIELD_SEPARATOR), ""),
            self.wanted.width.to_string(),
            self.wanted.height.to_string(),
            self.wanted.refresh_millihertz.to_string(),
            self.wanted.scale_percent.to_string(),
        ];
        fields.join("\0").into_bytes()
    }

    /// Reads a message's payload.
    fn decode(payload: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(payload).ok()?;
        let mut fields = text.split(char::from(FIELD_SEPARATOR));
        let monitor = fields.next()?.to_owned();
        let mut number = || fields.next()?.parse::<u32>().ok();
        let wanted = WantedDisplay {
            width: number()?,
            height: number()?,
            refresh_millihertz: number()?,
            scale_percent: number()?,
        };
        Some(Self { monitor, wanted })
    }
}

/// Returns a size's payload: its width and height, apart.
fn encode_size((width, height): (u32, u32)) -> Vec<u8> {
    format!("{width}\0{height}").into_bytes()
}

/// Reads a size's payload.
fn decode_size(payload: &[u8]) -> Option<(u32, u32)> {
    let text = std::str::from_utf8(payload).ok()?;
    let (width, height) = text.split_once(char::from(FIELD_SEPARATOR))?;
    Some((width.parse().ok()?, height.parse().ok()?))
}

/// Returns a placement's payload: the monitor's left, top, width and
/// height, then the desktop's, apart.
fn encode_placement(placement: &MonitorPlacement) -> Vec<u8> {
    let rect = |rect: LayoutRect| {
        [
            rect.x.to_string(),
            rect.y.to_string(),
            rect.width.to_string(),
            rect.height.to_string(),
        ]
    };
    let mut fields = rect(placement.monitor).to_vec();
    fields.extend(rect(placement.desktop));
    fields.join("\0").into_bytes()
}

/// Reads a placement's payload.
fn decode_placement(payload: &[u8]) -> Option<MonitorPlacement> {
    let text = std::str::from_utf8(payload).ok()?;
    let mut fields = text.split(char::from(FIELD_SEPARATOR));
    let mut rect = || {
        Some(LayoutRect {
            x: fields.next()?.parse().ok()?,
            y: fields.next()?.parse().ok()?,
            width: fields.next()?.parse().ok()?,
            height: fields.next()?.parse().ok()?,
        })
    };
    Some(MonitorPlacement {
        monitor: rect()?,
        desktop: rect()?,
    })
}

/// The helper's pipe: requests go in, answers come out, one at a time.
struct Pipe {
    /// Requests, until the helper is asked to finish.
    input: Option<ChildStdin>,
    /// Answers.
    output: BufReader<ChildStdout>,
    /// Whether it broke, as when the helper ended.
    broken: bool,
}

/// A helper running in a person's session, ended when dropped.
pub(crate) struct SessionHelper {
    /// The person's user ID.
    uid: u32,
    /// The helper's process.
    process: Mutex<Child>,
    /// Its pipe.
    pipe: Mutex<Pipe>,
    /// The next notification's number.
    next_notification: AtomicU64,
}

impl SessionHelper {
    /// Starts a helper as the person whose session `session` is, in its
    /// environment.
    fn start(session: &SeatSession) -> Result<Self, String> {
        let command = std::env::current_exe()
            .map_err(|error| format!("cannot find this command: {error}"))?;
        let runtime = &session.runtime_directory;
        let mut process = Command::new(command);
        process
            .arg(SESSION_HELPER_COMMAND)
            .env_clear()
            .env("PATH", HELPER_PATH)
            .env("XDG_RUNTIME_DIR", runtime)
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}", runtime.join(BUS_SOCKET).display()),
            )
            // Starting as the person drops root's groups too.
            .uid(session.uid)
            .gid(session.gid)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if let Some(display) = wayland_socket(runtime) {
            process.env("WAYLAND_DISPLAY", display);
        }
        if let Some(levels) = std::env::var_os(LOG_VARIABLE) {
            process.env(LOG_VARIABLE, levels);
        }
        let mut child = process
            .spawn()
            .map_err(|error| format!("cannot start the session's helper: {error}"))?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err("the session's helper has no pipe".to_owned());
        };
        Ok(Self {
            uid: session.uid,
            process: Mutex::new(child),
            pipe: Mutex::new(Pipe {
                input: Some(input),
                output: BufReader::new(output),
                broken: false,
            }),
            next_notification: AtomicU64::new(0),
        })
    }

    /// Returns whether the helper still runs and answers.
    fn is_alive(&self) -> bool {
        !lock(&self.pipe).broken && matches!(lock(&self.process).try_wait(), Ok(None))
    }

    /// Asks the helper `kind` with `payload`, and returns its answer.
    fn request(&self, kind: Kind, payload: &[u8]) -> Result<(Kind, Vec<u8>), String> {
        let mut pipe = lock(&self.pipe);
        let pipe = &mut *pipe;
        let Some(input) = pipe.input.as_mut().filter(|_| !pipe.broken) else {
            return Err("the session's helper ended".to_owned());
        };
        let answer =
            write_message(input, kind, payload).and_then(|()| read_message(&mut pipe.output));
        answer.map_err(|error| {
            pipe.broken = true;
            format!("the session's helper did not answer: {error}")
        })
    }

    /// Asks the helper `kind` with `payload`, which it answers with done.
    fn request_done(&self, kind: Kind, payload: &[u8]) -> Result<Vec<u8>, String> {
        match self.request(kind, payload)? {
            (Kind::Done, body) => Ok(body),
            (_, reason) => Err(String::from_utf8_lossy(&reason).into_owned()),
        }
    }

    /// Shows a notification in the session.
    pub(crate) fn notify(
        self: &Arc<Self>,
        summary: &str,
        body: &str,
        lifetime: Duration,
        urgency: Urgency,
    ) -> Result<HelperNotification, String> {
        let number = self.next_notification.fetch_add(1, Ordering::Relaxed);
        let request = NotificationRequest {
            number,
            urgency,
            lifetime,
            summary: summary.to_owned(),
            body: body.to_owned(),
        };
        self.request_done(Kind::Notify, &request.encode())?;
        Ok(HelperNotification {
            helper: Arc::clone(self),
            number,
        })
    }

    /// Brings the session's monitor named `monitor` as close to `wanted`
    /// as its modes allow, and returns the size it took.
    pub(crate) fn match_display(
        &self,
        monitor: &str,
        wanted: &WantedDisplay,
    ) -> Result<(u32, u32), String> {
        let request = MatchRequest {
            monitor: monitor.to_owned(),
            wanted: *wanted,
        };
        let size = self.request_done(Kind::MatchDisplay, &request.encode())?;
        decode_size(&size).ok_or_else(|| "the helper answered no size".to_owned())
    }

    /// Returns where the session's monitor named `monitor` is among the
    /// desktop's monitors, in the desktop's coordinates.
    pub(crate) fn placement(&self, monitor: &str) -> Result<MonitorPlacement, String> {
        let placement = self.request_done(Kind::Layout, monitor.as_bytes())?;
        decode_placement(&placement).ok_or_else(|| "the helper answered no placement".to_owned())
    }

    /// Puts back the own settings of the monitor `match_display` changed.
    pub(crate) fn restore_display(&self) {
        if let Err(error) = self.request_done(Kind::RestoreDisplay, &[]) {
            debug!(%error, "cannot put the monitor's own settings back");
        }
    }
}

impl Drop for SessionHelper {
    fn drop(&mut self) {
        // The helper finishes once its input closes, putting back a monitor
        // it changed; one that does not finish in time is ended.
        drop(lock(&self.pipe).input.take());
        let process = self
            .process
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        let deadline = Instant::now() + HELPER_FINISH;
        while Instant::now() < deadline {
            if !matches!(process.try_wait(), Ok(None)) {
                return;
            }
            std::thread::sleep(HELPER_FINISH_POLL);
        }
        let _ = process.kill();
        let _ = process.wait();
    }
}

/// A notification a helper shows.
pub(crate) struct HelperNotification {
    /// The helper.
    helper: Arc<SessionHelper>,
    /// Its number.
    number: u64,
}

impl std::fmt::Debug for HelperNotification {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HelperNotification")
            .field("number", &self.number)
            .finish_non_exhaustive()
    }
}

impl HelperNotification {
    /// Takes the notification away.
    pub(crate) fn close(self) {
        if let Err(error) = self
            .helper
            .request_done(Kind::Close, self.number.to_string().as_bytes())
        {
            debug!(%error, "cannot take the notification away");
        }
    }
}

/// The session's clipboard, which a helper reads and replaces.
pub(crate) struct HelperClipboard {
    /// The helper.
    helper: Arc<SessionHelper>,
}

impl HelperClipboard {
    /// Returns the clipboard of the helper's session.
    pub(crate) fn new(helper: Arc<SessionHelper>) -> Self {
        Self { helper }
    }
}

impl Clipboard for HelperClipboard {
    fn text(&mut self) -> Result<Option<String>, String> {
        match self.helper.request(Kind::GetText, &[])? {
            (Kind::Done, text) => String::from_utf8(text)
                .map(Some)
                .map_err(|_| "the clipboard's text is not UTF-8".to_owned()),
            (Kind::Empty, _) => Ok(None),
            (_, reason) => Err(String::from_utf8_lossy(&reason).into_owned()),
        }
    }

    fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.helper
            .request_done(Kind::SetText, text.as_bytes())
            .map(|_| ())
    }

    fn description(&self) -> &str {
        "the console session's clipboard"
    }
}

/// The helper of the session shown on the console, once the host follows
/// it.
static CONSOLE: OnceLock<Mutex<Option<Arc<SessionHelper>>>> = OnceLock::new();

/// Has notifications and the clipboard reach the person's desktop session
/// shown on the console, through a helper running as its user, rather than
/// this process's own session: for a host streaming the console from
/// outside any session.
pub fn follow_console_session() {
    CONSOLE.get_or_init(Mutex::default);
}

/// Returns the helper of the session shown on the console, starting one
/// for its user if none runs, if the host follows the console; it fails
/// while no one is logged in there.
pub(crate) fn console_helper() -> Option<Result<Arc<SessionHelper>, String>> {
    let console = CONSOLE.get()?;
    let Some(session) = seat_session() else {
        return Some(Err("no one is logged in on the console".to_owned()));
    };
    let mut current = console.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(helper) = current.as_ref() {
        if helper.uid == session.uid && helper.is_alive() {
            return Some(Ok(Arc::clone(helper)));
        }
    }
    let started = SessionHelper::start(&session).map(Arc::new);
    if let Ok(helper) = &started {
        info!(
            uid = session.uid,
            "started a helper in the console's desktop session"
        );
        *current = Some(Arc::clone(helper));
    }
    Some(started)
}

/// Returns the name of the Wayland compositor's socket in `runtime`, if
/// there is one.
fn wayland_socket(runtime: &Path) -> Option<String> {
    let mut names: Vec<String> = std::fs::read_dir(runtime)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(WAYLAND_SOCKET_PREFIX) && !name.ends_with(LOCK_SUFFIX))
        .collect();
    names.sort();
    names.into_iter().next()
}

/// Returns the lock's value, even if a thread panicked holding it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Serves a host's requests on standard input and output, in this desktop
/// session, until the host closes its end: what `qdp session-helper`
/// does, which the host starts as the session's user.
pub fn run_session_helper() -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .map_err(|error| format!("cannot start the helper's runtime: {error}"))?;
    let mut input = BufReader::new(io::stdin().lock());
    let mut output = BufWriter::new(io::stdout().lock());
    let mut shown: HashMap<u64, crate::Notification> = HashMap::new();
    let mut clipboard: Option<SystemClipboard> = None;
    let mut displays = Displays::new();
    loop {
        let (kind, payload) = match read_message(&mut input) {
            Ok(message) => message,
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(format!("cannot read the host's request: {error}")),
        };
        let (answer, body) = match kind {
            Kind::Notify => match NotificationRequest::decode(&payload) {
                Some(request) => {
                    let notified = runtime.block_on(crate::notify(
                        &request.summary,
                        &request.body,
                        request.lifetime,
                        request.urgency,
                    ));
                    match notified {
                        Ok(notification) => {
                            shown.insert(request.number, notification);
                            (Kind::Done, Vec::new())
                        }
                        Err(error) => (Kind::Failed, error.into_bytes()),
                    }
                }
                None => (Kind::Failed, b"a malformed notification".to_vec()),
            },
            Kind::Close => {
                let number = std::str::from_utf8(&payload)
                    .ok()
                    .and_then(|number| number.parse::<u64>().ok());
                if let Some(notification) = number.and_then(|number| shown.remove(&number)) {
                    runtime.block_on(notification.close());
                }
                (Kind::Done, Vec::new())
            }
            Kind::GetText => match open_clipboard(&mut clipboard).and_then(Clipboard::text) {
                Ok(Some(text)) => (Kind::Done, text.into_bytes()),
                Ok(None) => (Kind::Empty, Vec::new()),
                Err(error) => (Kind::Failed, error.into_bytes()),
            },
            Kind::SetText => {
                let set = String::from_utf8(payload)
                    .map_err(|_| "the text is not UTF-8".to_owned())
                    .and_then(|text| open_clipboard(&mut clipboard)?.set_text(&text));
                match set {
                    Ok(()) => (Kind::Done, Vec::new()),
                    Err(error) => (Kind::Failed, error.into_bytes()),
                }
            }
            Kind::MatchDisplay => match MatchRequest::decode(&payload) {
                Some(request) => match displays.match_display(&request) {
                    Ok(size) => (Kind::Done, encode_size(size)),
                    Err(error) => (Kind::Failed, error.into_bytes()),
                },
                None => (Kind::Failed, b"a malformed display".to_vec()),
            },
            Kind::RestoreDisplay => {
                displays.restore();
                (Kind::Done, Vec::new())
            }
            Kind::Layout => match placement_of(&String::from_utf8_lossy(&payload)) {
                Ok(placement) => (Kind::Done, encode_placement(&placement)),
                Err(error) => (Kind::Failed, error.into_bytes()),
            },
            Kind::Done | Kind::Empty | Kind::Failed => (
                Kind::Failed,
                b"the helper answers, it does not ask".to_vec(),
            ),
        };
        write_message(&mut output, answer, &body)
            .map_err(|error| format!("cannot answer the host: {error}"))?;
    }
    for (_, notification) in shown.drain() {
        runtime.block_on(notification.close());
    }
    displays.restore();
    Ok(())
}

/// The session's monitor the helper brings to the client's display, and
/// puts back.
struct Displays {
    /// The monitor being changed.
    #[cfg(feature = "screen-capture")]
    matching: Option<super::monitor_matching::MonitorMatching>,
}

impl Displays {
    /// Puts back the settings a helper that ended without putting them back
    /// left changed, and returns no monitor changed yet.
    fn new() -> Self {
        #[cfg(feature = "screen-capture")]
        if let Some(path) = monitor_settings_path() {
            super::monitor_matching::restore_saved(&path);
        }
        Self {
            #[cfg(feature = "screen-capture")]
            matching: None,
        }
    }

    /// Brings the monitor `request` names to its display, and returns the
    /// size it took.
    #[cfg(feature = "screen-capture")]
    fn match_display(&mut self, request: &MatchRequest) -> Result<(u32, u32), String> {
        if self.matching.is_none() {
            let path = monitor_settings_path()
                .ok_or("the session has no runtime directory to keep the monitor's settings in")?;
            self.matching = Some(super::monitor_matching::MonitorMatching::new(
                request.monitor.clone(),
                path,
            )?);
        }
        let matching = self
            .matching
            .as_mut()
            .ok_or("the monitor cannot be changed")?;
        let settings = matching.apply(&request.wanted)?;
        Ok((settings.mode.width, settings.mode.height))
    }

    /// Fails: this build cannot change monitors.
    #[cfg(not(feature = "screen-capture"))]
    fn match_display(&mut self, _request: &MatchRequest) -> Result<(u32, u32), String> {
        Err("this build cannot change monitors".to_owned())
    }

    /// Puts back the monitor's own settings, if they were changed.
    fn restore(&mut self) {
        #[cfg(feature = "screen-capture")]
        drop(self.matching.take());
    }
}

/// Returns where the session's monitor named `monitor` is among the
/// desktop's monitors.
#[cfg(feature = "screen-capture")]
fn placement_of(monitor: &str) -> Result<MonitorPlacement, String> {
    let outputs = super::desktop_layout::outputs()?;
    let output = outputs
        .iter()
        .find(|output| output.name == monitor)
        .ok_or_else(|| format!("the desktop has no monitor named {monitor}"))?;
    super::desktop_layout::placement(output.rect, &outputs)
        .ok_or_else(|| "the desktop's monitors have no extent".to_owned())
}

/// Fails: this build cannot list the desktop's monitors.
#[cfg(not(feature = "screen-capture"))]
fn placement_of(_monitor: &str) -> Result<MonitorPlacement, String> {
    Err("this build cannot list the desktop's monitors".to_owned())
}

/// Returns where the monitor's own settings are kept while they are
/// changed, in the session's runtime directory.
#[cfg(feature = "screen-capture")]
fn monitor_settings_path() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|directory| std::path::PathBuf::from(directory).join(MONITOR_SETTINGS_FILE))
}

/// Returns the session's clipboard, opening it the first time.
fn open_clipboard(clipboard: &mut Option<SystemClipboard>) -> Result<&mut SystemClipboard, String> {
    if clipboard.is_none() {
        *clipboard = Some(SystemClipboard::open()?);
    }
    clipboard
        .as_mut()
        .ok_or_else(|| "the clipboard cannot be opened".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Messages read back as written, and too long a message is refused.
    #[test]
    fn frames_messages() {
        let mut wire = Vec::new();
        write_message(&mut wire, Kind::SetText, "héllo\0there".as_bytes()).unwrap();
        write_message(&mut wire, Kind::Empty, &[]).unwrap();
        let mut reader = wire.as_slice();
        assert_eq!(
            read_message(&mut reader).unwrap(),
            (Kind::SetText, "héllo\0there".as_bytes().to_vec())
        );
        assert_eq!(
            read_message(&mut reader).unwrap(),
            (Kind::Empty, Vec::new())
        );
        assert_eq!(
            read_message(&mut reader).unwrap_err().kind(),
            ErrorKind::UnexpectedEof
        );
        let mut too_long = vec![b'g'];
        too_long.extend_from_slice(&(MAX_MESSAGE + 1).to_le_bytes());
        assert!(read_message(&mut too_long.as_slice()).is_err());
        assert!(read_message(&mut [b'?', 0, 0, 0, 0].as_slice()).is_err());
    }

    /// Notifications read back as written, without separators in their
    /// texts.
    #[test]
    fn encodes_notifications() {
        let request = NotificationRequest {
            number: 7,
            urgency: Urgency::Critical,
            lifetime: Duration::from_secs(120),
            summary: "Pair the\0phone?".to_owned(),
            body: "Type 1234\non the phone.".to_owned(),
        };
        let decoded = NotificationRequest::decode(&request.encode()).unwrap();
        assert_eq!(decoded.number, 7);
        assert_eq!(decoded.urgency, Urgency::Critical);
        assert_eq!(decoded.lifetime, Duration::from_secs(120));
        assert_eq!(decoded.summary, "Pair thephone?");
        assert_eq!(decoded.body, "Type 1234\non the phone.");
        assert!(NotificationRequest::decode(b"7\x00loud\x00100\x00a\x00b").is_none());
        assert!(NotificationRequest::decode(b"7\x00normal\x00100\x00a").is_none());
    }

    /// Display requests and sizes read back as written.
    #[test]
    fn encodes_display_requests() {
        let request = MatchRequest {
            monitor: "DP-3".to_owned(),
            wanted: WantedDisplay {
                width: 2560,
                height: 1600,
                refresh_millihertz: 120_000,
                scale_percent: 200,
            },
        };
        assert_eq!(MatchRequest::decode(&request.encode()), Some(request));
        assert_eq!(MatchRequest::decode(b"DP-3\x002560\x001600"), None);
        assert_eq!(decode_size(&encode_size((2560, 1600))), Some((2560, 1600)));
        assert_eq!(decode_size(b"2560"), None);
    }

    /// Placements read back as written.
    #[test]
    fn encodes_placements() {
        let placement = MonitorPlacement {
            monitor: LayoutRect {
                x: 2560,
                y: -120,
                width: 1920,
                height: 1080,
            },
            desktop: LayoutRect {
                x: 0,
                y: -120,
                width: 4480,
                height: 1560,
            },
        };
        assert_eq!(
            decode_placement(&encode_placement(&placement)),
            Some(placement)
        );
        assert_eq!(decode_placement(b"1\x002\x003"), None);
    }

    /// Every kind reads back from its byte.
    #[test]
    fn kinds_round_trip() {
        for (kind, byte) in Kind::BYTES {
            assert_eq!(kind.byte(), byte);
            assert_eq!(Kind::of(byte), Some(kind));
        }
        assert_eq!(Kind::of(b'x'), None);
    }

    /// The compositor's socket is found among the runtime directory's
    /// files, its lock aside.
    #[test]
    fn finds_the_wayland_socket() {
        let directory =
            std::env::temp_dir().join(format!("qdp-helper-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        for name in ["bus", "wayland-0.lock", "wayland-0", "pipewire-0"] {
            std::fs::write(directory.join(name), b"").unwrap();
        }
        assert_eq!(wayland_socket(&directory).as_deref(), Some("wayland-0"));
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
