//! Windows desktop integration: DXGI Desktop Duplication for the screen, and
//! `SendInput` for input.
//!
//! Both are COM or C interfaces, so this module uses `unsafe`: each block
//! states the contract it relies on, from Microsoft's documentation.

#![allow(unsafe_code)]

mod capture;
mod image;
mod input;
mod keycodes;

pub use capture::DesktopDuplication;
pub use input::SendInputInjector;
