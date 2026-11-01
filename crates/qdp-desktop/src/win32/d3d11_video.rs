//! Drawing a client's video into a window with Direct3D 11, on a thread of
//! its own.
//!
//! Pictures a D3D11 video decoder decoded are drawn where it left them, from
//! the NV12 slice of its texture array; pictures in memory are uploaded into
//! three plane textures first. A shader converts limited-range BT.709 to RGB
//! as it scales, in the same pass that draws the overlay, such as the host's
//! cursor.
//!
//! The window presents with the flip model, so that a full screen window's
//! frames go straight to the display (independent flip) rather than through
//! the desktop compositor. With [`SyncMode::VSync`], the render thread waits
//! for the swap chain's latency object, which allows one frame queued, and
//! only then takes the newest picture, so that what is shown at each refresh
//! is as new as it can be; with [`SyncMode::Tearing`], it presents each
//! picture as soon as it is drawn, tearing where the display allows it.

use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use qdp_codec::{D3d11Picture, DecodedPicture, Yuv420Frame};
use tracing::{debug, info, warn};
use windows::Win32::Foundation::{HANDLE, HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP, D3D_SRV_DIMENSION_TEXTURE2D,
    D3D_SRV_DIMENSION_TEXTURE2DARRAY, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_CONSTANT_BUFFER, D3D11_BIND_SHADER_RESOURCE, D3D11_BLEND_DESC,
    D3D11_BLEND_INV_SRC_ALPHA, D3D11_BLEND_ONE, D3D11_BLEND_OP_ADD, D3D11_BLEND_SRC_ALPHA,
    D3D11_BUFFER_DESC, D3D11_COLOR_WRITE_ENABLE_ALL, D3D11_COMPARISON_NEVER,
    D3D11_CPU_ACCESS_WRITE, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CULL_NONE, D3D11_FILL_SOLID,
    D3D11_FILTER_MIN_MAG_MIP_LINEAR, D3D11_MAP_WRITE_DISCARD, D3D11_MAPPED_SUBRESOURCE,
    D3D11_RASTERIZER_DESC, D3D11_RENDER_TARGET_BLEND_DESC, D3D11_SAMPLER_DESC, D3D11_SDK_VERSION,
    D3D11_SHADER_RESOURCE_VIEW_DESC, D3D11_SHADER_RESOURCE_VIEW_DESC_0, D3D11_TEX2D_ARRAY_SRV,
    D3D11_TEX2D_SRV, D3D11_TEXTURE_ADDRESS_CLAMP, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_DYNAMIC, D3D11_VIEWPORT, D3D11CreateDevice, ID3D11BlendState, ID3D11Buffer,
    ID3D11Device, ID3D11DeviceContext, ID3D11PixelShader, ID3D11RasterizerState,
    ID3D11RenderTargetView, ID3D11SamplerState, ID3D11ShaderResourceView, ID3D11Texture2D,
    ID3D11VertexShader,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12,
    DXGI_FORMAT_R8_UNORM, DXGI_FORMAT_R8G8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_UNKNOWN,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    DXGI_FEATURE_PRESENT_ALLOW_TEARING, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT,
    DXGI_PRESENT_ALLOW_TEARING, DXGI_SCALING_NONE, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIDevice, IDXGIFactory2,
    IDXGIFactory5, IDXGISwapChain2,
};
use windows::Win32::System::Threading::WaitForSingleObjectEx;
use windows::core::{BOOL, Interface, PCSTR, s};

use crate::video_scene::{Fate, OVERLAY_LAYERS, Overlay, PixelRect, Scene};

/// The render thread's name.
const RENDER_THREAD: &str = "qdp-d3d11-render";

/// The vertex shader: a quad filling the viewport, its texture coordinates
/// scaled by the extent of the textures shown, which leaves out a decoder's
/// padding.
const VERTEX_SHADER: &str = r"
cbuffer Extent : register(b0) {
    float2 extent;
    float2 unused;
};

struct Corner {
    float4 position : SV_Position;
    float2 coordinate : TEXCOORD0;
};

Corner qdp_quad(uint index : SV_VertexID) {
    float2 unit = float2(index & 1, index >> 1);
    Corner corner;
    corner.position = float4(unit.x * 2.0 - 1.0, 1.0 - unit.y * 2.0, 0.0, 1.0);
    corner.coordinate = unit * extent;
    return corner;
}
";

/// What the pixel shaders share: the vertex shader's output, the sampler,
/// and limited-range BT.709 to RGB, with the coefficients of the Metal
/// presenter and of the processor's conversion.
const PIXEL_COMMON: &str = r"
struct Corner {
    float4 position : SV_Position;
    float2 coordinate : TEXCOORD0;
};

SamplerState bilinear : register(s0);

float4 qdp_bt709(float luma, float blue, float red) {
    float y = (luma * 255.0 - 16.0) / 219.0;
    float cb = (blue * 255.0 - 128.0) / 224.0;
    float cr = (red * 255.0 - 128.0) / 224.0;
    float3 rgb = float3(y + 1.5748 * cr, y - 0.1873 * cb - 0.4681 * cr, y + 1.8556 * cb);
    return float4(saturate(rgb), 1.0);
}
";

/// The pixel shader of NV12 pictures: luma and interleaved chroma.
const NV12_SHADER: &str = r"
Texture2D<float> luma : register(t0);
Texture2D<float2> chroma : register(t1);

float4 qdp_nv12(Corner corner) : SV_Target {
    float2 cbcr = chroma.Sample(bilinear, corner.coordinate);
    return qdp_bt709(luma.Sample(bilinear, corner.coordinate), cbcr.x, cbcr.y);
}
";

/// The pixel shader of pictures uploaded from memory: three planes.
const PLANAR_SHADER: &str = r"
Texture2D<float> luma : register(t0);
Texture2D<float> blue : register(t1);
Texture2D<float> red : register(t2);

float4 qdp_planar(Corner corner) : SV_Target {
    return qdp_bt709(
        luma.Sample(bilinear, corner.coordinate),
        blue.Sample(bilinear, corner.coordinate),
        red.Sample(bilinear, corner.coordinate));
}
";

/// The pixel shader of the overlay, blended with straight alpha.
const OVERLAY_SHADER: &str = r"
Texture2D<float4> image : register(t0);

float4 qdp_overlay(Corner corner) : SV_Target {
    return image.Sample(bilinear, corner.coordinate);
}
";

/// The vertices of a quad, drawn as a triangle strip.
const QUAD_VERTICES: u32 = 4;

/// The swap chain's buffers: one shown, one queued and one drawn into, with
/// one more where tearing presents without waiting.
const VSYNC_BUFFERS: u32 = 3;

/// The swap chain's buffers when tearing.
const TEARING_BUFFERS: u32 = 4;

/// The frames that may be queued for the display ahead of the one shown.
const MAX_FRAME_LATENCY: u32 = 1;

/// The longest the render thread waits for a free buffer before drawing
/// anyway, in milliseconds: a display that stopped refreshing, as a minimized
/// window's may, must not stop the drawing.
const LATENCY_WAIT_MS: u32 = 100;

/// The background around the picture.
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// How the swap chain meets the display's refresh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SyncMode {
    /// Each frame waits for the next refresh, showing the newest picture
    /// then: no tearing, at most a refresh of delay.
    #[default]
    VSync,
    /// Each picture is shown as soon as it is drawn, tearing where the
    /// display does not refresh variably: the least delay.
    Tearing,
}

/// What the session's thread asks the render thread for.
struct Request<T> {
    /// The newest picture not yet taken, and its tag.
    picture: Option<(DecodedPicture, T)>,
    /// The tags of pictures replaced before the render thread took them.
    replaced: Vec<T>,
    /// What to draw, once asked.
    scene: Option<Scene>,
    /// Whether a drawing is asked for.
    pending: bool,
    /// Whether the thread is to end.
    stop: bool,
}

/// The request and the condition the render thread waits on for it.
struct Shared<T> {
    /// The request.
    request: Mutex<Request<T>>,
    /// Signalled when the request changes.
    changed: Condvar,
}

impl<T> Shared<T> {
    /// Locks the request.
    fn lock(&self) -> MutexGuard<'_, Request<T>> {
        self.request.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Draws a client's video into a window with Direct3D 11, on a thread of its
/// own, reporting what became of each picture, tagged with a `T`.
pub struct D3d11Video<T: Send + 'static> {
    /// What the render thread is asked for.
    shared: Arc<Shared<T>>,
    /// The render thread.
    thread: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> std::fmt::Debug for D3d11Video<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("D3d11Video").finish_non_exhaustive()
    }
}

/// Hands pictures to a [`D3d11Video`]'s render thread from any thread, such
/// as a session's, so that they need not pass through the window's.
pub struct Submitter<T> {
    /// What the render thread is asked for.
    shared: Arc<Shared<T>>,
}

impl<T> Clone for Submitter<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> std::fmt::Debug for Submitter<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Submitter").finish_non_exhaustive()
    }
}

impl<T> Submitter<T> {
    /// Asks for `scene` to be drawn with `picture`, tagged, as the picture
    /// from now on if there is one: the render thread draws the newest it
    /// was given when it next draws, and this returns at once.
    pub fn submit(&self, picture: Option<(DecodedPicture, T)>, scene: Scene) {
        let mut request = self.shared.lock();
        if let Some(picture) = picture {
            if let Some((_, replaced)) = request.picture.replace(picture) {
                request.replaced.push(replaced);
            }
        }
        request.scene = Some(scene);
        request.pending = true;
        drop(request);
        self.shared.changed.notify_one();
    }
}

impl<T: Send + 'static> D3d11Video<T> {
    /// Draws into the window `hwnd`, as `sync` says, telling `report`, on
    /// the render thread, what became of each picture.
    ///
    /// # Safety
    ///
    /// `hwnd` must be a window that outlives the returned value.
    pub unsafe fn attach(
        hwnd: isize,
        sync: SyncMode,
        mut report: impl FnMut(Fate<T>) + Send + 'static,
    ) -> Result<Self, String> {
        let renderer = Renderer::new(HWND(hwnd as *mut _), sync)?;
        let shared = Arc::new(Shared {
            request: Mutex::new(Request {
                picture: None,
                replaced: Vec::new(),
                scene: None,
                pending: false,
                stop: false,
            }),
            changed: Condvar::new(),
        });
        let thread = std::thread::Builder::new()
            .name(RENDER_THREAD.to_owned())
            .spawn({
                let shared = Arc::clone(&shared);
                move || render(renderer, &shared, &mut report)
            })
            .map_err(|error| format!("cannot start the render thread: {error}"))?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Returns a handle that hands pictures to the render thread from any
    /// thread.
    pub fn submitter(&self) -> Submitter<T> {
        Submitter {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Asks for `scene` to be drawn with `picture`, as
    /// [`Submitter::submit`] does.
    pub fn show(&mut self, picture: Option<(DecodedPicture, T)>, scene: Scene) {
        self.submitter().submit(picture, scene);
    }
}

impl<T: Send + 'static> Drop for D3d11Video<T> {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.changed.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The render thread: draws what is asked, as the swap chain allows.
fn render<T>(mut renderer: Renderer, shared: &Shared<T>, report: &mut impl FnMut(Fate<T>)) {
    loop {
        let mut request = shared.lock();
        while !request.pending && !request.stop {
            request = shared
                .changed
                .wait(request)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if request.stop {
            return;
        }
        drop(request);
        // Only once a buffer is free is the newest picture taken, so that a
        // picture that arrives while waiting is the one drawn.
        let waited = renderer.wait_for_buffer();
        let mut request = shared.lock();
        let picture = request.picture.take();
        let replaced = std::mem::take(&mut request.replaced);
        let scene = request.scene.take();
        request.pending = false;
        drop(request);
        for tag in replaced {
            report(Fate::Unshown(tag));
        }
        let Some(scene) = scene else {
            continue;
        };
        let started = Instant::now();
        let (picture, tag) = match picture {
            Some((picture, tag)) => (Some(picture), Some(tag)),
            None => (None, None),
        };
        match renderer.draw(&scene, picture) {
            Ok(drew_in) => {
                if let Some(tag) = tag {
                    report(Fate::Shown {
                        tag,
                        at: Instant::now(),
                        drew_in: drew_in.unwrap_or_else(|| started.elapsed()),
                        waited,
                    });
                }
            }
            Err(error) => {
                warn!(error, "a frame could not be drawn");
                if let Some(tag) = tag {
                    report(Fate::Unshown(tag));
                }
            }
        }
    }
}

/// The views of a decoder's texture array that the NV12 shader samples, a
/// pair for each slice drawn so far.
struct Nv12Views {
    /// The texture array.
    texture: ID3D11Texture2D,
    /// The luma and chroma views of each slice, made when it is first drawn.
    slices: Vec<Option<[ID3D11ShaderResourceView; 2]>>,
}

/// A texture of the window's own that a slice is copied into, for drivers
/// whose decoder textures shaders cannot sample.
struct CopyTarget {
    /// The texture's width and height.
    size: (u32, u32),
    /// The texture.
    texture: ID3D11Texture2D,
    /// Its luma and chroma views.
    views: [ID3D11ShaderResourceView; 2],
}

/// The textures pictures in memory are uploaded into.
struct PlaneTextures {
    /// The picture's width and height.
    size: (usize, usize),
    /// The luma, blue and red planes.
    textures: [ID3D11Texture2D; 3],
    /// Their views.
    views: [ID3D11ShaderResourceView; 3],
}

/// The picture drawn, kept so that a frame without a new one draws it again.
enum Current {
    /// A decoder's picture, its views made, and the part of its texture
    /// shown.
    Decoded {
        /// The picture, held so that its slice is not decoded into while it
        /// is shown.
        _picture: D3d11Picture,
        /// Its luma and chroma views.
        views: [ID3D11ShaderResourceView; 2],
        /// The part of the texture shown, as fractions of its width and
        /// height.
        extent: [f32; 2],
    },
    /// The picture uploaded into the plane textures.
    Uploaded,
}

/// The overlay's texture and the pixels it holds.
struct OverlayTexture {
    /// Its width and height.
    size: (usize, usize),
    /// The texture.
    texture: ID3D11Texture2D,
    /// Its view.
    view: ID3D11ShaderResourceView,
    /// The pixels uploaded, to tell a new image from the same one.
    rgba: Arc<[u8]>,
}

/// Draws the video on the render thread: the device, the swap chain, the
/// shaders and the textures of the pictures drawn.
struct Renderer {
    /// The device: the decoders', so that their pictures are drawn without
    /// a copy.
    device: ID3D11Device,
    /// Its immediate context, which the device makes thread safe.
    context: ID3D11DeviceContext,
    /// The swap chain.
    swap_chain: IDXGISwapChain2,
    /// The swap chain's flags, which resizing must repeat.
    flags: DXGI_SWAP_CHAIN_FLAG,
    /// How frames meet the display's refresh.
    sync: SyncMode,
    /// Whether the display allows tearing.
    tearing: bool,
    /// Signalled when the swap chain has a buffer free.
    latency: HANDLE,
    /// The swap chain's buffer size.
    size: (u32, u32),
    /// The view of the buffer drawn into, made when first needed after each
    /// resize.
    target: Option<ID3D11RenderTargetView>,
    /// The vertex shader.
    vertex: ID3D11VertexShader,
    /// The NV12 pixel shader.
    nv12: ID3D11PixelShader,
    /// The three-plane pixel shader.
    planar: ID3D11PixelShader,
    /// The overlay's pixel shader.
    overlay: ID3D11PixelShader,
    /// Samples bilinearly, clamped to the edges.
    sampler: ID3D11SamplerState,
    /// Draws both sides of the quad.
    rasterizer: ID3D11RasterizerState,
    /// Blends the overlay with straight alpha.
    blend: ID3D11BlendState,
    /// The extent of the textures shown.
    extent: ID3D11Buffer,
    /// The views of the decoder's texture array.
    nv12_views: Option<Nv12Views>,
    /// The texture slices are copied into, where the decoder's cannot be
    /// sampled.
    copy_target: Option<CopyTarget>,
    /// The textures pictures in memory are uploaded into.
    planes: Option<PlaneTextures>,
    /// The picture drawn.
    current: Option<Current>,
    /// The textures of the layers over the picture, once each was drawn.
    overlay_textures: [Option<OverlayTexture>; OVERLAY_LAYERS],
}

// SAFETY: Direct3D 11 devices, their objects and DXGI swap chains may be used
// from any thread, one at a time, and the device's immediate context is
// thread safe; the renderer is moved to the render thread once made, and used
// there alone.
unsafe impl Send for Renderer {}

/// Returns an error describing a failed call.
fn failed(what: &'static str) -> impl Fn(windows::core::Error) -> String {
    move |error| format!("{what}: {error}")
}

impl Renderer {
    /// Makes the swap chain of `hwnd` and the drawing's objects.
    fn new(hwnd: HWND, sync: SyncMode) -> Result<Self, String> {
        let device = match qdp_codec::video_device() {
            Ok(device) => device,
            Err(error) => {
                debug!(%error, "no video device; drawing on a device of the window's own");
                plain_device()?
            }
        };
        // SAFETY: the device's immediate context is valid while it is.
        let context =
            unsafe { device.GetImmediateContext() }.map_err(failed("the device has no context"))?;
        let dxgi: IDXGIDevice = device.cast().map_err(failed("the device is not DXGI's"))?;
        // SAFETY: the DXGI device's adapter and its factory are valid while it
        // is.
        let factory: IDXGIFactory2 =
            unsafe { dxgi.GetAdapter().and_then(|adapter| adapter.GetParent()) }
                .map_err(failed("cannot find the DXGI factory"))?;
        let tearing = allows_tearing(&factory);
        let mut flags = DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT;
        if tearing {
            flags |= DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING;
        }
        let description = DXGI_SWAP_CHAIN_DESC1 {
            // The window's size, as the swap chain finds it.
            Width: 0,
            Height: 0,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: if sync == SyncMode::Tearing {
                TEARING_BUFFERS
            } else {
                VSYNC_BUFFERS
            },
            // Without scaling, a buffer the window's size can be flipped
            // straight to the display.
            Scaling: DXGI_SCALING_NONE,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: flags.0 as u32,
        };
        // SAFETY: the window is valid, as the caller ensures, and the
        // description valid.
        let swap_chain =
            unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &description, None, None) }
                .map_err(failed("cannot make the window's swap chain"))?;
        // Alt+Enter is the app's, not DXGI's exclusive full screen.
        // SAFETY: the window is valid.
        if let Err(error) = unsafe { factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER) } {
            debug!(%error, "DXGI keeps Alt+Enter");
        }
        let swap_chain: IDXGISwapChain2 = swap_chain
            .cast()
            .map_err(failed("the swap chain has no latency object"))?;
        // SAFETY: the swap chain was made with a latency object.
        let latency = unsafe {
            swap_chain
                .SetMaximumFrameLatency(MAX_FRAME_LATENCY)
                .map_err(failed("cannot limit the frame latency"))?;
            swap_chain.GetFrameLatencyWaitableObject()
        };
        // SAFETY: the swap chain is valid.
        let size_description = unsafe { swap_chain.GetDesc1() }
            .map_err(failed("cannot read the swap chain's size"))?;
        let vertex_code = compile(VERTEX_SHADER, s!("qdp_quad"), s!("vs_4_0"))?;
        let pixel = |source: &str, entry: PCSTR| -> Result<ID3D11PixelShader, String> {
            let code = compile(&format!("{PIXEL_COMMON}{source}"), entry, s!("ps_4_0"))?;
            let mut shader = None;
            // SAFETY: the code is a compiled pixel shader.
            unsafe { device.CreatePixelShader(blob_bytes(&code), None, Some(&mut shader)) }
                .map_err(failed("cannot make a pixel shader"))?;
            shader.ok_or_else(|| "Direct3D returned no pixel shader".to_owned())
        };
        let mut vertex = None;
        // SAFETY: the code is a compiled vertex shader.
        unsafe { device.CreateVertexShader(blob_bytes(&vertex_code), None, Some(&mut vertex)) }
            .map_err(failed("cannot make the vertex shader"))?;
        let renderer = Self {
            nv12: pixel(NV12_SHADER, s!("qdp_nv12"))?,
            planar: pixel(PLANAR_SHADER, s!("qdp_planar"))?,
            overlay: pixel(OVERLAY_SHADER, s!("qdp_overlay"))?,
            vertex: vertex.ok_or_else(|| "Direct3D returned no vertex shader".to_owned())?,
            sampler: sampler(&device)?,
            rasterizer: rasterizer(&device)?,
            blend: blend(&device)?,
            extent: constant_buffer(&device)?,
            device,
            context,
            swap_chain,
            flags,
            sync,
            tearing,
            latency,
            size: (size_description.Width, size_description.Height),
            target: None,
            nv12_views: None,
            copy_target: None,
            planes: None,
            current: None,
            overlay_textures: [None, None],
        };
        info!(?sync, tearing, "drawing the video with Direct3D 11");
        Ok(renderer)
    }

    /// Waits until the swap chain has a buffer free, in step with the
    /// display, and returns how long that took; with tearing, presents do
    /// not wait for the display, and neither does this.
    fn wait_for_buffer(&self) -> Duration {
        let started = Instant::now();
        if self.sync == SyncMode::VSync {
            // SAFETY: the latency object is valid while the swap chain is.
            unsafe { WaitForSingleObjectEx(self.latency, LATENCY_WAIT_MS, true) };
        }
        started.elapsed()
    }

    /// Draws `scene` with `picture`, the new picture if there is one, and
    /// presents it, returning how long the drawing itself took.
    fn draw(
        &mut self,
        scene: &Scene,
        picture: Option<DecodedPicture>,
    ) -> Result<Option<Duration>, String> {
        let started = Instant::now();
        self.resize(scene.size)?;
        if let Some(picture) = picture {
            self.take(picture)?;
        }
        let target = self.target()?;
        let context = self.context.clone();
        // SAFETY: every object bound is the renderer's and valid; the
        // viewports lie within the target.
        unsafe {
            context.OMSetRenderTargets(Some(&[Some(target.clone())]), None);
            context.ClearRenderTargetView(&target, &BLACK);
            context.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLESTRIP);
            context.IASetInputLayout(None);
            context.VSSetShader(&self.vertex, None);
            context.VSSetConstantBuffers(0, Some(&[Some(self.extent.clone())]));
            context.RSSetState(&self.rasterizer);
            context.PSSetSamplers(0, Some(&[Some(self.sampler.clone())]));
            context.OMSetBlendState(None, None, u32::MAX);
        }
        if let (Some(rect), Some(current)) = (scene.picture, &self.current) {
            let (shader, views, extent): (_, Vec<Option<ID3D11ShaderResourceView>>, _) =
                match current {
                    Current::Decoded { views, extent, .. } => (
                        &self.nv12,
                        views.iter().cloned().map(Some).collect(),
                        *extent,
                    ),
                    Current::Uploaded => {
                        let planes = self.planes.as_ref().ok_or("no picture uploaded")?;
                        (
                            &self.planar,
                            planes.views.iter().cloned().map(Some).collect(),
                            [1.0, 1.0],
                        )
                    }
                };
            self.draw_quad(rect, shader, &views, extent);
        }
        for (layer, overlay) in scene.layers().into_iter().enumerate() {
            if let Some(overlay) = overlay {
                self.draw_overlay(layer, overlay)?;
            }
        }
        // SAFETY: the target was drawn into; unbinding leaves the decoder's
        // textures unbound while it decodes into them.
        unsafe {
            context.PSSetShaderResources(0, Some(&[None, None, None]));
        }
        let drew_in = started.elapsed();
        let (interval, flags) = match self.sync {
            SyncMode::VSync => (1, DXGI_PRESENT(0)),
            SyncMode::Tearing if self.tearing => (0, DXGI_PRESENT_ALLOW_TEARING),
            SyncMode::Tearing => (0, DXGI_PRESENT(0)),
        };
        // SAFETY: the swap chain is valid; tearing is asked for only when the
        // swap chain was made to allow it.
        unsafe { self.swap_chain.Present(interval, flags) }
            .ok()
            .map_err(failed("cannot present"))?;
        Ok(Some(drew_in))
    }

    /// Draws a quad of `rect` with `shader`, sampling `views`.
    fn draw_quad(
        &self,
        rect: PixelRect,
        shader: &ID3D11PixelShader,
        views: &[Option<ID3D11ShaderResourceView>],
        extent: [f32; 2],
    ) {
        let viewport = D3D11_VIEWPORT {
            TopLeftX: rect.left as f32,
            TopLeftY: rect.top as f32,
            Width: rect.width as f32,
            Height: rect.height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        let constants = [extent[0], extent[1], 0.0, 0.0];
        // SAFETY: the buffer holds four floats, as `constants` does; the
        // shader, views and viewport are valid.
        unsafe {
            self.context
                .UpdateSubresource(&self.extent, 0, None, constants.as_ptr().cast(), 0, 0);
            self.context.RSSetViewports(Some(&[viewport]));
            self.context.PSSetShader(shader, None);
            self.context.PSSetShaderResources(0, Some(views));
            self.context.Draw(QUAD_VERTICES, 0);
        }
    }

    /// Draws `overlay` as layer `layer` over the picture, uploading its
    /// image if it changed.
    fn draw_overlay(&mut self, layer: usize, overlay: &Overlay) -> Result<(), String> {
        if overlay.width == 0 || overlay.height == 0 {
            return Ok(());
        }
        let slot = &mut self.overlay_textures[layer];
        let stale = slot.as_ref().is_none_or(|texture| {
            texture.size != (overlay.width, overlay.height)
                || !Arc::ptr_eq(&texture.rgba, &overlay.rgba)
        });
        if stale {
            let size = (overlay.width, overlay.height);
            let reusable = slot.take().filter(|texture| texture.size == size);
            let (texture, view) = match reusable {
                Some(texture) => (texture.texture, texture.view),
                None => dynamic_texture(&self.device, size, DXGI_FORMAT_R8G8B8A8_UNORM)?,
            };
            self.upload(&texture, &overlay.rgba, overlay.width * 4, overlay.height)?;
            self.overlay_textures[layer] = Some(OverlayTexture {
                size,
                texture,
                view,
                rgba: Arc::clone(&overlay.rgba),
            });
        }
        let Some(texture) = &self.overlay_textures[layer] else {
            return Ok(());
        };
        let rect = PixelRect {
            left: overlay.left,
            top: overlay.top,
            width: overlay.width as f64,
            height: overlay.height as f64,
        };
        // SAFETY: the blend state is valid.
        unsafe { self.context.OMSetBlendState(&self.blend, None, u32::MAX) };
        self.draw_quad(
            rect,
            &self.overlay,
            &[Some(texture.view.clone())],
            [1.0, 1.0],
        );
        Ok(())
    }

    /// Makes `picture` the one drawn.
    fn take(&mut self, picture: DecodedPicture) -> Result<(), String> {
        match picture {
            DecodedPicture::Memory(frame) => {
                self.upload_frame(&frame)?;
                self.current = Some(Current::Uploaded);
            }
            DecodedPicture::D3d11(picture) => {
                let (views, extent) = self.decoded_views(&picture)?;
                self.current = Some(Current::Decoded {
                    _picture: picture,
                    views,
                    extent,
                });
            }
        }
        Ok(())
    }

    /// Returns the views a decoder's picture is drawn from, and the part of
    /// its texture shown, copying its slice first where its texture cannot
    /// be sampled.
    fn decoded_views(
        &mut self,
        picture: &D3d11Picture,
    ) -> Result<([ID3D11ShaderResourceView; 2], [f32; 2]), String> {
        let mut description = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: the texture is valid and the description valid to write.
        unsafe { picture.texture().GetDesc(&mut description) };
        let (width, height) = picture.visible_size();
        let extent = [
            width as f32 / description.Width as f32,
            height as f32 / description.Height as f32,
        ];
        if !picture.shader_readable() {
            let size = (description.Width, description.Height);
            if self
                .copy_target
                .as_ref()
                .is_none_or(|target| target.size != size)
            {
                let texture = make_texture(
                    &self.device,
                    size,
                    1,
                    DXGI_FORMAT_NV12,
                    D3D11_USAGE_DEFAULT.0,
                    D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    0,
                )?;
                let views = nv12_views(&self.device, &texture, None)?;
                self.copy_target = Some(CopyTarget {
                    size,
                    texture,
                    views,
                });
            }
            let target = self.copy_target.as_ref().ok_or("no copy target")?;
            // SAFETY: both textures are NV12 of the same size on the same
            // device, and the slice exists.
            unsafe {
                self.context.CopySubresourceRegion(
                    &target.texture,
                    0,
                    0,
                    0,
                    0,
                    picture.texture(),
                    picture.slice(),
                    None,
                );
            }
            return Ok((target.views.clone(), extent));
        }
        let same_texture = self
            .nv12_views
            .as_ref()
            .is_some_and(|views| views.texture == *picture.texture());
        if !same_texture {
            self.nv12_views = Some(Nv12Views {
                texture: picture.texture().clone(),
                slices: vec![None; description.ArraySize as usize],
            });
        }
        let cache = self.nv12_views.as_mut().ok_or("no decoder views")?;
        let slot = cache
            .slices
            .get_mut(picture.slice() as usize)
            .ok_or("a slice outside the decoder's texture")?;
        if slot.is_none() {
            *slot = Some(nv12_views(
                &self.device,
                picture.texture(),
                Some(picture.slice()),
            )?);
        }
        Ok((slot.clone().ok_or("no view of the slice")?, extent))
    }

    /// Uploads a picture in memory into the plane textures.
    fn upload_frame(&mut self, frame: &Yuv420Frame) -> Result<(), String> {
        let size = (frame.width(), frame.height());
        if self
            .planes
            .as_ref()
            .is_none_or(|planes| planes.size != size)
        {
            let chroma = (frame.chroma_width(), frame.chroma_height());
            let (luma, luma_view) = dynamic_texture(&self.device, size, DXGI_FORMAT_R8_UNORM)?;
            let (blue, blue_view) = dynamic_texture(&self.device, chroma, DXGI_FORMAT_R8_UNORM)?;
            let (red, red_view) = dynamic_texture(&self.device, chroma, DXGI_FORMAT_R8_UNORM)?;
            self.planes = Some(PlaneTextures {
                size,
                textures: [luma, blue, red],
                views: [luma_view, blue_view, red_view],
            });
        }
        let planes = self.planes.as_ref().ok_or("no plane textures")?;
        self.upload(
            &planes.textures[0],
            frame.y(),
            frame.width(),
            frame.height(),
        )?;
        self.upload(
            &planes.textures[1],
            frame.u(),
            frame.chroma_width(),
            frame.chroma_height(),
        )?;
        self.upload(
            &planes.textures[2],
            frame.v(),
            frame.chroma_width(),
            frame.chroma_height(),
        )
    }

    /// Writes `rows` rows of `row_bytes` bytes from `pixels` into a dynamic
    /// texture.
    fn upload(
        &self,
        texture: &ID3D11Texture2D,
        pixels: &[u8],
        row_bytes: usize,
        rows: usize,
    ) -> Result<(), String> {
        if pixels.len() < row_bytes * rows {
            return Err("a picture's plane is shorter than its size".to_owned());
        }
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: the texture is dynamic and writable by the processor; the
        // mapping holds `rows` rows of `RowPitch` bytes, at least `row_bytes`
        // each, until it is unmapped below.
        unsafe {
            self.context
                .Map(texture, 0, D3D11_MAP_WRITE_DISCARD, 0, Some(&mut mapped))
                .map_err(failed("cannot map a texture"))?;
            let pitch = mapped.RowPitch as usize;
            let destination =
                std::slice::from_raw_parts_mut(mapped.pData.cast::<u8>(), pitch * rows);
            for (row, source) in pixels.chunks(row_bytes).take(rows).enumerate() {
                destination[row * pitch..row * pitch + row_bytes].copy_from_slice(source);
            }
            self.context.Unmap(texture, 0);
        }
        Ok(())
    }

    /// Resizes the swap chain's buffers to `size`, if they differ.
    fn resize(&mut self, size: (u32, u32)) -> Result<(), String> {
        if size == self.size || size.0 == 0 || size.1 == 0 {
            return Ok(());
        }
        // Every view of the buffers must go before they can be resized.
        self.target = None;
        // SAFETY: the context is valid; unbinding the target releases the
        // context's hold on the buffers.
        unsafe {
            self.context.OMSetRenderTargets(None, None);
            self.context.Flush();
            self.swap_chain
                .ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, self.flags)
        }
        .map_err(failed("cannot resize the swap chain"))?;
        self.size = size;
        Ok(())
    }

    /// Returns the view of the buffer drawn into, making it if needed.
    fn target(&mut self) -> Result<ID3D11RenderTargetView, String> {
        if let Some(target) = &self.target {
            return Ok(target.clone());
        }
        // SAFETY: buffer 0 is the one drawn into, with the flip model.
        let buffer: ID3D11Texture2D = unsafe { self.swap_chain.GetBuffer(0) }
            .map_err(failed("cannot get the swap chain's buffer"))?;
        let mut target = None;
        // SAFETY: the buffer is a render target of the device.
        unsafe {
            self.device
                .CreateRenderTargetView(&buffer, None, Some(&mut target))
        }
        .map_err(failed("cannot draw into the swap chain"))?;
        let target = target.ok_or("Direct3D returned no render target")?;
        self.target = Some(target.clone());
        Ok(target)
    }
}

/// Returns whether the display allows presenting with tearing.
fn allows_tearing(factory: &IDXGIFactory2) -> bool {
    let Ok(factory) = factory.cast::<IDXGIFactory5>() else {
        return false;
    };
    let mut allowed = BOOL::default();
    // SAFETY: the feature's data is a BOOL, valid to write.
    unsafe {
        factory.CheckFeatureSupport(
            DXGI_FEATURE_PRESENT_ALLOW_TEARING,
            (&mut allowed as *mut BOOL).cast(),
            size_of::<BOOL>() as u32,
        )
    }
    .is_ok_and(|()| allowed.as_bool())
}

/// Makes a device of the window's own on the default adapter, for drawing
/// pictures in memory where no device decodes video.
fn plain_device() -> Result<ID3D11Device, String> {
    let mut device = None;
    // SAFETY: the output is valid to write; no adapter means the default.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            None,
        )
    }
    .map_err(failed("cannot open the graphics adapter"))?;
    device.ok_or_else(|| "Direct3D returned no device".to_owned())
}

/// Compiles HLSL `source`'s `entry` for `target`.
fn compile(source: &str, entry: PCSTR, target: PCSTR) -> Result<ID3DBlob, String> {
    let mut code = None;
    let mut errors = None;
    // SAFETY: the source is valid for its length, the names are
    // NUL-terminated, and the outputs valid to write.
    let compiled = unsafe {
        D3DCompile(
            source.as_ptr().cast(),
            source.len(),
            PCSTR::null(),
            None,
            None,
            entry,
            target,
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(error) = compiled {
        let message = errors
            .as_ref()
            .map(|errors| String::from_utf8_lossy(blob_bytes(errors)).into_owned())
            .unwrap_or_default();
        return Err(format!("cannot compile a shader: {error} {message}"));
    }
    code.ok_or_else(|| "the shader compiler returned no code".to_owned())
}

/// Returns a blob's bytes.
fn blob_bytes(blob: &ID3DBlob) -> &[u8] {
    // SAFETY: a blob's buffer is valid for its size while the blob is.
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    }
}

/// Makes a 2D texture.
fn make_texture(
    device: &ID3D11Device,
    (width, height): (u32, u32),
    slices: u32,
    format: DXGI_FORMAT,
    usage: i32,
    bind: u32,
    cpu_access: u32,
) -> Result<ID3D11Texture2D, String> {
    let description = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: slices,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: windows::Win32::Graphics::Direct3D11::D3D11_USAGE(usage),
        BindFlags: bind,
        CPUAccessFlags: cpu_access,
        MiscFlags: 0,
    };
    let mut texture = None;
    // SAFETY: the description is valid and the output valid to write.
    unsafe { device.CreateTexture2D(&description, None, Some(&mut texture)) }
        .map_err(failed("cannot make a texture"))?;
    texture.ok_or_else(|| "Direct3D returned no texture".to_owned())
}

/// Makes a texture the processor writes, and its view.
fn dynamic_texture(
    device: &ID3D11Device,
    (width, height): (usize, usize),
    format: DXGI_FORMAT,
) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView), String> {
    let texture = make_texture(
        device,
        (width as u32, height as u32),
        1,
        format,
        D3D11_USAGE_DYNAMIC.0,
        D3D11_BIND_SHADER_RESOURCE.0 as u32,
        D3D11_CPU_ACCESS_WRITE.0 as u32,
    )?;
    let mut view = None;
    // SAFETY: the texture may be sampled.
    unsafe { device.CreateShaderResourceView(&texture, None, Some(&mut view)) }
        .map_err(failed("cannot view a texture"))?;
    Ok((texture, view.ok_or("Direct3D returned no view")?))
}

/// Makes the luma and chroma views of an NV12 texture, or of one slice of an
/// NV12 texture array.
fn nv12_views(
    device: &ID3D11Device,
    texture: &ID3D11Texture2D,
    slice: Option<u32>,
) -> Result<[ID3D11ShaderResourceView; 2], String> {
    let view = |format: DXGI_FORMAT| -> Result<ID3D11ShaderResourceView, String> {
        let description = match slice {
            Some(slice) => D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: format,
                ViewDimension: D3D_SRV_DIMENSION_TEXTURE2DARRAY,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2DArray: D3D11_TEX2D_ARRAY_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                        FirstArraySlice: slice,
                        ArraySize: 1,
                    },
                },
            },
            None => D3D11_SHADER_RESOURCE_VIEW_DESC {
                Format: format,
                ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_SRV {
                        MostDetailedMip: 0,
                        MipLevels: 1,
                    },
                },
            },
        };
        let mut view = None;
        // SAFETY: the texture is NV12 and may be sampled; R8 views its luma
        // plane and R8G8 its chroma plane.
        unsafe { device.CreateShaderResourceView(texture, Some(&description), Some(&mut view)) }
            .map_err(failed("cannot view a decoded picture"))?;
        view.ok_or_else(|| "Direct3D returned no view".to_owned())
    };
    Ok([view(DXGI_FORMAT_R8_UNORM)?, view(DXGI_FORMAT_R8G8_UNORM)?])
}

/// Makes the bilinear sampler, clamped to the textures' edges.
fn sampler(device: &ID3D11Device) -> Result<ID3D11SamplerState, String> {
    let description = D3D11_SAMPLER_DESC {
        Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
        AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D11_COMPARISON_NEVER,
        BorderColor: [0.0; 4],
        MinLOD: 0.0,
        MaxLOD: f32::MAX,
    };
    let mut sampler = None;
    // SAFETY: the description is valid and the output valid to write.
    unsafe { device.CreateSamplerState(&description, Some(&mut sampler)) }
        .map_err(failed("cannot make the sampler"))?;
    sampler.ok_or_else(|| "Direct3D returned no sampler".to_owned())
}

/// Makes the rasterizer state, which draws both sides of a quad.
fn rasterizer(device: &ID3D11Device) -> Result<ID3D11RasterizerState, String> {
    let description = D3D11_RASTERIZER_DESC {
        FillMode: D3D11_FILL_SOLID,
        CullMode: D3D11_CULL_NONE,
        DepthClipEnable: true.into(),
        ..D3D11_RASTERIZER_DESC::default()
    };
    let mut state = None;
    // SAFETY: the description is valid and the output valid to write.
    unsafe { device.CreateRasterizerState(&description, Some(&mut state)) }
        .map_err(failed("cannot make the rasterizer state"))?;
    state.ok_or_else(|| "Direct3D returned no rasterizer state".to_owned())
}

/// Makes the blend state the overlay is drawn with: straight alpha.
fn blend(device: &ID3D11Device) -> Result<ID3D11BlendState, String> {
    let target = D3D11_RENDER_TARGET_BLEND_DESC {
        BlendEnable: true.into(),
        SrcBlend: D3D11_BLEND_SRC_ALPHA,
        DestBlend: D3D11_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D11_BLEND_OP_ADD,
        SrcBlendAlpha: D3D11_BLEND_ONE,
        DestBlendAlpha: D3D11_BLEND_INV_SRC_ALPHA,
        BlendOpAlpha: D3D11_BLEND_OP_ADD,
        RenderTargetWriteMask: D3D11_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    let description = D3D11_BLEND_DESC {
        AlphaToCoverageEnable: false.into(),
        IndependentBlendEnable: false.into(),
        RenderTarget: [target; 8],
    };
    let mut state = None;
    // SAFETY: the description is valid and the output valid to write.
    unsafe { device.CreateBlendState(&description, Some(&mut state)) }
        .map_err(failed("cannot make the blend state"))?;
    state.ok_or_else(|| "Direct3D returned no blend state".to_owned())
}

/// Makes the constant buffer of the extent: four floats.
fn constant_buffer(device: &ID3D11Device) -> Result<ID3D11Buffer, String> {
    let description = D3D11_BUFFER_DESC {
        ByteWidth: 4 * size_of::<f32>() as u32,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut buffer = None;
    // SAFETY: the description is valid and the output valid to write.
    unsafe { device.CreateBuffer(&description, None, Some(&mut buffer)) }
        .map_err(failed("cannot make the constant buffer"))?;
    buffer.ok_or_else(|| "Direct3D returned no buffer".to_owned())
}
