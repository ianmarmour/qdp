//! Linux desktop integration.
//!
//! - Input: uinput, a kernel interface reached through `ioctl`, so this module
//!   uses `unsafe`: each block states the contract it relies on.
//! - Screen capture: the ScreenCast portal and PipeWire, or, for a host
//!   running as root, the console's framebuffers through DRM, an `ioctl`
//!   interface too. A GNOME desktop can also be shared, input included,
//!   through Mutter's own remote desktop interface, which asks nobody.
//! - Audio: PipeWire.

#![allow(unsafe_code)]

#[cfg(feature = "audio")]
mod audio;
#[cfg(feature = "discovery")]
pub(crate) mod avahi;
#[cfg(feature = "discovery")]
pub(crate) mod avahi_browse;
#[cfg(feature = "screen-capture")]
mod desktop_layout;
#[cfg(feature = "screen-capture")]
mod dma_buf_map;
mod gamepad;
mod hidraw;
#[cfg(feature = "screen-capture")]
mod kde_displays;
#[cfg(feature = "screen-capture")]
mod kde_protocols;
mod keycodes;
#[cfg(feature = "screen-capture")]
mod kms;
#[cfg(feature = "screen-capture")]
mod monitor_matching;
#[cfg(feature = "screen-capture")]
mod mutter;
#[cfg(feature = "notifications")]
pub(crate) mod notifications;
mod pen;
#[cfg(feature = "screen-capture")]
mod screencast;
mod seat;
#[cfg(feature = "notifications")]
mod session_helper;
mod touchpad;
mod touchpad_capture;
mod touchscreen;
mod uhid;
mod uinput;
#[cfg(feature = "screen-capture")]
mod wlr_displays;

#[cfg(feature = "audio")]
pub use audio::{PipeWireInput, PipeWireOutput};
#[cfg(feature = "screen-capture")]
pub use kms::ConsoleCapture;
#[cfg(feature = "screen-capture")]
pub use mutter::{open_gnome, probe_injector as gnome_probe_injector};
#[cfg(feature = "screen-capture")]
pub use screencast::ScreenCapture;
pub use uinput::UinputInjector;

pub(crate) use hidraw::start as start_passthrough;
#[cfg(feature = "screen-capture")]
pub(crate) use monitor_matching::restore_saved as restore_monitor_settings;
pub(crate) use seat::devices_confined;
pub use seat::{SeatSession, seat_session};
#[cfg(feature = "notifications")]
pub(crate) use session_helper::{HelperClipboard, HelperNotification, console_helper};
#[cfg(feature = "notifications")]
pub use session_helper::{follow_console_session, run_session_helper};
pub(crate) use touchpad_capture::start as start_touchpad;
