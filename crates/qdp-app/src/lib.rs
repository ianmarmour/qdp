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

pub mod events;
pub mod gamepad;
pub mod session;
pub mod settings;
pub mod sink;
