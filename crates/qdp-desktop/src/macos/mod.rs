//! macOS desktop integration: ScreenCaptureKit for the screen, AppKit and
//! Quartz for the pointer, and Quartz events for input.
//!
//! Both are Objective-C or C interfaces, so this module uses `unsafe`: each
//! block states the contract it relies on, from Apple's documentation.

#![allow(unsafe_code)]

mod capture;
mod cursor;
mod input;
mod keycodes;

pub use capture::MacScreenCapture;
pub use input::QuartzInjector;
