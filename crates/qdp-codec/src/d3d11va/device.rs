//! The Direct3D 11 device decoders and windows share, so that a window draws
//! a picture where its decoder left it, without a copy.
//!
//! The device is made on the GPU that decodes AV1: the default adapter if it
//! does, as it drives the main display, and otherwise the first other GPU
//! that does, as on laptops whose integrated GPU predates AV1 but whose
//! discrete one decodes it. A window on a display the other GPU drives
//! still shows its pictures, which Windows copies across.

use std::sync::{Mutex, PoisonError};

use tracing::info;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL,
    D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0, D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device,
    ID3D11DeviceContext, ID3D11Multithread, ID3D11VideoDevice,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, IDXGIAdapter, IDXGIFactory1,
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
/// a window must draw their pictures with: made on the GPU that decodes
/// AV1 the first time, and again if that GPU was reset or removed since.
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

/// Returns whether `device`'s GPU decodes 8-bit AV1 into NV12 textures.
pub(super) fn decodes_av1(device: &ID3D11Device) -> bool {
    let Ok(video_device) = device.cast::<ID3D11VideoDevice>() else {
        return false;
    };
    // SAFETY: the device is valid; the calls have no other contract.
    let profiles = unsafe { video_device.GetVideoDecoderProfileCount() };
    let has_av1 = (0..profiles).any(|index| {
        // SAFETY: the index is below the count.
        unsafe { video_device.GetVideoDecoderProfile(index) }
            .is_ok_and(|profile| profile == D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0)
    });
    // SAFETY: the profile is a valid GUID.
    let nv12 = unsafe {
        video_device
            .CheckVideoDecoderFormat(&D3D11_DECODER_PROFILE_AV1_VLD_PROFILE0, DXGI_FORMAT_NV12)
    }
    .is_ok_and(|supported| supported.as_bool());
    has_av1 && nv12
}

/// Makes the device on the default adapter if it decodes AV1, else on the
/// first other hardware adapter that does, else on the default adapter.
fn create() -> Result<ID3D11Device, CodecError> {
    let default = create_on(None)?;
    if decodes_av1(&default) {
        return Ok(default);
    }
    // The default adapter is the first.
    let decoding = hardware_adapters()
        .into_iter()
        .filter(|adapter| adapter.index != 0)
        .filter_map(|adapter| Some((create_on(Some(&adapter.adapter)).ok()?, adapter.name)))
        .find(|(device, _)| decodes_av1(device));
    match decoding {
        Some((device, name)) => {
            info!(gpu = name, "decoding on the GPU that decodes AV1");
            Ok(device)
        }
        None => Ok(default),
    }
}

/// Makes a device on the first hardware adapter of the PCI vendor `vendor`,
/// as an encoder of that vendor's needs, returning it and the adapter's
/// name.
pub(crate) fn create_for_vendor(vendor: u32) -> Result<(ID3D11Device, String), CodecError> {
    let adapter = hardware_adapters()
        .into_iter()
        .find(|adapter| adapter.vendor == vendor)
        .ok_or_else(|| {
            CodecError::HardwareUnavailable(format!("no GPU of PCI vendor {vendor:#06x}"))
        })?;
    Ok((create_on(Some(&adapter.adapter))?, adapter.name))
}

/// A hardware adapter.
struct HardwareAdapter {
    /// Its index, the default adapter's 0.
    index: u32,
    /// The adapter.
    adapter: IDXGIAdapter,
    /// Its name.
    name: String,
    /// Its PCI vendor.
    vendor: u32,
}

/// Returns the hardware adapters, the default first.
fn hardware_adapters() -> Vec<HardwareAdapter> {
    // SAFETY: the call has no contract.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };
    let mut adapters = Vec::new();
    for index in 0.. {
        // SAFETY: indices from 0 up to the first that fails name adapters.
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(index) }) else {
            break;
        };
        // SAFETY: the adapter is valid.
        let Ok(description) = (unsafe { adapter.GetDesc1() }) else {
            continue;
        };
        if description.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let length = description
            .Description
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(description.Description.len());
        if let Ok(adapter) = adapter.cast::<IDXGIAdapter>() {
            adapters.push(HardwareAdapter {
                index,
                adapter,
                name: String::from_utf16_lossy(&description.Description[..length]),
                vendor: description.VendorId,
            });
        }
    }
    adapters
}

/// Makes a device that decodes video, with a thread safe immediate context,
/// on `adapter`, or on the default one.
fn create_on(adapter: Option<&IDXGIAdapter>) -> Result<ID3D11Device, CodecError> {
    let unavailable = |what: &str, error: windows::core::Error| {
        CodecError::HardwareUnavailable(format!("{what}: {error}"))
    };
    // A device on a given adapter names no driver type; the default one is
    // the hardware's.
    let driver: D3D_DRIVER_TYPE = if adapter.is_some() {
        D3D_DRIVER_TYPE_UNKNOWN
    } else {
        D3D_DRIVER_TYPE_HARDWARE
    };
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    // SAFETY: the outputs are valid to write and written before the call
    // returns; the adapter, if any, is valid.
    unsafe {
        D3D11CreateDevice(
            adapter,
            driver,
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
