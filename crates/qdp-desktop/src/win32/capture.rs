//! Screen capture on Windows with DXGI Desktop Duplication, which copies each
//! new desktop image on the graphics card and reports the pointer apart from
//! it, for the client to draw.
//!
//! Desktop Duplication stops while the secure desktop (the sign-in screen,
//! Ctrl+Alt+Delete and elevation prompts) is shown; the capture keeps
//! delivering the last image until it can duplicate the desktop again.

use std::slice;
use std::sync::Arc;
use std::time::{Duration, Instant};

use qdp_codec::{Rgb32Order, Yuv420Frame};
use qdp_runtime::{CursorShape, CursorState, FrameSource, PointerPosition, RuntimeError};
use tracing::{info, warn};
use windows::Win32::Foundation::{E_ACCESSDENIED, HMODULE};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_MODE_ROTATION, DXGI_MODE_ROTATION_ROTATE90,
    DXGI_MODE_ROTATION_ROTATE180, DXGI_MODE_ROTATION_ROTATE270, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_CURRENTLY_AVAILABLE,
    DXGI_ERROR_NOT_FOUND, DXGI_ERROR_SESSION_DISCONNECTED, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO, IDXGIAdapter1, IDXGIFactory1,
    IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
};
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::core::{HRESULT, Interface};

use super::image::{Bgra, PointerShape, Rotation, ShapeKind};

/// How long to wait for the first desktop image.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);

/// How long each wait for a new desktop image lasts while opening, in
/// milliseconds.
const OPEN_POLL_MILLISECONDS: u32 = 100;

/// Errors after which duplication can start again once the desktop allows
/// it: the secure desktop, another session taking the console, or too many
/// duplications.
const TRANSIENT_ERRORS: [HRESULT; 3] = [
    E_ACCESSDENIED,
    DXGI_ERROR_SESSION_DISCONNECTED,
    DXGI_ERROR_NOT_CURRENTLY_AVAILABLE,
];

/// Returns the rotation of a `DXGI_MODE_ROTATION`.
fn rotation(value: DXGI_MODE_ROTATION) -> Rotation {
    match value {
        DXGI_MODE_ROTATION_ROTATE90 => Rotation::Clockwise90,
        DXGI_MODE_ROTATION_ROTATE180 => Rotation::Clockwise180,
        DXGI_MODE_ROTATION_ROTATE270 => Rotation::Clockwise270,
        _ => Rotation::Identity,
    }
}

/// The pointer as Desktop Duplication last reported it.
#[derive(Debug, Default)]
struct Pointer {
    /// Whether the pointer is on the captured monitor and shown.
    visible: bool,
    /// The top-left corner of its image, in desktop pixels.
    position: (i32, i32),
    /// Its image as RGBA, once reported.
    shape: Option<Arc<CursorShape>>,
}

impl Pointer {
    /// Returns the pointer as the client draws it: its hotspot's position
    /// and its image, while it is shown.
    fn state(&self) -> CursorState {
        let shape = self.shape.clone().filter(|_| self.visible);
        let position = shape.as_ref().map(|shape| {
            let offset = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
            (
                self.position.0.saturating_add(offset(shape.hotspot.0)),
                self.position.1.saturating_add(offset(shape.hotspot.1)),
            )
        });
        CursorState {
            position: position.map_or(PointerPosition::Hidden, |(x, y)| PointerPosition::At(x, y)),
            shape,
        }
    }
}

/// Returns the adapter and output of the primary monitor: the output whose
/// desktop area starts at the origin.
fn primary_output(factory: &IDXGIFactory1) -> Result<(IDXGIAdapter1, IDXGIOutput1), String> {
    for adapter_index in 0.. {
        // SAFETY: enumeration takes any index, ending with NOT_FOUND.
        let adapter = match unsafe { factory.EnumAdapters1(adapter_index) } {
            Ok(adapter) => adapter,
            Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
            Err(error) => return Err(format!("cannot list the graphics adapters: {error}")),
        };
        for output_index in 0.. {
            // SAFETY: as above.
            let output = match unsafe { adapter.EnumOutputs(output_index) } {
                Ok(output) => output,
                Err(_) => break,
            };
            // SAFETY: the output is valid.
            let Ok(description) = (unsafe { output.GetDesc() }) else {
                continue;
            };
            let area = description.DesktopCoordinates;
            if description.AttachedToDesktop.as_bool() && area.left == 0 && area.top == 0 {
                let output = output
                    .cast::<IDXGIOutput1>()
                    .map_err(|_| "Desktop Duplication needs Windows 8 or later".to_owned())?;
                return Ok((adapter, output));
            }
        }
    }
    Err("no monitor is attached to the desktop".to_owned())
}

/// Captures the primary monitor.
pub struct DesktopDuplication {
    /// The Direct3D device on the monitor's adapter.
    device: ID3D11Device,
    /// The device's immediate context, which copies images.
    context: ID3D11DeviceContext,
    /// The monitor.
    output: IDXGIOutput1,
    /// The duplication, while the desktop allows one.
    duplication: Option<IDXGIOutputDuplication>,
    /// The CPU-readable copy of the newest desktop image, made on first use.
    staging: Option<ID3D11Texture2D>,
    /// How the monitor is rotated.
    rotation: Rotation,
    /// The upright desktop's size in pixels, from the first image.
    size: (usize, usize),
    /// The newest desktop image, upright.
    desktop: Bgra,
    /// The pointer, which the client draws (Section 9.1).
    pointer: Pointer,
    /// The newest frame, or `None` if the desktop changed since.
    frame: Option<Yuv420Frame>,
}

impl std::fmt::Debug for DesktopDuplication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DesktopDuplication")
            .field("size", &self.size)
            .field("rotation", &self.rotation)
            .finish_non_exhaustive()
    }
}

impl DesktopDuplication {
    /// Starts capturing the primary monitor at its full pixel resolution.
    /// Blocks until the first image arrives.
    pub fn open() -> Result<Self, String> {
        // Windows refuses to duplicate scaled monitors for processes that do
        // not handle scaling themselves, since their coordinates are scaled.
        // Setting it fails harmlessly if the process already chose.
        // SAFETY: the call takes a predefined context.
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        // SAFETY: creating a factory has no preconditions.
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
            .map_err(|error| format!("cannot start DXGI: {error}"))?;
        let (adapter, output) = primary_output(&factory)?;
        let mut device = None;
        let mut context = None;
        // SAFETY: the adapter is valid, and the outputs are written before
        // the call returns.
        unsafe {
            D3D11CreateDevice(
                &adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        }
        .map_err(|error| format!("cannot open the graphics adapter: {error}"))?;
        let (Some(device), Some(context)) = (device, context) else {
            return Err("the graphics adapter returned no device".to_owned());
        };
        let duplication = duplicate(&output, &device)?;
        // SAFETY: the duplication is valid.
        let rotation = rotation(unsafe { duplication.GetDesc() }.Rotation);
        let mut capture = Self {
            device,
            context,
            output,
            duplication: Some(duplication),
            staging: None,
            rotation,
            size: (0, 0),
            desktop: Bgra::default(),
            pointer: Pointer::default(),
            frame: None,
        };
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
        while capture.desktop.data.is_empty() {
            if Instant::now() > deadline {
                return Err("no image arrived from the screen".to_owned());
            }
            capture.acquire(OPEN_POLL_MILLISECONDS)?;
        }
        let (width, height) = (capture.desktop.width, capture.desktop.height);
        capture.size = (width, height);
        info!(width, height, ?rotation, "capturing the screen");
        Ok(capture)
    }

    /// Takes the newest desktop image and pointer, waiting up to `timeout`
    /// milliseconds for a change. Starts duplicating again if the desktop
    /// stopped it.
    fn acquire(&mut self, timeout: u32) -> Result<(), String> {
        let duplication = match &self.duplication {
            Some(duplication) => duplication.clone(),
            None => match self.reconnect()? {
                Some(duplication) => duplication,
                None => return Ok(()),
            },
        };
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        // SAFETY: both outputs are written before the call returns.
        match unsafe { duplication.AcquireNextFrame(timeout, &mut info, &mut resource) } {
            Ok(()) => {}
            Err(error) if error.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(()),
            Err(error) if error.code() == DXGI_ERROR_ACCESS_LOST => {
                info!("the desktop changed; duplicating it again");
                self.duplication = None;
                return Ok(());
            }
            Err(error) => return Err(format!("cannot capture the screen: {error}")),
        }
        let taken = self.take(&duplication, &info, resource);
        // SAFETY: a frame was acquired above.
        let released = unsafe { duplication.ReleaseFrame() };
        taken?;
        released.map_err(|error| format!("cannot capture the screen: {error}"))
    }

    /// Starts duplicating the desktop again. Returns `None` while the
    /// desktop does not allow it.
    fn reconnect(&mut self) -> Result<Option<IDXGIOutputDuplication>, String> {
        // SAFETY: the output and device are valid.
        let duplication = match unsafe { self.output.DuplicateOutput(&self.device) } {
            Ok(duplication) => duplication,
            Err(error) if TRANSIENT_ERRORS.contains(&error.code()) => return Ok(None),
            Err(error) => return Err(format!("cannot capture the screen: {error}")),
        };
        // SAFETY: the duplication is valid.
        self.rotation = rotation(unsafe { duplication.GetDesc() }.Rotation);
        self.duplication = Some(duplication.clone());
        // The new duplication's first frame holds the whole desktop, perhaps
        // at a new size: forgetting the old image makes sure it is taken.
        // Until it arrives, the last frame is sent again.
        self.desktop.data.clear();
        Ok(Some(duplication))
    }

    /// Keeps what an acquired frame reports: a new desktop image, pointer
    /// position or pointer shape.
    fn take(
        &mut self,
        duplication: &IDXGIOutputDuplication,
        info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: Option<IDXGIResource>,
    ) -> Result<(), String> {
        // The first frame of a duplication holds the whole desktop even when
        // nothing was presented since it started.
        if info.LastPresentTime != 0 || self.desktop.data.is_empty() {
            let texture = resource
                .ok_or("the screen delivered no image")?
                .cast::<ID3D11Texture2D>()
                .map_err(|error| format!("the screen image is not a texture: {error}"))?;
            self.copy_desktop(&texture)?;
            self.frame = None;
        }
        if info.LastMouseUpdateTime != 0 {
            let position = info.PointerPosition;
            self.pointer.visible = position.Visible.as_bool();
            self.pointer.position = (position.Position.x, position.Position.y);
        }
        if info.PointerShapeBufferSize > 0 {
            let mut buffer = vec![0_u8; info.PointerShapeBufferSize as usize];
            let mut required = 0;
            let mut shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
            // SAFETY: the buffer holds the size given, and the outputs are
            // written before the call returns.
            unsafe {
                duplication.GetFramePointerShape(
                    info.PointerShapeBufferSize,
                    buffer.as_mut_ptr().cast(),
                    &mut required,
                    &mut shape,
                )
            }
            .map_err(|error| format!("cannot read the pointer's image: {error}"))?;
            let hotspot = (shape.HotSpot.x, shape.HotSpot.y);
            self.pointer.shape = ShapeKind::from_dxgi(shape.Type)
                .and_then(|kind| {
                    PointerShape::new(
                        kind,
                        (shape.Width as usize, shape.Height as usize),
                        shape.Pitch as usize,
                        buffer,
                    )
                })
                .map(|pointer| Arc::new(cursor_shape(&pointer, hotspot)));
            if self.pointer.shape.is_none() {
                warn!(kind = shape.Type, "unreadable pointer image skipped");
            }
        }
        Ok(())
    }

    /// Copies a desktop image from the graphics card.
    fn copy_desktop(&mut self, texture: &ID3D11Texture2D) -> Result<(), String> {
        let mut description = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the description is written before the call returns.
        unsafe { texture.GetDesc(&mut description) };
        if description.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(format!(
                "the screen image has an unexpected format ({})",
                description.Format.0
            ));
        }
        let staging = match &self.staging {
            Some(staging) => staging.clone(),
            None => {
                let staging = self.create_staging(description)?;
                self.staging = Some(staging.clone());
                staging
            }
        };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: both textures are valid and the same size and format; the
        // staging texture is CPU-readable and unmapped below.
        unsafe {
            self.context.CopyResource(&staging, texture);
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
                .map_err(|error| format!("cannot read the screen image: {error}"))?;
        }
        let native = (description.Width as usize, description.Height as usize);
        let stride = mapped.RowPitch as usize;
        // SAFETY: the mapped texture holds `Height` rows of `RowPitch` bytes
        // until it is unmapped.
        let data = unsafe { slice::from_raw_parts(mapped.pData.cast::<u8>(), stride * native.1) };
        let copied = self
            .desktop
            .copy_upright(data, native, stride, self.rotation);
        // SAFETY: the texture was mapped above.
        unsafe { self.context.Unmap(&staging, 0) };
        copied.ok_or_else(|| "the screen image is too small".to_owned())
    }

    /// Creates a CPU-readable texture like `description`.
    fn create_staging(&self, description: D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D, String> {
        let description = D3D11_TEXTURE2D_DESC {
            MipLevels: 1,
            ArraySize: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: 0,
            CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
            MiscFlags: 0,
            ..description
        };
        let mut staging = None;
        // SAFETY: the description is valid, and the texture is written
        // before the call returns.
        unsafe {
            self.device
                .CreateTexture2D(&description, None, Some(&mut staging))
        }
        .map_err(|error| format!("cannot create a texture: {error}"))?;
        staging.ok_or_else(|| "the graphics adapter returned no texture".to_owned())
    }
}

/// Returns a pointer image as the client draws it, with its hotspot.
fn cursor_shape(pointer: &PointerShape, (hotspot_x, hotspot_y): (i32, i32)) -> CursorShape {
    let (width, height) = (pointer.width() as u32, pointer.height() as u32);
    let clamp = |value: i32, size: u32| u32::try_from(value).unwrap_or(0).min(size - 1);
    CursorShape {
        width,
        height,
        hotspot: (clamp(hotspot_x, width), clamp(hotspot_y, height)),
        pixels: pointer.to_rgba(),
    }
}

/// Starts duplicating `output`'s desktop on `device`.
fn duplicate(
    output: &IDXGIOutput1,
    device: &ID3D11Device,
) -> Result<IDXGIOutputDuplication, String> {
    // SAFETY: the output and device are valid.
    unsafe { output.DuplicateOutput(device) }.map_err(|error| {
        if error.code() == E_ACCESSDENIED {
            "Windows does not allow capturing the screen right now (the secure desktop is shown)"
                .to_owned()
        } else {
            format!("cannot capture the screen: {error}")
        }
    })
}

impl FrameSource for DesktopDuplication {
    fn size(&self) -> (usize, usize) {
        self.size
    }

    fn capture(&mut self) -> Result<Yuv420Frame, RuntimeError> {
        self.acquire(0).map_err(RuntimeError::Capture)?;
        // A display that changes mode is duplicated again at its new size,
        // which the host follows with a new stream.
        self.size = (self.desktop.width, self.desktop.height);
        if self.frame.is_none() {
            let desktop = &self.desktop;
            self.frame = Yuv420Frame::from_rgb32(
                desktop.width,
                desktop.height,
                desktop.stride(),
                Rgb32Order::Bgr,
                &desktop.data,
            );
        }
        self.frame
            .clone()
            .ok_or_else(|| RuntimeError::Capture("the screen image is unreadable".to_owned()))
    }

    fn cursor(&mut self) -> Option<CursorState> {
        Some(self.pointer.state())
    }
}
