//! The functions the app's `io.github.ianmarmour.qdp.Native` class calls.
//!
//! The app runs one session at a time: `connect` starts it in the app's
//! video surface, the input functions send the user's input to it, a thread
//! of the app's waits for its events with `nextEvent`, and `disconnect` ends
//! it before the surface goes. Input sent while no session runs is dropped.
//! JNI is a C interface, so this module uses `unsafe`.

#![allow(unsafe_code)]

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::{Mutex, Once, PoisonError};
use std::time::Duration;

use jni_sys::{
    JNI_FALSE, JNI_OK, JNI_TRUE, JNIEnv, JavaVM, jboolean, jclass, jfloat, jint, jintArray,
    jobject, jsize, jstring,
};
use ndk::native_window::NativeWindow;
use qdp::message::DeviceFeatures;
use qdp_codec::{CodecPreference, set_video_surface};
use qdp_runtime::{Focus, Microphone, PointerButton, TouchPhase, UserInput};
use tracing::{info, warn};

use crate::gamepad::GamepadState;
use crate::keymap::{HidKey, hid_key};
use crate::logcat;
use crate::session::{Config, Session};
use crate::settings::{display_size, parse_pin};

/// The `decoder` value of `connect` asking for MediaCodec, failing without
/// it; any other value but [`DECODER_SOFTWARE`] decodes in hardware when the
/// device can, and in software otherwise.
const DECODER_HARDWARE: jint = 1;

/// The `decoder` value of `connect` asking for the software decoder.
const DECODER_SOFTWARE: jint = 2;

/// The session, while one runs.
static SESSION: Mutex<Option<Session>> = Mutex::new(None);

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

/// Sets up logging, and gives the audio library the app's context.
#[unsafe(no_mangle)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_init(
    env: *mut JNIEnv,
    _class: jclass,
    context: jobject,
) {
    guarded((), || {
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

/// Starts a session with the host at `host`, whose key pin is `pin`, with
/// `token`, showing the video in `surface` on a display of `width × height`
/// pixels refreshing at `refresh_millihertz`. Returns `null` once it has
/// started, or why it could not.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_io_github_ianmarmour_qdp_Native_connect(
    env: *mut JNIEnv,
    _class: jclass,
    surface: jobject,
    host: jstring,
    pin: jstring,
    token: jstring,
    width: jint,
    height: jint,
    refresh_millihertz: jint,
    decoder: jint,
    microphone: jboolean,
) -> jstring {
    guarded(ptr::null_mut(), || {
        // SAFETY: the VM calls this with its environment and live
        // references.
        let (host, pin, token, window) = unsafe {
            (
                read_string(env, host),
                read_string(env, pin),
                read_string(env, token),
                NativeWindow::from_surface(env, surface),
            )
        };
        let config = parse_pin(&pin).map(|pin| Config {
            host,
            pin,
            token,
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
        });
        let started = config.and_then(|config| {
            let window = window.ok_or("the surface has no window")?;
            stop_session();
            set_video_surface(Some(window));
            let session = Session::start(config)
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
/// `timeout_millis` for one, or `null` if none came or no session runs.
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
        match events.and_then(|events| events.next(timeout)) {
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
        let ints = cursor.to_ints();
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
        let phase = match phase {
            0 => TouchPhase::Down,
            1 => TouchPhase::Move,
            2 => TouchPhase::Up,
            3 => TouchPhase::Cancel,
            _ => return,
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
