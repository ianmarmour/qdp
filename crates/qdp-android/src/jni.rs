//! The functions the app's `io.github.ianmarmour.qdp.Native` class calls.
//!
//! The app runs one session at a time: `connect` starts it in the app's
//! video surface, the input functions send the user's input to it, a thread
//! of the app's waits for its events with `nextEvent`, and `disconnect` ends
//! it before the surface goes. Input sent while no session runs is dropped.
//! JNI is a C interface, so this module uses `unsafe`.

#![allow(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr;
use std::sync::{Mutex, MutexGuard, Once, OnceLock, PoisonError};
use std::time::Duration;

use jni_sys::{
    JNI_FALSE, JNI_OK, JNI_TRUE, JNIEnv, JavaVM, jboolean, jclass, jfloat, jfloatArray, jint,
    jintArray, jobject, jsize, jstring,
};
use ndk::native_window::NativeWindow;
use qdp::message::DeviceFeatures;
use qdp_codec::{CodecPreference, set_video_surface};
use qdp_quic::{IdentityFiles, TlsIdentity};
use qdp_runtime::{Focus, Microphone, PointerButton, TouchPhase, UserInput};
use tracing::{info, warn};

use qdp_app::gamepad::GamepadState;
use qdp_app::qr::QrModules;
use qdp_app::session::{Authentication, Config, GatewayConfig, Session};
use qdp_app::settings::{display_size, format_pin, host_reachable, parse_pin};
use qdp_app::virtual_gamepad::{self, Insets, Point, Screen, VirtualGamepad};
use qdp_runtime::{Credentials, SignInAnswer};

use crate::cursor::CursorBitmap;
use crate::keymap::{HidKey, hid_key};
use crate::logcat;
use crate::sink::AndroidPresenter;

/// The `decoder` value of `connect` asking for MediaCodec, failing without
/// it; any other value but [`DECODER_SOFTWARE`] decodes in hardware when the
/// device can, and in software otherwise.
const DECODER_HARDWARE: jint = 1;

/// The `decoder` value of `connect` asking for the software decoder.
const DECODER_SOFTWARE: jint = 2;

/// The product name the app sends hosts in `CLIENT_HELLO`.
const CLIENT_NAME: &str = concat!("qdp-android/", env!("CARGO_PKG_VERSION"));

/// The `authentication` value of `connect` presenting a token.
const AUTH_TOKEN: jint = 0;

/// The `authentication` value of `connect` for a host the device paired
/// with, which accepts the device's key.
const AUTH_PAIRED: jint = 1;

/// The `authentication` value of `connect` pairing with the code the host
/// shows.
const AUTH_PAIR: jint = 2;

/// `connect` reaches the host through the gateway, which vouches for the
/// user (Section 13.7).
const AUTH_GATEWAY: jint = 3;

/// The session, while one runs.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

/// The on-screen gamepad, which the stream's menu shows and hides.
static VIRTUAL_GAMEPAD: Mutex<VirtualGamepad> = Mutex::new(VirtualGamepad::new());

/// The app's private files directory, where the device's key is kept.
static FILES_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

/// Sets up logging and the app's context once.
static INIT: Once = Once::new();

/// Calls JNI function `$function` of `$env` with the arguments after it.
macro_rules! jni {
    ($env:expr, $function:ident $(, $argument:expr)* $(,)?) => {
        ((**$env)
            .$function
            .expect(concat!("JNI has ", stringify!($function))))($env $(, $argument)*)
    };
}

/// Runs `body`, returning `fallback` if it panics, which is logged, rather
/// than unwinding into the Java VM.
fn guarded<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(fallback)
}

/// Returns whether a Java boolean is true.
fn flag(value: jboolean) -> bool {
    value != JNI_FALSE
}

/// Returns a Java boolean.
fn boolean(value: bool) -> jboolean {
    if value { JNI_TRUE } else { JNI_FALSE }
}

/// Sends `input` to the session, if one runs.
fn send(input: UserInput) {
    if let Some(session) = SESSION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
    {
        session.send(input);
    }
}

/// Ends the session, if one runs, returning once it has.
fn stop_session() {
    let session = SESSION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(session) = session {
        session.stop();
    }
}

/// Reads a Java string, or returns an empty one for `null`.
///
/// # Safety
///
/// `env` is the calling thread's JNI environment and `string` a live
/// reference to a string or null.
unsafe fn read_string(env: *mut JNIEnv, string: jstring) -> String {
    if string.is_null() {
        return String::new();
    }
    // SAFETY: as the caller promises.
    unsafe {
        let length = jni!(env, GetStringLength, string);
        let mut units = vec![0; usize::try_from(length).unwrap_or(0)];
        jni!(env, GetStringRegion, string, 0, length, units.as_mut_ptr());
        String::from_utf16_lossy(&units)
    }
}

/// Returns `text` as a new Java string.
///
/// # Safety
///
/// `env` is the calling thread's JNI environment.
unsafe fn new_string(env: *mut JNIEnv, text: &str) -> jstring {
    let units: Vec<u16> = text.encode_utf16().collect();
    let length = jsize::try_from(units.len()).unwrap_or(jsize::MAX);
    // SAFETY: as the caller promises; `units` holds `length` code units.
    unsafe { jni!(env, NewString, units.as_ptr(), length) }
}

/// Returns a Java integer as a count, or 0 if it is negative.
fn count(value: jint) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

/// Returns the touch phase the app numbers `phase`: touching (0), moving
/// (1), lifting (2) or cancelled (3).
fn touch_phase(phase: jint) -> Option<TouchPhase> {
    match phase {
        0 => Some(TouchPhase::Down),
        1 => Some(TouchPhase::Move),
        2 => Some(TouchPhase::Up),
        3 => Some(TouchPhase::Cancel),
        _ => None,
    }
}

/// Returns the on-screen gamepad, which outlives sessions as the app's
/// stream screen does.
fn virtual_gamepad() -> MutexGuard<'static, VirtualGamepad> {
    VIRTUAL_GAMEPAD
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Sends the on-screen gamepad's state to the session.
fn send_virtual_gamepad(gamepad: &VirtualGamepad) {
    send(UserInput::Gamepad {
        slot: virtual_gamepad::SLOT,
        report: gamepad.state().report(),
    });
}

/// Sets up logging, gives the audio library the app's context, and keeps
/// the app's private files directory, `files_directory`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_init(
    env: *mut JNIEnv,
    _class: jclass,
    context: jobject,
    files_directory: jstring,
) {
    guarded((), || {
        // SAFETY: the VM calls this with its environment and a live
        // reference.
        let directory = unsafe { read_string(env, files_directory) };
        let _ = FILES_DIRECTORY.set(PathBuf::from(directory));
        INIT.call_once(|| {
            logcat::init();
            let mut vm: *mut JavaVM = ptr::null_mut();
            // SAFETY: the VM calls this with its environment and a live
            // reference to the app's context, kept by the global
            // reference for the life of the process.
            unsafe {
                let status = jni!(env, GetJavaVM, &mut vm);
                let context = jni!(env, NewGlobalRef, context);
                if status == JNI_OK && !vm.is_null() && !context.is_null() {
                    ndk_context::initialize_android_context(vm.cast(), context.cast());
                } else {
                    warn!("cannot keep the app's context; audio is unavailable");
                }
            }
            info!(version = env!("CARGO_PKG_VERSION"), "QDP core started");
        });
    });
}

/// Returns the app's private files directory, once `init` gave it.
fn files_directory() -> Result<PathBuf, String> {
    FILES_DIRECTORY
        .get()
        .cloned()
        .ok_or_else(|| "the core was not given the app's files directory".to_owned())
}

/// Returns how a session authenticates: the `authentication` value of
/// `connect`, the host's key pin `pin` and the token `token` where it takes
/// them, and the device's name `device_name` for pairing.
fn authentication(
    authentication: jint,
    pin: &str,
    token: String,
    device_name: String,
) -> Result<Authentication, String> {
    match authentication {
        AUTH_TOKEN => Ok(Authentication::Token {
            pin: parse_pin(pin)?,
            token,
        }),
        AUTH_PAIRED => Ok(Authentication::Paired {
            pin: parse_pin(pin)?,
        }),
        AUTH_PAIR => Ok(Authentication::Pair { device_name }),
        AUTH_GATEWAY => Ok(Authentication::Gateway),
        other => Err(format!("unknown authentication {other}")),
    }
}

/// Starts a session with the host at `host`, authenticating as
/// `authentication` says (one of [`AUTH_TOKEN`], [`AUTH_PAIRED`],
/// [`AUTH_PAIR`] and [`AUTH_GATEWAY`]) with the host's key pin `pin`, `token`
/// and the device's name `device_name` where it takes them, through the
/// gateway at `gateway_address`, if not empty, verified by `gateway_pin`, if
/// not empty, signing in with `gateway_token`, a `gatewaysession` event's
/// token, if not empty; showing the video in `surface` on a display of
/// `width × height` pixels refreshing at `refresh_millihertz`. Returns
/// `null` once it has started, or why it could not.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_connect(
    env: *mut JNIEnv,
    _class: jclass,
    surface: jobject,
    host: jstring,
    authentication_kind: jint,
    pin: jstring,
    token: jstring,
    device_name: jstring,
    gateway_address: jstring,
    gateway_pin: jstring,
    gateway_token: jstring,
    width: jint,
    height: jint,
    refresh_millihertz: jint,
    decoder: jint,
    microphone: jboolean,
) -> jstring {
    guarded(ptr::null_mut(), || {
        // SAFETY: the VM calls this with its environment and live
        // references.
        let (host, pin, token, device_name, window) = unsafe {
            (
                read_string(env, host),
                read_string(env, pin),
                read_string(env, token),
                read_string(env, device_name),
                NativeWindow::from_surface(env, surface),
            )
        };
        // SAFETY: as above.
        let gateway = unsafe {
            GatewayConfig::parse(
                &read_string(env, gateway_address),
                &read_string(env, gateway_pin),
                &read_string(env, gateway_token),
            )
        };
        let config = authentication(authentication_kind, &pin, token, device_name).and_then(
            |authentication| {
                Ok(Config {
                    client_name: CLIENT_NAME.to_owned(),
                    host,
                    authentication,
                    gateway: gateway?,
                    identity_directory: files_directory()?,
                    display: display_size(count(width), count(height), count(refresh_millihertz)),
                    decoder: match decoder {
                        DECODER_HARDWARE => CodecPreference::Hardware,
                        DECODER_SOFTWARE => CodecPreference::Software,
                        _ => CodecPreference::Auto,
                    },
                    microphone: if flag(microphone) {
                        Microphone::On
                    } else {
                        Microphone::Off
                    },
                    client_audio: qdp_desktop::client_audio,
                    clipboard: None,
                    keyboard_info: None,
                })
            },
        );
        let started = config.and_then(|config| {
            let window = window.ok_or("the surface has no window")?;
            stop_session();
            set_video_surface(Some(window));
            let session = Session::start(config, AndroidPresenter::new)
                .map_err(|error| format!("cannot start the session: {error}"))?;
            *SESSION.lock().unwrap_or_else(PoisonError::into_inner) = Some(session);
            Ok(())
        });
        match started {
            Ok(()) => ptr::null_mut(),
            // SAFETY: as above.
            Err(error) => unsafe { new_string(env, &error) },
        }
    })
}

/// Ends the session, if one runs, returning once the video surface is no
/// longer used.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_disconnect(
    _env: *mut JNIEnv,
    _class: jclass,
) {
    guarded((), || {
        stop_session();
        set_video_surface(None);
    });
}

/// Returns the session's next event, as `events` describes, waiting up to
/// `timeout_millis` for one, or `null` if none came or no session runs; with
/// no session it waits out the timeout, so that the app's loop does not
/// spin.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_nextEvent(
    env: *mut JNIEnv,
    _class: jclass,
    timeout_millis: jint,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let events = SESSION
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(Session::events);
        let timeout = Duration::from_millis(count(timeout_millis).into());
        let Some(events) = events else {
            std::thread::sleep(timeout);
            return ptr::null_mut();
        };
        match events.next(timeout) {
            // SAFETY: the VM calls this with its environment.
            Some(event) => unsafe { new_string(env, &event.to_string()) },
            None => ptr::null_mut(),
        }
    })
}

/// Returns the host's cursor: its width, height, hotspot column and hotspot
/// row, then its pixels as `0xAARRGGBB`, or `null` while it is hidden.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_cursorImage(
    env: *mut JNIEnv,
    _class: jclass,
) -> jintArray {
    guarded(ptr::null_mut(), || {
        let cursor = SESSION
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(Session::cursor);
        let Some(cursor) = cursor else {
            return ptr::null_mut();
        };
        let ints = CursorBitmap::new(&cursor).to_ints();
        let Ok(length) = jsize::try_from(ints.len()) else {
            return ptr::null_mut();
        };
        // SAFETY: the VM calls this with its environment; the array holds
        // `length` integers.
        unsafe {
            let array = jni!(env, NewIntArray, length);
            if !array.is_null() {
                jni!(env, SetIntArrayRegion, array, 0, length, ints.as_ptr());
            }
            array
        }
    })
}

/// Sends a key press or release, returning whether the key is one QDP sends.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_key(
    _env: *mut JNIEnv,
    _class: jclass,
    key_code: jint,
    pressed: jboolean,
) -> jboolean {
    guarded(JNI_FALSE, || {
        let pressed = flag(pressed);
        let Some(key) = hid_key(key_code) else {
            return JNI_FALSE;
        };
        send(match key {
            HidKey::Keyboard(usage) => UserInput::Key { usage, pressed },
            HidKey::Consumer(usage) => UserInput::ConsumerKey { usage, pressed },
        });
        JNI_TRUE
    })
}

/// Sends text committed without key presses, as by the on-screen keyboard.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_text(
    env: *mut JNIEnv,
    _class: jclass,
    text: jstring,
) {
    guarded((), || {
        // SAFETY: the VM calls this with its environment and a live
        // reference.
        let text = unsafe { read_string(env, text) };
        if !text.is_empty() {
            send(UserInput::Text(text));
        }
    });
}

/// Moves the pointer to `x, y` in video pixels.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_pointer(
    _env: *mut JNIEnv,
    _class: jclass,
    x: jfloat,
    y: jfloat,
) {
    guarded((), || {
        send(UserInput::PointerMoved {
            x: x.into(),
            y: y.into(),
        });
    });
}

/// Moves the mouse by `dx, dy` counts, as a captured pointer reports.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_mouseMove(
    _env: *mut JNIEnv,
    _class: jclass,
    dx: jfloat,
    dy: jfloat,
) {
    guarded((), || {
        send(UserInput::MouseMoved {
            dx: dx.into(),
            dy: dy.into(),
        });
    });
}

/// Presses or releases a pointer button: 0 left, 1 right, 2 middle, 3 back
/// and 4 forward.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_button(
    _env: *mut JNIEnv,
    _class: jclass,
    button: jint,
    pressed: jboolean,
) {
    guarded((), || {
        let button = match button {
            0 => PointerButton::Left,
            1 => PointerButton::Right,
            2 => PointerButton::Middle,
            3 => PointerButton::Back,
            4 => PointerButton::Forward,
            _ => return,
        };
        send(UserInput::Button {
            button,
            pressed: flag(pressed),
        });
    });
}

/// Scrolls by `vertical` and `horizontal` 1/120ths of a wheel detent,
/// positive away from the user and to the right.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_scroll(
    _env: *mut JNIEnv,
    _class: jclass,
    vertical: jint,
    horizontal: jint,
) {
    guarded((), || {
        send(UserInput::Scroll {
            vertical,
            horizontal,
        });
    });
}

/// Sends a finger touching (phase 0), moving (1), lifting (2) or cancelled
/// (3) at `x, y` in video pixels, pressing with `pressure` from 0 to 1, or a
/// negative pressure if the screen does not sense it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_touch(
    _env: *mut JNIEnv,
    _class: jclass,
    finger: jint,
    phase: jint,
    x: jfloat,
    y: jfloat,
    pressure: jfloat,
) {
    guarded((), || {
        let Some(phase) = touch_phase(phase) else {
            return;
        };
        send(UserInput::Touch {
            finger: count(finger).into(),
            phase,
            x: x.into(),
            y: y.into(),
            pressure: (pressure >= 0.0).then(|| f64::from(pressure.min(1.0))),
        });
    });
}

/// Connects gamepad `slot`, named `name`, which can rumble if `rumble`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_gamepadConnected(
    env: *mut JNIEnv,
    _class: jclass,
    slot: jint,
    name: jstring,
    rumble: jboolean,
) {
    guarded((), || {
        // SAFETY: the VM calls this with its environment and a live
        // reference.
        let name = unsafe { read_string(env, name) };
        let features = if flag(rumble) {
            DeviceFeatures::default().union(DeviceFeatures::GAMEPAD_RUMBLE)
        } else {
            DeviceFeatures::default()
        };
        send(UserInput::GamepadConnected {
            slot: count(slot),
            features,
            name,
        });
    });
}

/// Disconnects gamepad `slot`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_gamepadDisconnected(
    _env: *mut JNIEnv,
    _class: jclass,
    slot: jint,
) {
    guarded((), || {
        send(UserInput::GamepadDisconnected { slot: count(slot) });
    });
}

/// Sends gamepad `slot`'s state: its held buttons, a mask of Standard
/// Gamepad button indices, its sticks from -1 to 1, negative left and up,
/// and its triggers from 0 to 1.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_gamepad(
    _env: *mut JNIEnv,
    _class: jclass,
    slot: jint,
    buttons: jint,
    left_x: jfloat,
    left_y: jfloat,
    right_x: jfloat,
    right_y: jfloat,
    left_trigger: jfloat,
    right_trigger: jfloat,
) {
    guarded((), || {
        let state = GamepadState {
            buttons: buttons.cast_unsigned(),
            left_stick: (left_x, left_y),
            right_stick: (right_x, right_y),
            left_trigger,
            right_trigger,
        };
        send(UserInput::Gamepad {
            slot: count(slot),
            report: state.report(),
        });
    });
}

/// The numbers [`Java_io_github_ianmarmour_qdp_Native_virtualGamepadControls`]
/// gives each control.
const VIRTUAL_CONTROL_FLOATS: usize = 9;

/// Connects the on-screen gamepad to the session, as its own gamepad, or
/// disconnects it with every control released.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_virtualGamepadConnected(
    _env: *mut JNIEnv,
    _class: jclass,
    connected: jboolean,
) {
    guarded((), || {
        if flag(connected) {
            send(UserInput::GamepadConnected {
                slot: virtual_gamepad::SLOT,
                features: DeviceFeatures::default(),
                name: virtual_gamepad::NAME.to_owned(),
            });
        } else {
            virtual_gamepad().release_all();
            send(UserInput::GamepadDisconnected {
                slot: virtual_gamepad::SLOT,
            });
        }
    });
}

/// Lays the on-screen gamepad out on a screen `width × height` pixels,
/// `density` pixels a dp, inside the safe area the insets leave, releasing
/// every control held.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_virtualGamepadLayout(
    _env: *mut JNIEnv,
    _class: jclass,
    width: jint,
    height: jint,
    density: jfloat,
    left: jint,
    top: jint,
    right: jint,
    bottom: jint,
) {
    guarded((), || {
        // Screen sizes and insets are far below f32's exact integers.
        #[allow(clippy::cast_precision_loss)]
        let screen = Screen {
            width: width as f32,
            height: height as f32,
            density,
            insets: Insets {
                left: left as f32,
                top: top as f32,
                right: right as f32,
                bottom: bottom as f32,
            },
        };
        let mut gamepad = virtual_gamepad();
        if gamepad.lay_out(&screen) {
            send_virtual_gamepad(&gamepad);
        }
    });
}

/// Takes touch `finger`'s `phase`, numbered as [`touch_phase`], at `x, y`
/// on the screen, in pixels, on the on-screen gamepad, sending its state if
/// that changed it. Returns what changed, as [`virtual_gamepad::REDRAW`] and
/// [`virtual_gamepad::PRESSED`] bits.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_virtualGamepadTouch(
    _env: *mut JNIEnv,
    _class: jclass,
    finger: jint,
    phase: jint,
    x: jfloat,
    y: jfloat,
) -> jint {
    guarded(0, || {
        let Some(phase) = touch_phase(phase) else {
            return 0;
        };
        let mut gamepad = virtual_gamepad();
        let outcome = gamepad.touch(count(finger), phase, Point { x, y });
        if outcome.state_changed {
            send_virtual_gamepad(&gamepad);
        }
        jint::try_from(outcome.flags()).unwrap_or(0)
    })
}

/// Releases every control of the on-screen gamepad held, as when the
/// stream's menu opens.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_virtualGamepadRelease(
    _env: *mut JNIEnv,
    _class: jclass,
) {
    guarded((), || {
        let mut gamepad = virtual_gamepad();
        if gamepad.release_all() {
            send_virtual_gamepad(&gamepad);
        }
    });
}

/// Returns the on-screen gamepad's controls as the app draws them,
/// [`VIRTUAL_CONTROL_FLOATS`] numbers each: its code, its frame's left,
/// top, width and height in pixels, 1 if held and 0 if not, a stick's tilt
/// across and down, and the directional pad's directions held.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_virtualGamepadControls(
    env: *mut JNIEnv,
    _class: jclass,
) -> jfloatArray {
    guarded(ptr::null_mut(), || {
        // Codes and directions are below 32, which f32 holds exactly.
        #[allow(clippy::cast_precision_loss)]
        let floats: Vec<jfloat> = virtual_gamepad()
            .controls()
            .iter()
            .flat_map(|view| {
                let frame = view.frame;
                let held = if view.held { 1.0 } else { 0.0 };
                [
                    view.control.code() as f32,
                    frame.x,
                    frame.y,
                    frame.width,
                    frame.height,
                    held,
                    view.tilt.0,
                    view.tilt.1,
                    view.directions as f32,
                ]
            })
            .collect();
        debug_assert_eq!(floats.len() % VIRTUAL_CONTROL_FLOATS, 0);
        let Ok(length) = jsize::try_from(floats.len()) else {
            return ptr::null_mut();
        };
        // SAFETY: the VM calls this with its environment; the array holds
        // `length` floats.
        unsafe {
            let array = jni!(env, NewFloatArray, length);
            if !array.is_null() {
                jni!(env, SetFloatArrayRegion, array, 0, length, floats.as_ptr());
            }
            array
        }
    })
}

/// Tells the host the display changed to `width × height` pixels refreshing
/// at `refresh_millihertz`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_display(
    _env: *mut JNIEnv,
    _class: jclass,
    width: jint,
    height: jint,
    refresh_millihertz: jint,
) {
    guarded((), || {
        send(UserInput::Display(display_size(
            count(width),
            count(height),
            count(refresh_millihertz),
        )));
    });
}

/// Tells the host whether the app has focus; losing it releases everything
/// held.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_focus(
    _env: *mut JNIEnv,
    _class: jclass,
    focused: jboolean,
) {
    guarded((), || {
        if flag(focused) {
            send(UserInput::Focus(Focus::Focused));
        } else {
            send(UserInput::Focus(Focus::Unfocused));
            send(UserInput::ReleaseAll);
        }
    });
}

/// Turns the microphone on or off.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_microphone(
    _env: *mut JNIEnv,
    _class: jclass,
    on: jboolean,
) {
    guarded((), || {
        send(UserInput::Microphone(if flag(on) {
            Microphone::On
        } else {
            Microphone::Off
        }));
    });
}

/// Releases everything held, as when the app's controls are hidden.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_releaseAll(
    _env: *mut JNIEnv,
    _class: jclass,
) {
    guarded((), || send(UserInput::ReleaseAll));
}

/// Answers the session's request for a pairing code with `code`, or
/// declines with `null`, which ends the session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_pairingCode(
    env: *mut JNIEnv,
    _class: jclass,
    code: jstring,
) {
    guarded((), || {
        // SAFETY: the VM calls this with its environment and a live
        // reference or null.
        let code = (!code.is_null()).then(|| unsafe { read_string(env, code) });
        if let Some(session) = SESSION
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            session.answer_pairing(code);
        }
    });
}

/// Answers the gateway's request for the user's name and password with
/// `user` and `password`, or declines with a `null` user, which ends the
/// session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_signIn(
    env: *mut JNIEnv,
    _class: jclass,
    user: jstring,
    password: jstring,
) {
    guarded((), || {
        // SAFETY: the VM calls this with its environment and live
        // references, the user's possibly null.
        let answer = (!user.is_null()).then(|| unsafe {
            SignInAnswer::Credentials(Credentials {
                user: read_string(env, user),
                password: read_string(env, password),
            })
        });
        answer_sign_in(answer);
    });
}

/// Answers the gateway's request for a code from the user's authenticator
/// app with `code`, or declines with `null`, which ends the session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_signInCode(
    env: *mut JNIEnv,
    _class: jclass,
    code: jstring,
) {
    guarded((), || {
        // SAFETY: the VM calls this with its environment and a live
        // reference or null.
        let code = (!code.is_null()).then(|| unsafe { read_string(env, code) });
        answer_sign_in(code.map(SignInAnswer::Code));
    });
}

/// Gives the session the user's answer to the gateway's sign-in.
fn answer_sign_in(answer: Option<SignInAnswer>) {
    if let Some(session) = SESSION
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
    {
        session.answer_sign_in(answer);
    }
}

/// Returns the QR code of `text`, as a `totp` event's enrollment URI is
/// shown: its modules on a side, then each module, row by row, 1 if dark
/// and 0 if light; or `null` if it is too long for a QR code.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_qrModules(
    env: *mut JNIEnv,
    _class: jclass,
    text: jstring,
) -> jintArray {
    guarded(ptr::null_mut(), || {
        // SAFETY: the VM calls this with its environment and a live
        // reference.
        let text = unsafe { read_string(env, text) };
        let Some(code) = QrModules::encode(&text) else {
            return ptr::null_mut();
        };
        let size = code.size();
        let ints: Vec<jint> = std::iter::once(jint::try_from(size).unwrap_or(0))
            .chain(
                (0..size * size).map(|index| jint::from(code.is_dark(index % size, index / size))),
            )
            .collect();
        let Ok(length) = jsize::try_from(ints.len()) else {
            return ptr::null_mut();
        };
        // SAFETY: the VM calls this with its environment; the array holds
        // `length` integers.
        unsafe {
            let array = jni!(env, NewIntArray, length);
            if !array.is_null() {
                jni!(env, SetIntArrayRegion, array, 0, length, ints.as_ptr());
            }
            array
        }
    })
}

/// Returns the device's key pin, generating its key the first time, for a
/// host to accept with `qdp serve --client-key`; or `null` if the key cannot
/// be kept.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_clientPin(
    env: *mut JNIEnv,
    _class: jclass,
) -> jstring {
    guarded(ptr::null_mut(), || {
        let pin = files_directory().and_then(|directory| {
            let (identity, _) = TlsIdentity::load_or_generate(&directory, IdentityFiles::CLIENT)
                .map_err(|error| error.to_string())?;
            identity.key_pin().map_err(|error| error.to_string())
        });
        match pin {
            // SAFETY: the VM calls this with its environment.
            Ok(pin) => unsafe { new_string(env, &format_pin(pin)) },
            Err(error) => {
                warn!(%error, "cannot keep the device's key");
                ptr::null_mut()
            }
        }
    })
}

/// Returns whether a QDP host answers at `address`, "host" or "host:port",
/// without connecting to it. Blocks for up to a second and a half, so the
/// app calls it off its main thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_hostReachable(
    env: *mut JNIEnv,
    _class: jclass,
    address: jstring,
) -> jboolean {
    guarded(JNI_FALSE, || {
        // SAFETY: the VM calls this with its environment and a live
        // reference.
        let address = unsafe { read_string(env, address) };
        boolean(host_reachable(&address))
    })
}

/// Returns whether `pin` is a key pin, for the app to check what the user
/// typed.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_isPin(
    env: *mut JNIEnv,
    _class: jclass,
    pin: jstring,
) -> jboolean {
    guarded(JNI_FALSE, || {
        // SAFETY: the VM calls this with its environment and a live
        // reference.
        let pin = unsafe { read_string(env, pin) };
        boolean(parse_pin(&pin).is_ok())
    })
}
