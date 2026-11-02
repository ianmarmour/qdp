//! The C functions of `include/qdp.h`, which the apps call from Swift.
//!
//! A session is a handle the app holds: [`qdp_session_start`] makes it on
//! the main thread with the layer the video is drawn in, the input
//! functions send the user's input to it from any thread, a thread of the
//! app's waits for its events with [`qdp_session_next_event`], and
//! [`qdp_session_stop`] ends and frees it on the main thread. C is an
//! unsafe interface, so this module uses `unsafe`.

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::Duration;

use qdp::message::DeviceFeatures;
use qdp_app::gamepad::GamepadState;
use qdp_app::session::{Authentication, Config, GatewayConfig, Session};
use qdp_app::settings::{format_pin, parse_pin};
use qdp_app::sink::SharedCursor;
use qdp_app::virtual_gamepad::{self, Insets, Point, Screen, VirtualGamepad};
use qdp_codec::CodecPreference;
use qdp_desktop::MetalVideo;
use qdp_quic::{IdentityFiles, TlsIdentity};
use qdp_runtime::{
    Credentials, DisplaySize, Focus, Microphone, PointerButton, SignInAnswer, TouchPhase, UserInput,
};
use tracing::{info, warn};

use crate::stage::{ApplePresenter, FrameId, Presentations, SharedStage, Stage, lock, reporter};

/// The product name the app sends hosts in `CLIENT_HELLO`.
const CLIENT_NAME: &str = concat!("qdp-apple/", env!("CARGO_PKG_VERSION"));

/// `QDP_AUTH_TOKEN`: the host's key is pinned and the device presents a
/// token.
const AUTH_TOKEN: i32 = 0;

/// `QDP_AUTH_PAIRED`: a host the device paired with.
const AUTH_PAIRED: i32 = 1;

/// `QDP_AUTH_PAIR`: pairing with the code the host shows.
const AUTH_PAIR: i32 = 2;

/// `QDP_AUTH_GATEWAY`: through the gateway, which vouches for the user.
const AUTH_GATEWAY: i32 = 3;

/// `QDP_DECODER_HARDWARE`: VideoToolbox only. Any value but this and
/// [`DECODER_SOFTWARE`] decodes in hardware when the device can.
const DECODER_HARDWARE: i32 = 1;

/// `QDP_DECODER_SOFTWARE`: rav1d.
const DECODER_SOFTWARE: i32 = 2;

/// The bytes of an RGBA pixel.
const RGBA_BYTES: usize = 4;

/// The smallest display side the host accepts, in pixels.
const MIN_DISPLAY_SIDE: u32 = 16;

/// The largest display side the host accepts, in pixels.
const MAX_DISPLAY_SIDE: u32 = 16_384;

/// Sets up logging once.
static INIT: Once = Once::new();

/// The app's private directory, where the device's key is kept.
static FILES_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// What a session connects with, as `QdpConfig` lays it out.
#[repr(C)]
pub struct QdpConfig {
    /// The host's address or name, with or without a port.
    host: *const c_char,
    /// A `QdpAuthentication` value.
    authentication: i32,
    /// The host's key pin, for token and paired authentication.
    pin: *const c_char,
    /// The token, for token authentication.
    token: *const c_char,
    /// The device's name, which the host shows when pairing.
    device_name: *const c_char,
    /// The width of the display the session asks for, in pixels.
    width: u32,
    /// Its height.
    height: u32,
    /// Its refresh rate, in millihertz.
    refresh_millihertz: u32,
    /// Its interface scale, in percent, or 0 for unscaled.
    scale_percent: u32,
    /// A `QdpDecoder` value.
    decoder: i32,
    /// Whether the microphone starts on.
    microphone: bool,
    /// The gateway's address, or empty or null to reach the host directly.
    gateway_address: *const c_char,
    /// The gateway's key pin, or empty or null to verify it by the system's
    /// certificate authorities.
    gateway_pin: *const c_char,
    /// The token of the last `gatewaysession` event, or empty or null.
    gateway_token: *const c_char,
}

/// The host's cursor image, as `QdpCursor` lays it out.
#[repr(C)]
pub struct QdpCursor {
    /// Its width in pixels.
    width: u32,
    /// Its height in pixels.
    height: u32,
    /// The hotspot's column.
    hotspot_x: u32,
    /// The hotspot's row.
    hotspot_y: u32,
    /// Its scale relative to the video, in percent.
    scale_percent: u32,
    /// Its RGBA pixels, which [`qdp_cursor_free`] frees.
    rgba: *mut u8,
}

/// The on-screen gamepad, which the app holds as `QdpVirtualGamepad *`.
pub struct QdpVirtualGamepad {
    /// Its controls and the touches holding them.
    gamepad: VirtualGamepad,
}

/// A control of the on-screen gamepad as the app draws it, as
/// `QdpVirtualControl` lays it out.
#[repr(C)]
pub struct QdpVirtualControl {
    /// Which control it is: a Standard Gamepad button index, 12 for the
    /// directional pad, or 17 and 18 for the left and right sticks.
    control: u32,
    /// Its frame's left edge, in the app's points.
    x: f32,
    /// Its frame's top edge.
    y: f32,
    /// Its frame's width.
    width: f32,
    /// Its frame's height.
    height: f32,
    /// Whether a touch holds it.
    held: bool,
    /// A stick's tilt across, from -1 to 1, negative left.
    tilt_x: f32,
    /// A stick's tilt down, from -1 to 1, negative up.
    tilt_y: f32,
    /// The directional pad's directions held: 1 up, 2 down, 4 left, 8 right.
    directions: u32,
}

/// A running session, which the app holds as `QdpSession *`.
pub struct QdpSession {
    /// The session.
    session: Session,
    /// The video's drawing, used on the main thread only.
    video: MetalVideo<FrameId>,
    /// What is drawn, shared with the session's thread.
    stage: SharedStage,
}

/// Runs `body`, returning `fallback` if it panics, rather than unwinding
/// into Swift.
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
/// not stopped.
unsafe fn session<'a>(session: *mut QdpSession) -> Option<&'a QdpSession> {
    // SAFETY: as the caller promises.
    unsafe { session.as_ref() }
}

/// Returns the touch phase the app numbers `phase`, as `QdpTouchPhase`
/// does.
fn touch_phase(phase: i32) -> Option<TouchPhase> {
    match phase {
        0 => Some(TouchPhase::Down),
        1 => Some(TouchPhase::Move),
        2 => Some(TouchPhase::Up),
        3 => Some(TouchPhase::Cancel),
        _ => None,
    }
}

/// Sends the on-screen gamepad's state to the session a handle points to.
///
/// # Safety
///
/// As for [`session`].
unsafe fn send_virtual_gamepad(handle: *mut QdpSession, gamepad: &VirtualGamepad) {
    let input = UserInput::Gamepad {
        slot: virtual_gamepad::SLOT,
        report: gamepad.state().report(),
    };
    // SAFETY: as the caller promises.
    unsafe { send(handle, input) };
}

/// Sends `input` to the session a handle points to.
///
/// # Safety
///
/// As for [`session`].
unsafe fn send(handle: *mut QdpSession, input: UserInput) {
    // SAFETY: as the caller promises.
    if let Some(handle) = unsafe { session(handle) } {
        handle.session.send(input);
    }
}

/// Returns the display a session asks for, each side kept to what hosts
/// accept.
fn display(width: u32, height: u32, refresh_millihertz: u32, scale_percent: u32) -> DisplaySize {
    let side = |pixels: u32| pixels.clamp(MIN_DISPLAY_SIDE, MAX_DISPLAY_SIDE);
    DisplaySize {
        width: side(width),
        height: side(height),
        refresh_millihertz,
        scale_percent: if scale_percent == 0 {
            DisplaySize::UNSCALED
        } else {
            scale_percent
        },
    }
}

/// Returns a layer size in whole pixels.
fn pixels(width: f64, height: f64) -> (u32, u32) {
    let side = |length: f64| length.max(0.0).round() as u32;
    (side(width), side(height))
}

/// Returns the device's key pin, generating the key the first time.
fn client_pin() -> Result<qdp_quic::KeyPin, String> {
    let directory = FILES_DIRECTORY
        .get()
        .ok_or_else(|| "qdp_init was not called".to_owned())?;
    let (identity, _) = TlsIdentity::load_or_generate(directory, IdentityFiles::CLIENT)
        .map_err(|error| error.to_string())?;
    identity.key_pin().map_err(|error| error.to_string())
}

/// Sets up logging and keeps the app's private directory.
///
/// # Safety
///
/// `files_directory` is a live NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_init(files_directory: *const c_char) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let directory = unsafe { read(files_directory) };
        let _ = FILES_DIRECTORY.set(PathBuf::from(directory));
        INIT.call_once(|| {
            let _ = tracing_subscriber::fmt()
                .with_writer(std::io::stderr)
                .with_ansi(false)
                .try_init();
            info!(version = env!("CARGO_PKG_VERSION"), "QDP core started");
        });
    });
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

/// Starts a session, drawing its video in a layer over `layer`.
///
/// # Safety
///
/// `config` points to a live `QdpConfig` whose strings are null or live,
/// `layer` to a live `CALayer`, `error` is null or writable, and this is
/// called on the main thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_start(
    config: *const QdpConfig,
    layer: *mut c_void,
    width: f64,
    height: f64,
    scale: f64,
    error: *mut *mut c_char,
) -> *mut QdpSession {
    guarded(ptr::null_mut(), || {
        // SAFETY: as the caller promises.
        let started = unsafe { start(config, layer, width, height, scale) };
        match started {
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
unsafe fn start(
    config: *const QdpConfig,
    layer: *mut c_void,
    width: f64,
    height: f64,
    scale: f64,
) -> Result<QdpSession, String> {
    // SAFETY: as the caller promises.
    let config = unsafe { config.as_ref() }.ok_or("no configuration")?;
    // SAFETY: the caller promises live strings or nulls.
    let (host, pin, token, device_name) = unsafe {
        (
            read(config.host),
            read(config.pin),
            read(config.token),
            read(config.device_name),
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
        AUTH_GATEWAY => Authentication::Gateway,
        other => return Err(format!("unknown authentication {other}")),
    };
    // SAFETY: the caller promises live strings or nulls.
    let gateway = unsafe {
        GatewayConfig::parse(
            &read(config.gateway_address),
            &read(config.gateway_pin),
            &read(config.gateway_token),
        )
    }?;
    let identity_directory = FILES_DIRECTORY
        .get()
        .cloned()
        .ok_or("qdp_init was not called")?;
    let layer = NonNull::new(layer).ok_or("there is no layer to draw in")?;
    let presentations = Presentations::default();
    // SAFETY: the caller promises a live CALayer and the main thread.
    let video =
        unsafe { MetalVideo::attach_to_layer(layer, scale, reporter(Arc::clone(&presentations))) }?;
    // The stage is the main thread's until the session starts, and takes
    // the session's cursor once it has.
    let stage: SharedStage = Arc::new(Mutex::new(Stage::new(
        video.submitter(),
        pixels(width, height),
        SharedCursor::default(),
    )));
    let session = Session::start(
        Config {
            client_name: CLIENT_NAME.to_owned(),
            host,
            authentication,
            gateway,
            identity_directory,
            display: display(
                config.width,
                config.height,
                config.refresh_millihertz,
                config.scale_percent,
            ),
            decoder: match config.decoder {
                DECODER_HARDWARE => CodecPreference::Hardware,
                DECODER_SOFTWARE => CodecPreference::Software,
                _ => CodecPreference::Auto,
            },
            microphone: if config.microphone {
                Microphone::On
            } else {
                Microphone::Off
            },
            client_audio: qdp_desktop::client_audio,
            clipboard: None,
            keyboard_info: None,
        },
        {
            let stage = Arc::clone(&stage);
            move || ApplePresenter::new(stage, presentations)
        },
    )
    .map_err(|error| format!("cannot start the session: {error}"))?;
    lock(&stage).set_cursor(session.shared_cursor());
    Ok(QdpSession {
        session,
        video,
        stage,
    })
}

/// Ends and frees a session.
///
/// # Safety
///
/// `session` is null or a handle [`qdp_session_start`] returned that was
/// not stopped, and this is called on the main thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_stop(session: *mut QdpSession) {
    guarded((), || {
        if session.is_null() {
            return;
        }
        // SAFETY: as the caller promises, the handle is ours to free.
        let handle = unsafe { Box::from_raw(session) };
        let QdpSession { session, video, .. } = *handle;
        session.stop();
        drop(video);
    });
}

/// Tells a session its layer's new size.
///
/// # Safety
///
/// As for [`qdp_session_stop`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_resize(
    session: *mut QdpSession,
    width: f64,
    height: f64,
    scale: f64,
) {
    guarded((), || {
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { self::session(session) } {
            handle.video.fit(scale);
            lock(&handle.stage).resize(pixels(width, height));
        }
    });
}

/// Returns a session's next event, or null if none came in time.
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
        let event = handle.session.events().next(timeout);
        if matches!(event, Some(qdp_app::events::Event::Cursor)) {
            lock(&handle.stage).cursor_changed();
        }
        event.map_or(ptr::null_mut(), |event| new_string(&event.to_string()))
    })
}

/// Copies the host's cursor, returning false while it is hidden.
///
/// # Safety
///
/// `session` is null or a live handle, and `cursor` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_cursor(
    session: *mut QdpSession,
    cursor: *mut QdpCursor,
) -> bool {
    guarded(false, || {
        // SAFETY: as the caller promises.
        let (Some(handle), Some(out)) = (unsafe { self::session(session) }, unsafe {
            cursor.as_mut()
        }) else {
            return false;
        };
        let Some(image) = handle.session.cursor() else {
            return false;
        };
        let length = image.width as usize * image.height as usize * RGBA_BYTES;
        let mut pixels = image.pixels;
        pixels.resize(length, 0);
        *out = QdpCursor {
            width: image.width,
            height: image.height,
            hotspot_x: image.hotspot_x,
            hotspot_y: image.hotspot_y,
            scale_percent: u32::try_from(image.scale.into_inner()).unwrap_or(u32::MAX),
            rgba: Box::into_raw(pixels.into_boxed_slice()).cast::<u8>(),
        };
        true
    })
}

/// Frees a cursor's pixels.
///
/// # Safety
///
/// `cursor` is null or one [`qdp_session_cursor`] filled, not freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_cursor_free(cursor: *mut QdpCursor) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let Some(cursor) = (unsafe { cursor.as_mut() }) else {
            return;
        };
        if cursor.rgba.is_null() {
            return;
        }
        let length = cursor.width as usize * cursor.height as usize * RGBA_BYTES;
        // SAFETY: the pixels are the boxed slice of that length
        // `qdp_session_cursor` leaked.
        drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(cursor.rgba, length)) });
        cursor.rgba = ptr::null_mut();
    });
}

/// Draws the host's cursor over the video at the pointer, or stops.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_draw_cursor(session: *mut QdpSession, draw: bool) {
    guarded((), || {
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { self::session(session) } {
            lock(&handle.stage).set_draw_cursor(draw);
        }
    });
}

/// Moves the cursor drawn over the video to `x, y`, without moving the
/// host's pointer, as after a warp.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_cursor_at(session: *mut QdpSession, x: f32, y: f32) {
    guarded((), || {
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { self::session(session) } {
            lock(&handle.stage).move_pointer(x.into(), y.into());
        }
    });
}

/// Answers a request for the pairing code.
///
/// # Safety
///
/// `session` is null or a live handle, and `code` null or a live string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_pairing_code(session: *mut QdpSession, code: *const c_char) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let code = (!code.is_null()).then(|| unsafe { read(code) });
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { self::session(session) } {
            handle.session.answer_pairing(code);
        }
    });
}

/// Answers the gateway's request for the user's name and password with
/// `user` and `password`, or declines with a null user, which ends the
/// session.
///
/// # Safety
///
/// `session` is null or a live handle, and `user` and `password` null or
/// live strings.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_sign_in(
    session: *mut QdpSession,
    user: *const c_char,
    password: *const c_char,
) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let answer = (!user.is_null()).then(|| unsafe {
            SignInAnswer::Credentials(Credentials {
                user: read(user),
                password: read(password),
            })
        });
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { self::session(session) } {
            handle.session.answer_sign_in(answer);
        }
    });
}

/// Answers the gateway's request for a code from the user's authenticator
/// app with `code`, or declines with null, which ends the session.
///
/// # Safety
///
/// `session` is null or a live handle, and `code` null or a live string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_sign_in_code(session: *mut QdpSession, code: *const c_char) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let code = (!code.is_null()).then(|| unsafe { read(code) });
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { self::session(session) } {
            handle.session.answer_sign_in(code.map(SignInAnswer::Code));
        }
    });
}

/// Tells the host the display changed.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_display(
    session: *mut QdpSession,
    width: u32,
    height: u32,
    refresh_millihertz: u32,
    scale_percent: u32,
) {
    guarded((), || {
        let display = display(width, height, refresh_millihertz, scale_percent);
        // SAFETY: as the caller promises.
        unsafe { send(session, UserInput::Display(display)) };
    });
}

/// Sends a key by its HID usage on the keyboard page.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_key(session: *mut QdpSession, usage: u16, pressed: bool) {
    guarded((), || {
        let Ok(usage) = u8::try_from(usage) else {
            return;
        };
        // SAFETY: as the caller promises.
        unsafe { send(session, UserInput::Key { usage, pressed }) };
    });
}

/// Returns the HID usage of a macOS virtual key code, or 0 for none.
#[unsafe(no_mangle)]
pub extern "C" fn qdp_mac_key_usage(key_code: u16) -> u16 {
    #[cfg(target_os = "macos")]
    {
        qdp_desktop::mac_key_usage(key_code).map_or(0, u16::from)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = key_code;
        0
    }
}

/// Sends text typed without key presses.
///
/// # Safety
///
/// `session` is null or a live handle, and `text` null or a live string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_text(session: *mut QdpSession, text: *const c_char) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let text = unsafe { read(text) };
        if !text.is_empty() {
            // SAFETY: as the caller promises.
            unsafe { send(session, UserInput::Text(text)) };
        }
    });
}

/// Moves the pointer to `x, y`, and the cursor drawn with it.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_pointer(session: *mut QdpSession, x: f32, y: f32) {
    guarded((), || {
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { self::session(session) } {
            handle.session.send(UserInput::PointerMoved {
                x: x.into(),
                y: y.into(),
            });
            lock(&handle.stage).move_pointer(x.into(), y.into());
        }
    });
}

/// Moves the captured mouse by `dx, dy`.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_mouse_move(session: *mut QdpSession, dx: f32, dy: f32) {
    guarded((), || {
        let input = UserInput::MouseMoved {
            dx: dx.into(),
            dy: dy.into(),
        };
        // SAFETY: as the caller promises.
        unsafe { send(session, input) };
    });
}

/// Presses or releases a pointer button.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_button(session: *mut QdpSession, button: i32, pressed: bool) {
    guarded((), || {
        let button = match button {
            0 => PointerButton::Left,
            1 => PointerButton::Right,
            2 => PointerButton::Middle,
            3 => PointerButton::Back,
            4 => PointerButton::Forward,
            _ => return,
        };
        // SAFETY: as the caller promises.
        unsafe { send(session, UserInput::Button { button, pressed }) };
    });
}

/// Scrolls by 1/120ths of a detent, positive up and right.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_scroll(
    session: *mut QdpSession,
    vertical: i32,
    horizontal: i32,
) {
    guarded((), || {
        let input = UserInput::Scroll {
            vertical,
            horizontal,
        };
        // SAFETY: as the caller promises.
        unsafe { send(session, input) };
    });
}

/// Sends a finger's touch.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_touch(
    session: *mut QdpSession,
    finger: u32,
    phase: i32,
    x: f32,
    y: f32,
    pressure: f32,
) {
    guarded((), || {
        let Some(phase) = touch_phase(phase) else {
            return;
        };
        let input = UserInput::Touch {
            finger: finger.into(),
            phase,
            x: x.into(),
            y: y.into(),
            pressure: (pressure >= 0.0).then(|| f64::from(pressure.min(1.0))),
        };
        // SAFETY: as the caller promises.
        unsafe { send(session, input) };
    });
}

/// Connects a gamepad.
///
/// # Safety
///
/// `session` is null or a live handle, and `name` null or a live string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_gamepad_connected(
    session: *mut QdpSession,
    slot: u32,
    name: *const c_char,
    rumble: bool,
) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let name = unsafe { read(name) };
        let features = if rumble {
            DeviceFeatures::default().union(DeviceFeatures::GAMEPAD_RUMBLE)
        } else {
            DeviceFeatures::default()
        };
        let input = UserInput::GamepadConnected {
            slot,
            features,
            name,
        };
        // SAFETY: as the caller promises.
        unsafe { send(session, input) };
    });
}

/// Disconnects a gamepad.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_gamepad_disconnected(session: *mut QdpSession, slot: u32) {
    guarded((), || {
        // SAFETY: as the caller promises.
        unsafe { send(session, UserInput::GamepadDisconnected { slot }) };
    });
}

/// Sends a gamepad's state.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn qdp_session_gamepad(
    session: *mut QdpSession,
    slot: u32,
    buttons: u32,
    left_x: f32,
    left_y: f32,
    right_x: f32,
    right_y: f32,
    left_trigger: f32,
    right_trigger: f32,
) {
    guarded((), || {
        let state = GamepadState {
            buttons,
            left_stick: (left_x, left_y),
            right_stick: (right_x, right_y),
            left_trigger,
            right_trigger,
        };
        let input = UserInput::Gamepad {
            slot,
            report: state.report(),
        };
        // SAFETY: as the caller promises.
        unsafe { send(session, input) };
    });
}

/// Returns a new on-screen gamepad, not laid out yet, which the app frees
/// with [`qdp_virtual_gamepad_free`].
#[unsafe(no_mangle)]
pub extern "C" fn qdp_virtual_gamepad_new() -> *mut QdpVirtualGamepad {
    guarded(ptr::null_mut(), || {
        Box::into_raw(Box::new(QdpVirtualGamepad {
            gamepad: VirtualGamepad::new(),
        }))
    })
}

/// Frees an on-screen gamepad.
///
/// # Safety
///
/// `gamepad` is null or a live handle from [`qdp_virtual_gamepad_new`], not
/// used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_virtual_gamepad_free(gamepad: *mut QdpVirtualGamepad) {
    if !gamepad.is_null() {
        // SAFETY: as the caller promises.
        drop(unsafe { Box::from_raw(gamepad) });
    }
}

/// Connects the on-screen gamepad to the session, as its own gamepad, or
/// disconnects it with every control released.
///
/// # Safety
///
/// `gamepad` and `session` are null or live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_virtual_gamepad_connect(
    gamepad: *mut QdpVirtualGamepad,
    session: *mut QdpSession,
    connected: bool,
) {
    guarded((), || {
        let input = if connected {
            UserInput::GamepadConnected {
                slot: virtual_gamepad::SLOT,
                features: DeviceFeatures::default(),
                name: virtual_gamepad::NAME.to_owned(),
            }
        } else {
            // SAFETY: as the caller promises.
            if let Some(handle) = unsafe { gamepad.as_mut() } {
                handle.gamepad.release_all();
            }
            UserInput::GamepadDisconnected {
                slot: virtual_gamepad::SLOT,
            }
        };
        // SAFETY: as the caller promises.
        unsafe { send(session, input) };
    });
}

/// Lays the on-screen gamepad out on a view `width × height` points, inside
/// the safe area the insets leave, releasing every control held.
///
/// # Safety
///
/// `gamepad` and `session` are null or live handles.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn qdp_virtual_gamepad_lay_out(
    gamepad: *mut QdpVirtualGamepad,
    session: *mut QdpSession,
    width: f32,
    height: f32,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
) {
    guarded((), || {
        // SAFETY: as the caller promises.
        let Some(handle) = (unsafe { gamepad.as_mut() }) else {
            return;
        };
        let screen = Screen {
            width,
            height,
            // UIKit's points are density-independent already.
            density: 1.0,
            insets: Insets {
                left,
                top,
                right,
                bottom,
            },
        };
        if handle.gamepad.lay_out(&screen) {
            // SAFETY: as the caller promises.
            unsafe { send_virtual_gamepad(session, &handle.gamepad) };
        }
    });
}

/// Takes touch `finger`'s `phase` at `x, y` in the view's points on the
/// on-screen gamepad, sending its state to the session if that changed it.
/// Returns what changed: 1 if the controls look different, plus 2 if a
/// button or direction was pressed.
///
/// # Safety
///
/// `gamepad` and `session` are null or live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_virtual_gamepad_touch(
    gamepad: *mut QdpVirtualGamepad,
    session: *mut QdpSession,
    finger: u32,
    phase: i32,
    x: f32,
    y: f32,
) -> u32 {
    guarded(0, || {
        // SAFETY: as the caller promises.
        let (Some(handle), Some(phase)) = (unsafe { gamepad.as_mut() }, touch_phase(phase)) else {
            return 0;
        };
        let outcome = handle.gamepad.touch(finger, phase, Point { x, y });
        if outcome.state_changed {
            // SAFETY: as the caller promises.
            unsafe { send_virtual_gamepad(session, &handle.gamepad) };
        }
        outcome.flags()
    })
}

/// Releases every control of the on-screen gamepad held, as when the
/// stream's menu opens.
///
/// # Safety
///
/// `gamepad` and `session` are null or live handles.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_virtual_gamepad_release(
    gamepad: *mut QdpVirtualGamepad,
    session: *mut QdpSession,
) {
    guarded((), || {
        // SAFETY: as the caller promises.
        if let Some(handle) = unsafe { gamepad.as_mut() }
            && handle.gamepad.release_all()
        {
            // SAFETY: as the caller promises.
            unsafe { send_virtual_gamepad(session, &handle.gamepad) };
        }
    });
}

/// Writes up to `capacity` of the on-screen gamepad's controls, as the app
/// draws them, to `controls`, returning how many there are.
///
/// # Safety
///
/// `gamepad` is null or a live handle, and `controls` null or room for
/// `capacity` controls.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_virtual_gamepad_controls(
    gamepad: *mut QdpVirtualGamepad,
    controls: *mut QdpVirtualControl,
    capacity: usize,
) -> usize {
    guarded(0, || {
        // SAFETY: as the caller promises.
        let Some(handle) = (unsafe { gamepad.as_ref() }) else {
            return 0;
        };
        let views = handle.gamepad.controls();
        if !controls.is_null() {
            for (index, view) in views.iter().take(capacity).enumerate() {
                let control = QdpVirtualControl {
                    control: view.control.code(),
                    x: view.frame.x,
                    y: view.frame.y,
                    width: view.frame.width,
                    height: view.frame.height,
                    held: view.held,
                    tilt_x: view.tilt.0,
                    tilt_y: view.tilt.1,
                    directions: view.directions,
                };
                // SAFETY: the caller promises room for `capacity` controls,
                // and `index` is below it.
                unsafe { controls.add(index).write(control) };
            }
        }
        views.len()
    })
}

/// Tells the host whether the app has focus; losing it releases everything.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_focus(session: *mut QdpSession, focused: bool) {
    guarded((), || {
        // SAFETY: as the caller promises.
        unsafe {
            if focused {
                send(session, UserInput::Focus(Focus::Focused));
            } else {
                send(session, UserInput::Focus(Focus::Unfocused));
                send(session, UserInput::ReleaseAll);
            }
        }
    });
}

/// Turns the microphone on or off.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_microphone(session: *mut QdpSession, on: bool) {
    guarded((), || {
        let state = if on { Microphone::On } else { Microphone::Off };
        // SAFETY: as the caller promises.
        unsafe { send(session, UserInput::Microphone(state)) };
    });
}

/// Releases every key and button held.
///
/// # Safety
///
/// `session` is null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_session_release_all(session: *mut QdpSession) {
    guarded((), || {
        // SAFETY: as the caller promises.
        unsafe { send(session, UserInput::ReleaseAll) };
    });
}

/// Frees a string the core returned.
///
/// # Safety
///
/// `string` is null or a string the core returned, not freed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qdp_string_free(string: *mut c_char) {
    guarded((), || {
        if !string.is_null() {
            // SAFETY: as the caller promises, it is a `CString` the core
            // leaked.
            drop(unsafe { CString::from_raw(string) });
        }
    });
}
