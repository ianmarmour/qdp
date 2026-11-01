//! The Windows app's core of Quick Desktop Protocol (QDP): the app calls it
//! from C# through the C interface in `include/qdp_windows.h`, which
//! `qdp_windows.dll` exports.
//!
//! A session is [`qdp_app`]'s, as the Android and Apple apps' are, shown in a
//! stream window of the core's own rather than in the app's: a top-level
//! window drawn with a flip-model swap chain, which the system can hand to
//! the display without composing it, the least delay a window can have.
//! Its video is decoded by D3D11 video, or by rav1d where the GPU has no AV1
//! decoder, and drawn by a render thread of its own: pictures go from the
//! session's thread straight to the render thread, never through the
//! window's. The window reads the user's keyboard, mouse, touch, pen and
//! gamepads as the desktop client's does, and closes when the session ends.
//!
//! The C functions exist only on Windows; [`events`], the queue of what
//! the app is told, is tested with the workspace.

pub mod events;

#[cfg(windows)]
mod badge;
#[cfg(windows)]
mod ffi;
#[cfg(windows)]
mod stage;
#[cfg(windows)]
mod window;
