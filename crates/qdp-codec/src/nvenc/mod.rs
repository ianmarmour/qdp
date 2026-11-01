//! Hardware AV1 encoding with NVENC, on NVIDIA GPUs that encode AV1 (GeForce
//! RTX 40 and later, and their professional counterparts), through the
//! driver's `nvEncodeAPI64.dll`.
//!
//! NVENC is a C interface, so this module uses `unsafe`: each block states
//! the contract it relies on, from NVIDIA's documentation.

#![cfg_attr(windows, allow(unsafe_code))]

#[cfg(windows)]
mod encoder;
// The structures' layout tests run everywhere; the encoder is Windows'.
pub(crate) mod ffi;

#[cfg(windows)]
pub(crate) use encoder::NvencEncoder;
