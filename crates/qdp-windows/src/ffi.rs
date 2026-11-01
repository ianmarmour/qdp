//! The C functions of `include/qdp_windows.h`, which the app calls from C#.
//!
//! A session is a handle the app holds: [`qdp_session_start`] makes it and
//! opens its stream window, a thread of the app's reads its lines with
//! [`qdp_session_next_event`] until the last, `ended` or `failed`,
//! [`qdp_session_stop`] ends it from any thread, and [`qdp_session_free`]
//! frees it once no thread uses it. C is an unsafe interface, so this module
//! uses `unsafe`.

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char};
use std::fs::File;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::Duration;

use qdp_app::events::Ending;
use qdp_app::session::{Authentication, Config};
use qdp_app::settings::{format_pin, parse_pin};
use qdp_codec::CodecPreference;
use qdp_desktop::SyncMode;
use qdp_quic::{IdentityFiles, TlsIdentity};
use qdp_runtime::{DisplaySize, Microphone};
use tracing::{info, warn};

use crate::events::EventQueue;
use crate::window::{Command, Screen, SessionMessage, Start, WindowChoices, send};

/// The product name the app sends hosts in `CLIENT_HELLO`.
const CLIENT_NAME: &str = concat!("qdp-windows/", env!("CARGO_PKG_VERSION"));

/// The core's version, NUL-terminated, as `qdp_version` returns it.
const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// `QDP_AUTH_TOKEN`: the host's key is pinned and the device presents a
/// token.
const AUTH_TOKEN: i32 = 0;

/// `QDP_AUTH_PAIRED`: a host the device paired with.
const AUTH_PAIRED: i32 = 1;

/// `QDP_AUTH_PAIR`: pairing with the code the host shows.
const AUTH_PAIR: i32 = 2;

/// `QDP_DECODER_HARDWARE`: D3D11 video only. Any value but this and
/// [`DECODER_SOFTWARE`] decodes in hardware when the GPU can.
const DECODER_HARDWARE: i32 = 1;

/// `QDP_DECODER_SOFTWARE`: rav1d.
const DECODER_SOFTWARE: i32 = 2;

/// `QDP_SCREEN_FULL`: the stream window opens full screen; any other value
/// opens it in a window.
const SCREEN_FULL: i32 = 1;

/// `QDP_SYNC_TEARING`: pictures are shown as soon as they are drawn; any
/// other value waits for the display's refresh.
const SYNC_TEARING: i32 = 1;

/// The smallest display side the host accepts, in pixels.
const MIN_DISPLAY_SIDE: u32 = 16;

/// The largest display side the host accepts, in pixels.
const MAX_DISPLAY_SIDE: u32 = 16_384;

/// The log file, in the app's data directory.
const LOG_FILE: &str = "qdp.log";

/// The previous run's log file, kept for troubleshooting.
const PREVIOUS_LOG_FILE: &str = "qdp.previous.log";

/// Sets up logging once.
static INIT: Once = Once::new();

/// The app's data directory, where the device's key and the log are kept.
static DATA_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// The number of the next session.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// What a session connects with, as `QdpConfig` lays it out.
#[repr(C)]
pub struct QdpConfig {
    /// The host's address or name, with or without a port.
    host: *const c_char,
    /// The host's key pin, for token and paired authentication.
    pin: *const c_char,
    /// The token, for token authentication.
    token: *const c_char,
    /// The device's name, which the host shows when pairing.
    device_name: *const c_char,
    /// The stream window's title.
    title: *const c_char,
    /// The icon file the stream window shows, or null.
    icon_path: *const c_char,
    /// The app's window, whose monitor the stream window opens on, or 0.
    owner_window: isize,
    /// A `QdpAuthentication` value.
    authentication: i32,
    /// The width of the video to ask for, in pixels, or 0 for the window's.
    width: u32,
    /// Its height, or 0 for the window's.
    height: u32,
    /// A `QdpDecoder` value.
    decoder: i32,
    /// Whether the microphone starts on: 1 if so, 0 if not.
    microphone: i32,
    /// A `QdpScreen` value.
    screen: i32,
    /// A `QdpSync` value.
    sync: i32,
}

/// A session, which the app holds as `QdpSession *`.
pub struct QdpSession {
    /// The session's number, which its window's messages carry.
    serial: u64,
    /// The lines the app reads.
    queue: Arc<EventQueue>,
}

impl QdpSession {
    /// Asks the session's window for something; if the window thread is
    /// gone, the session has ended.
    fn tell(&self, message: SessionMessage) {
        if let Err(error) = send(Command::Session(self.serial, message)) {
            warn!(error, "the session's window cannot be reached");
            self.queue.finish(&Ending::Failed(error));
        }
    }
}

/// Runs `body`, returning `fallback` if it panics, rather than unwinding
/// into C#.
fn guarded<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(fallback)
}

/// Reads a C string, or returns an empty one for null.
///
/// # Safety
///
/// `text` is null or a live NUL-terminated string.
unsafe fn read(text: *const c_char) -> String {
    if text.is_null() {
        return String::new();
    }
    // SAFETY: as the caller promises.
    unsafe { CStr::from_ptr(text) }
        .to_string_lossy()
        .into_owned()
}

/// Returns `text` as a C string the app frees with [`qdp_string_free`].
fn new_string(text: &str) -> *mut c_char {
    CString::new(text.replace('\0', " ")).map_or(ptr::null_mut(), CString::into_raw)
}

/// Returns the session a handle points to.
///
/// # Safety
///
/// `session` is null or a handle [`qdp_session_start`] returned that was
/// not freed.
unsafe fn session<'a>(session: *mut QdpSession) -> Option<&'a QdpSession> {
    // SAFETY: as the caller promises.
    unsafe { session.as_ref() }
}

/// Returns the device's key pin, generating the key the first time.
fn client_pin() -> Result<qdp_quic::KeyPin, String> {
    let directory = DATA_DIRECTORY
        .get()
        .ok_or_else(|| "qdp_init was not called".to_owned())?;
    let (identity, _) = TlsIdentity::load_or_generate(directory, IdentityFiles::CLIENT)
        .map_err(|error| error.to_string())?;
    identity.key_pin().map_err(|error| error.to_string())
}

/// Logs to the log file in `directory`, keeping the previous run's, or to
/// the standard error if the file cannot be written.
fn start_logging(directory: &Path) {
    let log = directory.join(LOG_FILE);
    let _ = std::fs::rename(&log, directory.join(PREVIOUS_LOG_FILE));
    let subscriber = tracing_subscriber::fmt().with_ansi(false);
    let started = match std::fs::create_dir_all(directory).and_then(|()| File::create(&log)) {
        Ok(file) => subscriber.with_writer(Mutex::new(file)).try_init(),
        Err(_) => subscriber.with_writer(std::io::stderr).try_init(),
    };
    if started.is_ok() {
        info!(version = env!("CARGO_PKG_VERSION"), "QDP core started");
    }
}

/// Sets up logging and keeps the app's data directory, where the device's
/// key and the log are kept.
///
/// # Safety
///
/// `data_directory` is a live NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_init(data_directory: *const c_char) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let directory = PathBuf::from(unsafe { read(data_directory) });
        INIT.call_once(|| start_logging(&directory));
        let _ = DATA_DIRECTORY.set(directory);
    });
}

/// Returns the core's version, which the app must not free.
#[unsafe(no_mangle)]
pub extern "C" fn qdp_version() -> *const c_char {
    VERSION.as_ptr().cast()
}

/// Returns the device's key pin, or null if its key cannot be kept.
#[unsafe(no_mangle)]
pub extern "C" fn qdp_client_pin() -> *mut c_char {
    guarded(ptr::null_mut(), || match client_pin() {
        Ok(pin) => new_string(&format_pin(pin)),
        Err(error) => {
            warn!(%error, "cannot keep the device's key");
            ptr::null_mut()
        }
    })
}

/// Returns whether `text` is a key pin.
///
/// # Safety
///
/// `text` is null or a live NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_is_pin(text: *const c_char) -> bool {
    // SAFETY: as the caller promises.
    guarded(false, || parse_pin(&unsafe { read(text) }).is_ok())
}

/// Starts a session in a stream window of its own, which opens hidden and
/// shows once the video arrives. Returns null, with the reason in `error`
/// for the app to free, if the configuration is wrong; a session that
/// fails later says so in its last line.
///
/// # Safety
///
/// `config` points to a live `QdpConfig` whose strings are null or live, and
/// `error` is null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_start(
    config: *const QdpConfig,
    error: *mut *mut c_char,
) -> *mut QdpSession {
    guarded(ptr::null_mut(), || {
        // SAFETY: as the caller promises.
        match unsafe { start(config) } {
            Ok(handle) => Box::into_raw(Box::new(handle)),
            Err(message) => {
                warn!(error = message, "the session cannot start");
                if !error.is_null() {
                    // SAFETY: the caller promises `error` is writable.
                    unsafe { *error = new_string(&message) };
                }
                ptr::null_mut()
            }
        }
    })
}

/// Starts a session as [`qdp_session_start`] says.
///
/// # Safety
///
/// As for [`qdp_session_start`].
unsafe fn start(config: *const QdpConfig) -> Result<QdpSession, String> {
    // SAFETY: as the caller promises.
    let config = unsafe { config.as_ref() }.ok_or("no configuration")?;
    // SAFETY: the caller promises live strings or nulls.
    let (host, pin, token, device_name, title, icon_path) = unsafe {
        (
            read(config.host),
            read(config.pin),
            read(config.token),
            read(config.device_name),
            read(config.title),
            read(config.icon_path),
        )
    };
    let authentication = match config.authentication {
        AUTH_TOKEN => Authentication::Token {
            pin: parse_pin(&pin)?,
            token,
        },
        AUTH_PAIRED => Authentication::Paired {
            pin: parse_pin(&pin)?,
        },
        AUTH_PAIR => Authentication::Pair { device_name },
        other => return Err(format!("unknown authentication {other}")),
    };
    let identity_directory = DATA_DIRECTORY
        .get()
        .cloned()
        .ok_or("qdp_init was not called")?;
    let side = |pixels: u32| pixels.clamp(MIN_DISPLAY_SIDE, MAX_DISPLAY_SIDE);
    let resolution =
        (config.width > 0 && config.height > 0).then(|| (side(config.width), side(config.height)));
    let serial = NEXT_SERIAL.fetch_add(1, Ordering::Relaxed);
    let queue = Arc::new(EventQueue::default());
    let start = Start {
        serial,
        config: Config {
            client_name: CLIENT_NAME.to_owned(),
            host: host.clone(),
            authentication,
            identity_directory,
            // The stream window measures its display once it opens.
            display: DisplaySize {
                width: 0,
                height: 0,
                refresh_millihertz: 0,
                scale_percent: DisplaySize::UNSCALED,
            },
            decoder: match config.decoder {
                DECODER_HARDWARE => CodecPreference::Hardware,
                DECODER_SOFTWARE => CodecPreference::Software,
                _ => CodecPreference::Auto,
            },
            microphone: if config.microphone != 0 {
                Microphone::On
            } else {
                Microphone::Off
            },
            client_audio: qdp_desktop::client_audio,
        },
        choices: WindowChoices {
            title: if title.is_empty() { host } else { title },
            screen: if config.screen == SCREEN_FULL {
                Screen::FullScreen
            } else {
                Screen::Windowed
            },
            sync: if config.sync == SYNC_TEARING {
                SyncMode::Tearing
            } else {
                SyncMode::VSync
            },
            resolution,
            owner: (config.owner_window != 0).then_some(config.owner_window),
            icon: (!icon_path.is_empty()).then(|| PathBuf::from(icon_path)),
        },
        queue: Arc::clone(&queue),
    };
    send(Command::Start(Box::new(start)))?;
    Ok(QdpSession { serial, queue })
}

/// Returns the session's next line for the app to free with
/// [`qdp_string_free`], waiting up to `timeout_millis` for one, or null if
/// none came. After the last line it waits out the timeout and returns
/// null.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_next_event(
    session: *mut QdpSession,
    timeout_millis: u32,
) -> *mut c_char {
    guarded(ptr::null_mut(), || {
        let timeout = Duration::from_millis(timeout_millis.into());
        // SAFETY: as the caller promises.
        let Some(handle) = (unsafe { self::session(session) }) else {
            std::thread::sleep(timeout);
            return ptr::null_mut();
        };
        handle
            .queue
            .next(timeout)
            .map_or(ptr::null_mut(), |line| new_string(&line))
    })
}

/// Answers the session's request for the code the host shows with `code`,
/// or declines with null.
///
/// # Safety
///
/// `session` is null or a live handle, and `code` null or a live
/// NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_pairing_code(session: *mut QdpSession, code: *const c_char) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let code = (!code.is_null()).then(|| unsafe { read(code) });
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { self::session(session) } {
            handle.tell(SessionMessage::PairingCode(code));
        }
    });
}

/// Ends the session and closes its window, returning at once: its last line
/// follows once it has ended.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_stop(session: *mut QdpSession) {
    guarded((), || {
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { self::session(session) } {
            if !handle.queue.finished() {
                handle.tell(SessionMessage::Stop);
            }
        }
    });
}

/// Ends the session if it has not ended, and frees the handle, which no
/// thread may use any more.
///
/// # Safety
///
/// `session` is null or a live handle no other thread uses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_free(session: *mut QdpSession) {
    guarded((), || {
        if session.is_null() {
            return;
        }
        // SAFETY: the caller gives the handle up.
        let handle = unsafe { Box::from_raw(session) };
        if !handle.queue.finished() {
            handle.tell(SessionMessage::Stop);
        }
    });
}

/// Frees a string the core returned.
///
/// # Safety
///
/// `string` is null or a string the core returned, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_string_free(string: *mut c_char) {
    guarded((), || {
        if !string.is_null() {
            // SAFETY: the string was made by `CString::into_raw`.
            drop(unsafe { CString::from_raw(string) });
        }
    });
}
