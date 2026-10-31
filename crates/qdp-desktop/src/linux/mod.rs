//! Linux desktop integration.
//!
//! uinput is a kernel interface reached through `ioctl`, so this module uses
//! `unsafe`: each block states the contract it relies on.

#![allow(unsafe_code)]

mod keycodes;
mod uinput;

pub use uinput::UinputInjector;
