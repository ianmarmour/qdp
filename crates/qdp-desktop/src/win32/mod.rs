//! Windows desktop integration: DXGI Desktop Duplication for the screen,
//! `SendInput`, `InjectTouchInput` and a synthetic pen for input, Direct3D
//! 11 for drawing a client's video, a keyboard hook for sending Windows'
//! shortcuts to the host, and notices and privacy on the host's screen.
//!
//! Both are COM or C interfaces, so this module uses `unsafe`: each block
//! states the contract it relies on, from Microsoft's documentation.

#![allow(unsafe_code)]

mod capture;
mod d3d11_video;
mod image;
mod input;
mod input_desktop;
mod keycodes;
#[cfg(feature = "notifications")]
mod notice;
mod overlay;
mod pen;
mod pen_capture;
mod privacy;
mod system_keys;
mod timing;
mod touch;

pub use capture::DesktopDuplication;
pub use d3d11_video::{D3d11Video, Submitter, SyncMode};
pub use input::SendInputInjector;
#[cfg(feature = "notifications")]
pub(crate) use notice::Notice;
pub use pen_capture::WindowPen;
pub(crate) use privacy::Privacy;
pub use system_keys::SystemKeys;
pub use timing::display_timing;
