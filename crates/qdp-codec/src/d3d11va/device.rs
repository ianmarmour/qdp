//! The Direct3D 11 device decoders and windows share, so that a window draws
//! a picture where its decoder left it, without a copy.

use std::sync::{Mutex, PoisonError};

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Multithread,
};
use windows::core::Interface;

use crate::CodecError;

/// The feature levels asked for, best first: video decoding and the flip
/// model need 11.
const FEATURE_LEVELS: [D3D_FEATURE_LEVEL; 2] = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];

/// The shared device, once made.
struct SharedDevice(ID3D11Device);

// SAFETY: Direct3D 11 devices are free threaded, and the device's immediate
// context, which is not, is made thread safe with ID3D11Multithread when the
// device is created.
unsafe impl Send for SharedDevice {}

/// The device decoders and windows share.
static SHARED: Mutex<Option<SharedDevice>> = Mutex::new(None);

/// Returns the Direct3D 11 device that hardware decoders decode with, which
/// a window must draw their pictures with: made on the default adapter the
/// first time, and again if the adapter was reset or removed since.
///
/// Its immediate context may be used from any thread: Direct3D serializes
/// the calls.
pub fn video_device() -> Result<ID3D11Device, CodecError> {
    let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(SharedDevice(device)) = shared.as_ref() {
        // SAFETY: the device is valid; the call has no other contract.
        if unsafe { device.GetDeviceRemovedReason() }.is_ok() {
            return Ok(device.clone());
        }
    }
    let device = create()?;
    *shared = Some(SharedDevice(device.clone()));
    Ok(device)
}

/// Makes a device on the default adapter that decodes video, with a thread
/// safe immediate context.
fn create() -> Result<ID3D11Device, CodecError> {
    let unavailable = |what: &str, error: windows::core::Error| {
        CodecError::HardwareUnavailable(format!("{what}: {error}"))
    };
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    // SAFETY: the outputs are valid to write and written before the call
    // returns; no adapter means the default one.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&FEATURE_LEVELS),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|error| unavailable("no Direct3D 11 device decodes video", error))?;
    let device = device.ok_or(CodecError::HardwareUnavailable(
        "Direct3D returned no device".to_owned(),
    ))?;
    let multithread: ID3D11Multithread = context
        .ok_or(CodecError::HardwareUnavailable(
            "Direct3D returned no device context".to_owned(),
        ))?
        .cast()
        .map_err(|error| unavailable("the device context cannot be shared", error))?;
    // SAFETY: the interface is valid; the call has no other contract. It
    // returns whether the context was protected before, which it was not.
    let _was_protected = unsafe { multithread.SetMultithreadProtected(true) };
    Ok(device)
}
