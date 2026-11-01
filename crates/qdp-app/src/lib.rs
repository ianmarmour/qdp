//! The core QDP's app clients share, the Android and Apple apps, which each
//! call it from their own code: Kotlin through JNI, Swift through C.
//!
//! - [`session`] runs a session with a host on a thread of its own,
//!   reconnecting as the desktop client does and pairing with the code the
//!   host shows (Section 4.4.9), and passes the user's input to it.
//! - [`events`] holds what the app is told, as lines of text.
//! - [`sink`] passes the session's video to the platform's [`sink::Presenter`]
//!   and its cursor, pointer mode and statistics to the app.
//! - [`settings`] reads what the app connects with.
//! - [`gamepad`] converts a gamepad's state to QDP's report.
//! - [`cursor`] scales the host's cursor images to the picture, for the
//!   clients that draw them, the desktop client among them.
//! - `keymap`, with the `winit` feature, reports the keys of winit's windows
//!   as HID usages, for the desktop clients.
//! - [`placement`] fits the picture in the app's view, keeping its shape.

pub mod cursor;
pub mod events;
pub mod gamepad;
#[cfg(feature = "winit")]
pub mod keymap;
pub mod placement;
pub mod session;
pub mod settings;
pub mod sink;
