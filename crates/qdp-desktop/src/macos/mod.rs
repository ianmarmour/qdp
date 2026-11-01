//! macOS desktop integration: ScreenCaptureKit for the screen, AppKit and
//! Quartz for the pointer, Quartz events for input, and Notification Center
//! for notifications.
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
#[cfg(feature = "notifications")]
pub(crate) mod notifications;
mod pen;

#[cfg(feature = "audio")]
pub use audio::MacDesktopAudio;
pub use capture::MacScreenCapture;

/// Returns the USB HID Keyboard/Keypad page usage of a macOS virtual key
/// code, as `NSEvent.keyCode` gives it, if it is one of a key QDP sends.
pub fn mac_key_usage(virtual_key: u16) -> Option<u8> {
    keycodes::usage(virtual_key)
}

/// Returns the desktop's sound, captured with ScreenCaptureKit.
#[cfg(feature = "audio")]
pub(crate) fn desktop_audio() -> Option<Box<dyn qdp_runtime::AudioInput>> {
    Some(Box::new(MacDesktopAudio))
}
pub use input::QuartzInjector;
