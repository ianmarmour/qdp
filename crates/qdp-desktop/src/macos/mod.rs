//! macOS desktop integration: ScreenCaptureKit for the screen, AppKit and
//! Quartz for the pointer, and Quartz events for input.
//!
//! Both are Objective-C or C interfaces, so this module uses `unsafe`: each
//! block states the contract it relies on, from Apple's documentation.

#![allow(unsafe_code)]

#[cfg(feature = "audio")]
mod audio;
mod capture;
mod cursor;
mod input;
mod keycodes;
mod metal_video;
mod pen;

#[cfg(feature = "audio")]
pub use audio::MacDesktopAudio;
pub use capture::MacScreenCapture;
pub use metal_video::{Drawing, MetalVideo, Overlay, PixelRect};

/// Returns the desktop's sound, captured with ScreenCaptureKit.
#[cfg(feature = "audio")]
pub(crate) fn desktop_audio() -> Option<Box<dyn qdp_runtime::AudioInput>> {
    Some(Box::new(MacDesktopAudio))
}
pub use input::QuartzInjector;
