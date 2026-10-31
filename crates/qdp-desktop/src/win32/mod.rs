//! Windows desktop integration: DXGI Desktop Duplication for the screen, and
//! `SendInput`, `InjectTouchInput` and a synthetic pen for input.
//!
//! Both are COM or C interfaces, so this module uses `unsafe`: each block
//! states the contract it relies on, from Microsoft's documentation.

#![allow(unsafe_code)]

mod capture;
mod image;
mod input;
mod keycodes;
mod pen;
mod pen_capture;
mod timing;
mod touch;

pub use capture::DesktopDuplication;
pub use input::SendInputInjector;
pub use pen_capture::WindowPen;
pub use timing::display_timing;
