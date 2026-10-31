//! Linux desktop integration.
//!
//! - Input: uinput, a kernel interface reached through `ioctl`, so this module
//!   uses `unsafe`: each block states the contract it relies on.
//! - Screen capture: the ScreenCast portal and PipeWire.

#![allow(unsafe_code)]

mod keycodes;
#[cfg(feature = "screen-capture")]
mod screencast;
mod uinput;

#[cfg(feature = "screen-capture")]
pub use screencast::ScreenCapture;
pub use uinput::UinputInjector;
