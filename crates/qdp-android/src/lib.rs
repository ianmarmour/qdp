//! The Android client of Quick Desktop Protocol (QDP): the Rust core of the
//! app, which its Kotlin code calls through JNI.
//!
//! The app shows a session in a `SurfaceView` and hands its surface to the
//! core, which runs the session on a thread of its own. Video is decoded by
//! MediaCodec straight into the surface, or, on a device without an AV1
//! decoder, by rav1d and drawn there on the processor. Audio plays and
//! captures through AAudio. The app passes on the user's keys, pointer,
//! touches and gamepads, and reads the session's events: its progress, the
//! video's size, the host's cursor and pointer mode, and gamepad rumble.
//!
//! The JNI functions, logging and drawing exist only on Android; the rest
//! builds everywhere, so that it is tested with the workspace.

pub mod cursor;
pub mod events;
pub mod gamepad;
pub mod keymap;
pub mod session;
pub mod settings;
pub mod sink;

#[cfg(target_os = "android")]
mod jni;
#[cfg(target_os = "android")]
mod logcat;
mod pixels;
#[cfg(target_os = "android")]
mod software;
