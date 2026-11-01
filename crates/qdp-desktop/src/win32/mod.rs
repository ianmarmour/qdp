//! Windows desktop integration: DXGI Desktop Duplication for the screen,
//! `SendInput`, `InjectTouchInput` and a synthetic pen for input, Direct3D
//! 11 for drawing a client's video, and a keyboard hook for sending
//! Windows' shortcuts to the host.
//!
//! Both are COM or C interfaces, so this module uses `unsafe`: each block
//! states the contract it relies on, from Microsoft's documentation.

#![allow(unsafe_code)]

mod capture;
mod d3d11_video;
mod image;
mod input;
mod keycodes;
mod pen;
mod pen_capture;
mod system_keys;
mod timing;
mod touch;

pub use capture::DesktopDuplication;
pub use d3d11_video::{D3d11Video, Submitter, SyncMode};
pub use input::SendInputInjector;
pub use pen_capture::WindowPen;
pub use system_keys::SystemKeys;
pub use timing::display_timing;
