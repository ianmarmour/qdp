//! Linux desktop integration.
//!
//! - Input: uinput, a kernel interface reached through `ioctl`, so this module
//!   uses `unsafe`: each block states the contract it relies on.
//! - Screen capture: the ScreenCast portal and PipeWire.
//! - Audio: PipeWire.

#![allow(unsafe_code)]

#[cfg(feature = "audio")]
mod audio;
#[cfg(feature = "screen-capture")]
mod desktop_layout;
mod gamepad;
mod hidraw;
mod keycodes;
mod pen;
#[cfg(feature = "screen-capture")]
mod screencast;
mod seat;
mod touchpad;
mod touchpad_capture;
mod touchscreen;
mod uhid;
mod uinput;

#[cfg(feature = "audio")]
pub use audio::{PipeWireInput, PipeWireOutput};
#[cfg(feature = "screen-capture")]
pub use screencast::ScreenCapture;
pub use uinput::UinputInjector;

pub(crate) use hidraw::start as start_passthrough;
pub(crate) use seat::devices_confined;
pub(crate) use touchpad_capture::start as start_touchpad;
