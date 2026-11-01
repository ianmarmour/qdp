//! Hardware AV1 decoding with D3D11 video (DXVA) on Windows.
//!
//! DXVA decoders are given each frame's header as fields rather than reading
//! the bitstream, so the frames are parsed here first, and each is decoded
//! into a slice of an NV12 texture array on the device windows draw with,
//! which a window then samples without a copy.
//!
//! Direct3D is a COM interface, so the Windows modules use `unsafe`: each
//! block states the contract it relies on, from Microsoft's documentation.

#![cfg_attr(windows, allow(unsafe_code))]

#[cfg(windows)]
mod decoder;
#[cfg(windows)]
mod device;
pub(crate) mod dxva;
#[cfg(windows)]
mod picture;

#[cfg(windows)]
pub use decoder::D3d11Decoder;
#[cfg(windows)]
pub use device::video_device;
#[cfg(windows)]
pub use picture::D3d11Picture;
