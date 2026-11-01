//! Showing a client's video in a window with Metal: the GPU converts each
//! picture's colours and scales it into the window in one pass, rather than
//! the processor doing both. A picture VideoToolbox decoded is drawn from
//! its own pixel buffer, whose planes Core Video shows Metal as textures,
//! without a copy; a picture in memory has its planes uploaded as textures.
//!
//! A `CAMetalLayer` is added over the layer of the window's view, as
//! softbuffer adds its own, and kept to the view's size. The black bars
//! around the picture are the layer's clear colour, and a cursor the client
//! draws is a second, blended quad. A display link declares the frame rates the video
//! wants, since a ProMotion display otherwise may not refresh faster than
//! 60 Hz for it.
//!
//! At most two drawings wait to reach the screen, which with the one shown
//! are all the drawables a layer has. A picture that arrives while two wait
//! is drawn once one is shown, rather than waiting for a free drawable on the
//! main thread, which also reads the user's input. With only one waiting,
//! a frame arriving a little early each refresh waited a refresh for the
//! next, and a display at 120 Hz showed as few as 40 a second.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use block2::RcBlock;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSView;
use objc2_core_foundation::{CFRetained, CGSize};
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceSRGB};
use objc2_core_video::{
    CVMetalTexture, CVMetalTextureCache, CVMetalTextureGetTexture, CVPixelBuffer,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetWidthOfPlane, kCVReturnSuccess,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes, NSString};
use objc2_metal::{
    MTLBlendFactor, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue,
    MTLCreateSystemDefaultDevice, MTLDevice, MTLDrawable, MTLLibrary, MTLLoadAction, MTLOrigin,
    MTLPixelFormat, MTLPrimitiveType, MTLRegion, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLSamplerAddressMode,
    MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerState, MTLSize, MTLStoreAction,
    MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};
use objc2_quartz_core::{
    CADisplayLink, CAFrameRateRange, CALayer, CAMetalDrawable, CAMetalLayer, CATransaction,
};
use qdp_codec::{DecodedPicture, PixelBufferPicture, Yuv420Frame};
use tracing::{debug, warn};

/// The shaders: a quad placed by a rectangle, the picture's colours from its
/// three planes or from NV12's two, and an overlay image. Their names start
/// with `qdp_`, since Metal reserves some plain ones, such as `quad`.
///
/// The picture is limited-range BT.709 YUV 4:2:0, the format QDP's standard
/// dynamic range streams use; the coefficients are those of the processor's
/// conversion.
const SHADERS: &str = r"
#include <metal_stdlib>
using namespace metal;

struct Corner {
    float4 position [[position]];
    float2 coordinate;
};

// The rectangle's left, top, right and bottom edges, from -1 to 1.
vertex Corner qdp_quad(uint index [[vertex_id]], constant float4 &rect [[buffer(0)]]) {
    float2 unit = float2(float(index & 1), float(index >> 1));
    Corner corner;
    corner.position = float4(mix(rect.x, rect.z, unit.x), mix(rect.y, rect.w, unit.y), 0.0, 1.0);
    corner.coordinate = unit;
    return corner;
}

// Limited-range BT.709 luma and chroma, as sampled, to RGB.
static float4 qdp_bt709(float luma, float blue, float red) {
    float y = (luma * 255.0 - 16.0) / 219.0;
    float cb = (blue * 255.0 - 128.0) / 224.0;
    float cr = (red * 255.0 - 128.0) / 224.0;
    float3 rgb = float3(y + 1.5748 * cr, y - 0.1873 * cb - 0.4681 * cr, y + 1.8556 * cb);
    return float4(saturate(rgb), 1.0);
}

fragment float4 qdp_picture(
    Corner corner [[stage_in]],
    texture2d<float> luma [[texture(0)]],
    texture2d<float> blue [[texture(1)]],
    texture2d<float> red [[texture(2)]],
    sampler bilinear [[sampler(0)]]
) {
    return qdp_bt709(
        luma.sample(bilinear, corner.coordinate).r,
        blue.sample(bilinear, corner.coordinate).r,
        red.sample(bilinear, corner.coordinate).r
    );
}

// The part of the textures shown, as fractions of their width and height,
// leaves out a decoder's padding.
fragment float4 qdp_nv12(
    Corner corner [[stage_in]],
    texture2d<float> luma [[texture(0)]],
    texture2d<float> chroma [[texture(1)]],
    sampler bilinear [[sampler(0)]],
    constant float2 &extent [[buffer(0)]]
) {
    float2 at = corner.coordinate * extent;
    float2 cbcr = chroma.sample(bilinear, at).rg;
    return qdp_bt709(luma.sample(bilinear, at).r, cbcr.x, cbcr.y);
}

fragment float4 qdp_overlay(
    Corner corner [[stage_in]],
    texture2d<float> image [[texture(0)]],
    sampler bilinear [[sampler(0)]]
) {
    return image.sample(bilinear, corner.coordinate);
}
";

/// The drawable's pixel format.
const DRAWABLE_FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;

/// The vertices of a quad, drawn as a triangle strip.
const QUAD_VERTICES: usize = 4;

/// The frame rates the video asks a ProMotion display for: up to 120 Hz.
const FRAME_RATES: CAFrameRateRange = CAFrameRateRange {
    minimum: 60.0,
    maximum: 120.0,
    preferred: 120.0,
};

/// The bytes of an RGBA pixel.
const RGBA_BYTES: usize = 4;

/// The sets of plane textures frames are uploaded into in turn, so that a
/// frame is never written into textures the GPU may still be drawing from.
const PLANE_SETS: usize = 3;

/// The pictures in pixel buffers kept after they are shown, each with its
/// textures, so that none is released, and its buffer decoded into again,
/// while drawings of it may still be on the GPU: one more than the
/// drawings a layer has in flight.
const PIXEL_BUFFER_PICTURES: usize = PLANE_SETS + 1;

/// The drawings that may wait to reach the screen at once: a layer's three
/// drawables but the one shown.
const WAITING_DRAWINGS: usize = 2;

/// How long a drawing may wait to reach the screen before the next is drawn
/// regardless, should macOS never say that it was shown.
const PRESENT_TIMEOUT: Duration = Duration::from_millis(250);

/// The luma plane of an NV12 pixel buffer.
const LUMA_PLANE: usize = 0;

/// The interleaved chroma plane of an NV12 pixel buffer.
const CHROMA_PLANE: usize = 1;

/// A rectangle in the drawable's pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelRect {
    /// Its left edge.
    pub left: f64,
    /// Its top edge.
    pub top: f64,
    /// Its width.
    pub width: f64,
    /// Its height.
    pub height: f64,
}

/// An image drawn over the picture, such as the host's cursor.
#[derive(Debug, Clone, Copy)]
pub struct Overlay<'a> {
    /// Its width in pixels.
    pub width: usize,
    /// Its height in pixels.
    pub height: usize,
    /// Its RGBA pixels with straight alpha, row by row.
    pub rgba: &'a [u8],
    /// Its top-left corner in the drawable's pixels.
    pub left: f64,
    /// Its top edge.
    pub top: f64,
}

/// A texture and the size it was made for.
struct SizedTexture {
    /// Its width and height in pixels.
    size: (usize, usize),
    /// The texture.
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
}

/// The textures of a frame's planes.
struct Planes {
    /// The luma plane.
    luma: SizedTexture,
    /// The blue-difference chroma plane.
    blue: SizedTexture,
    /// The red-difference chroma plane.
    red: SizedTexture,
}

/// A picture in a pixel buffer, with the textures that show Metal its
/// planes.
struct PixelBufferTextures {
    /// The picture, kept so that its buffer is not decoded into again while
    /// the textures show it.
    _picture: PixelBufferPicture,
    /// Core Video's textures, kept so that their Metal textures stay valid.
    _planes: [CFRetained<CVMetalTexture>; 2],
    /// The luma plane.
    luma: Retained<ProtocolObject<dyn MTLTexture>>,
    /// The interleaved chroma plane.
    chroma: Retained<ProtocolObject<dyn MTLTexture>>,
    /// The part of the textures shown, as fractions of their width and
    /// height.
    extent: [f32; 2],
}

/// Which picture is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// The one uploaded into the newest set of plane textures.
    Uploaded,
    /// The newest one in a pixel buffer.
    PixelBuffer,
}

/// The overlay's texture and the pixels it holds.
struct OverlayTexture {
    /// The texture.
    texture: SizedTexture,
    /// The pixels uploaded, to tell a new image from the same one.
    rgba: Vec<u8>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class has no
    // `Drop` implementation.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "QdpFrameRateDemand"]
    struct FrameRateDemand;

    unsafe impl NSObjectProtocol for FrameRateDemand {}

    impl FrameRateDemand {
        /// Called each refresh: the link only declares the rates wanted, and
        /// draws nothing.
        #[unsafe(method(step:))]
        fn step(&self, _link: &CADisplayLink) {}
    }
);

impl FrameRateDemand {
    /// Returns a target for a display link.
    fn new(main_thread: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(main_thread).set_ivars(());
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// What became of a drawing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drawing {
    /// The picture was drawn, and goes to the screen.
    Drawn,
    /// A drawing still waits to reach the screen: the window is woken to
    /// draw the picture once it has.
    Deferred,
}

/// The drawings committed but not yet on the screen, shared with the
/// handlers Metal calls once each is shown.
struct Presents {
    /// How many wait to reach the screen.
    waiting: AtomicUsize,
    /// Whether a drawing was put off until one reaches the screen.
    deferred: AtomicBool,
    /// Wakes the window to draw what was put off.
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Presents {
    /// Notes that a drawing reached the screen, or was passed over for a
    /// newer one, and wakes the window if a drawing was put off.
    fn presented(&self) {
        let _ = self
            .waiting
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |waiting| {
                waiting.checked_sub(1)
            });
        if self.deferred.swap(false, Ordering::AcqRel) {
            (self.wake)();
        }
    }

    /// Returns whether a drawing must wait for one already waiting to reach
    /// the screen, noting that the window is to be woken if so.
    fn must_wait(&self) -> bool {
        if self.waiting.load(Ordering::Acquire) < WAITING_DRAWINGS {
            return false;
        }
        self.deferred.store(true, Ordering::Release);
        // The drawing may have reached the screen meanwhile, its handler
        // finding nothing put off.
        if self.waiting.load(Ordering::Acquire) < WAITING_DRAWINGS {
            self.deferred.store(false, Ordering::Release);
            return false;
        }
        true
    }
}

/// Draws a client's video into a window's view with Metal.
pub struct MetalVideo {
    /// The view's own layer, which the drawing layer is kept to the size of.
    root: Retained<CALayer>,
    /// The layer drawn into, over the view's.
    layer: Retained<CAMetalLayer>,
    /// The GPU.
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    /// The queue the drawing goes through.
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    /// Draws the picture from its three planes.
    picture: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// Draws the picture from NV12's two planes.
    nv12: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// Draws the overlay, blended.
    overlay: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    /// Samples the textures bilinearly.
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    /// The sets of plane textures frames are uploaded into in turn.
    plane_sets: Vec<Planes>,
    /// The set holding the newest frame, once one arrived.
    newest: Option<usize>,
    /// Shows Metal the planes of pixel buffers, if Core Video can.
    texture_cache: Option<CFRetained<CVMetalTextureCache>>,
    /// The latest pictures in pixel buffers, the newest last.
    pixel_buffers: VecDeque<PixelBufferTextures>,
    /// Which picture is shown, once one arrived.
    shown: Option<Shown>,
    /// Whether a pixel buffer could not be shown to Metal, which is said
    /// once.
    import_failed: bool,
    /// The overlay's texture, once one was drawn.
    overlay_texture: Option<OverlayTexture>,
    /// The display link that declares the frame rates wanted, where macOS
    /// has them (14 and later).
    frame_rate_link: Option<Retained<CADisplayLink>>,
    /// The link's target, kept while the link is.
    _frame_rate_target: Option<Retained<FrameRateDemand>>,
    /// The drawings waiting to reach the screen.
    presents: Arc<Presents>,
    /// When the last drawing was committed.
    committed_at: Option<Instant>,
}

impl std::fmt::Debug for MetalVideo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("MetalVideo").finish_non_exhaustive()
    }
}

impl MetalVideo {
    /// Draws into the view `ns_view` points to, a window's `NSView`, whose
    /// pixels are `scale` times its points, calling `wake`, from any thread,
    /// to have the window draw again a picture it put off.
    ///
    /// # Safety
    ///
    /// `ns_view` must point to a live `NSView`, and this must be called and
    /// the result used on the main thread, while the view lives.
    pub unsafe fn attach(
        ns_view: NonNull<c_void>,
        scale: f64,
        wake: Box<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let main_thread = MainThreadMarker::new()
            .ok_or_else(|| "Metal drawing needs the main thread".to_owned())?;
        // SAFETY: the caller promises a live NSView.
        let view = unsafe { Retained::retain(ns_view.as_ptr().cast::<NSView>()) }
            .ok_or_else(|| "the window has no view".to_owned())?;
        let device = MTLCreateSystemDefaultDevice()
            .ok_or_else(|| "this Mac has no Metal device".to_owned())?;
        let queue = device
            .newCommandQueue()
            .ok_or_else(|| "cannot make a Metal command queue".to_owned())?;
        let library = device
            .newLibraryWithSource_options_error(&NSString::from_str(SHADERS), None)
            .map_err(|error| format!("cannot compile the video shaders: {error}"))?;
        let picture = pipeline(&device, &library, "qdp_picture", Blending::Opaque)?;
        let nv12 = pipeline(&device, &library, "qdp_nv12", Blending::Opaque)?;
        let texture_cache = texture_cache(&device);
        let overlay = pipeline(&device, &library, "qdp_overlay", Blending::Alpha)?;
        let sampler = linear_sampler(&device)?;

        // winit makes its views layer-backed.
        let root = view
            .layer()
            .ok_or_else(|| "the window's view has no layer".to_owned())?;
        let layer = CAMetalLayer::new();
        layer.setDevice(Some(&device));
        layer.setPixelFormat(DRAWABLE_FORMAT);
        layer.setFramebufferOnly(true);
        // BT.709's primaries are sRGB's, which a wide-gamut display would
        // otherwise oversaturate.
        // SAFETY: the name is one of Core Graphics' own colour spaces.
        let srgb = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }));
        layer.setColorspace(srgb.as_deref());
        // Frames are shown as soon as they are drawn, rather than held for
        // the next refresh, so that drawing never waits on the display.
        layer.setDisplaySyncEnabled(false);
        root.addSublayer(&layer);
        fit_layer(&root, &layer, scale);

        let (frame_rate_link, frame_rate_target) = match frame_rate_link(&view, main_thread) {
            Some((link, target)) => (Some(link), Some(target)),
            None => {
                debug!("this macOS cannot ask the display for frame rates");
                (None, None)
            }
        };
        Ok(Self {
            root,
            layer,
            device,
            queue,
            picture,
            nv12,
            overlay,
            sampler,
            plane_sets: Vec::new(),
            newest: None,
            texture_cache,
            pixel_buffers: VecDeque::new(),
            shown: None,
            import_failed: false,
            overlay_texture: None,
            frame_rate_link,
            _frame_rate_target: frame_rate_target,
            presents: Arc::new(Presents {
                waiting: AtomicUsize::new(0),
                deferred: AtomicBool::new(false),
                wake,
            }),
            committed_at: None,
        })
    }

    /// Returns when a drawing put off is drawn regardless, should macOS
    /// never say that the one before it reached the screen.
    pub fn retry_at(&self) -> Option<Instant> {
        self.presents
            .deferred
            .load(Ordering::Acquire)
            .then_some(self.committed_at?)
            .map(|committed_at| committed_at + PRESENT_TIMEOUT)
    }

    /// Returns whether the next drawing must wait for the one waiting to
    /// reach the screen, unless that has waited too long to count on.
    fn must_wait(&self) -> bool {
        let overdue = self
            .committed_at
            .is_some_and(|committed_at| committed_at.elapsed() >= PRESENT_TIMEOUT);
        if overdue {
            self.presents.waiting.store(0, Ordering::Release);
            self.presents.deferred.store(false, Ordering::Release);
            return false;
        }
        self.presents.must_wait()
    }

    /// Takes `picture` as the one drawn from now on: drawn from its pixel
    /// buffer where it has one, and uploaded otherwise.
    pub fn take(&mut self, picture: DecodedPicture) {
        match picture {
            DecodedPicture::Memory(frame) => self.upload(&frame),
            DecodedPicture::PixelBuffer(picture) => {
                let imported = self
                    .texture_cache
                    .as_deref()
                    .ok_or_else(|| "Core Video cannot show Metal pixel buffers".to_owned())
                    .and_then(|cache| plane_textures(cache, picture.pixel_buffer()));
                match imported {
                    Ok((planes, luma, chroma)) => {
                        let (width, height) = picture.visible_size();
                        let extent = [
                            width as f32 / luma.width().max(1) as f32,
                            height as f32 / luma.height().max(1) as f32,
                        ];
                        self.pixel_buffers.push_back(PixelBufferTextures {
                            _picture: picture,
                            _planes: planes,
                            luma,
                            chroma,
                            extent,
                        });
                        while self.pixel_buffers.len() > PIXEL_BUFFER_PICTURES {
                            self.pixel_buffers.pop_front();
                        }
                        if let Some(cache) = &self.texture_cache {
                            // Releases the textures of pictures no longer
                            // kept.
                            cache.flush(0);
                        }
                        self.shown = Some(Shown::PixelBuffer);
                    }
                    Err(error) => {
                        if !self.import_failed {
                            self.import_failed = true;
                            warn!(%error, "copying decoded pictures to draw them");
                        }
                        match picture.to_memory() {
                            Ok(frame) => self.upload(&frame),
                            Err(error) => warn!(%error, "cannot read the decoded picture"),
                        }
                    }
                }
            }
        }
    }

    /// Takes `frame` as the picture drawn from now on.
    pub fn upload(&mut self, frame: &Yuv420Frame) {
        let luma_size = (frame.width(), frame.height());
        let chroma_size = (frame.chroma_width(), frame.chroma_height());
        // A frame of another size needs textures of its size.
        if self
            .plane_sets
            .first()
            .is_some_and(|planes| planes.luma.size != luma_size || planes.blue.size != chroma_size)
        {
            self.plane_sets.clear();
            self.newest = None;
        }
        let next = self.newest.map_or(0, |newest| (newest + 1) % PLANE_SETS);
        if next == self.plane_sets.len() {
            let planes = (|| {
                Some(Planes {
                    luma: texture(&self.device, MTLPixelFormat::R8Unorm, luma_size)?,
                    blue: texture(&self.device, MTLPixelFormat::R8Unorm, chroma_size)?,
                    red: texture(&self.device, MTLPixelFormat::R8Unorm, chroma_size)?,
                })
            })();
            let Some(planes) = planes else {
                return;
            };
            self.plane_sets.push(planes);
        }
        let planes = &self.plane_sets[next];
        fill(&planes.luma, frame.y(), 1);
        fill(&planes.blue, frame.u(), 1);
        fill(&planes.red, frame.v(), 1);
        self.newest = Some(next);
        self.shown = Some(Shown::Uploaded);
    }

    /// Draws the picture at `picture` and `overlay` over it in a drawable of
    /// `size` pixels, `scale` of them to a point of the window, black around
    /// them, and presents it; or puts the drawing off while the one before
    /// it waits to reach the screen.
    pub fn draw(
        &mut self,
        size: (u32, u32),
        scale: f64,
        picture: Option<PixelRect>,
        overlay: Option<Overlay<'_>>,
    ) -> Result<Drawing, String> {
        if self.must_wait() {
            return Ok(Drawing::Deferred);
        }
        fit_layer(&self.root, &self.layer, scale);
        let drawable_size = CGSize::new(f64::from(size.0), f64::from(size.1));
        if self.layer.drawableSize() != drawable_size {
            self.layer.setDrawableSize(drawable_size);
        }
        if let Some(overlay) = overlay {
            self.take_overlay(overlay);
        }
        let drawable = self
            .layer
            .nextDrawable()
            .ok_or_else(|| "no drawable is free".to_owned())?;
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: a render pass has a first colour attachment.
        let attachment = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setTexture(Some(&drawable.texture()));
        attachment.setLoadAction(MTLLoadAction::Clear);
        attachment.setClearColor(MTLClearColor {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        });
        attachment.setStoreAction(MTLStoreAction::Store);
        let commands = self
            .queue
            .commandBuffer()
            .ok_or_else(|| "cannot make a Metal command buffer".to_owned())?;
        let encoder = commands
            .renderCommandEncoderWithDescriptor(&pass)
            .ok_or_else(|| "cannot encode the drawing".to_owned())?;
        let planes = self.newest.and_then(|newest| self.plane_sets.get(newest));
        let pixel_buffer = self.pixel_buffers.back();
        if let (Some(rect), Some(Shown::PixelBuffer), Some(textures)) =
            (picture, self.shown, pixel_buffer)
        {
            encoder.setRenderPipelineState(&self.nv12);
            quad(&encoder, rect, size);
            let extent = NonNull::from(&textures.extent).cast::<c_void>();
            // SAFETY: the textures, sampler and extent are those the shader
            // reads, at the indices it reads them from; the extent is the
            // `float2` it reads, copied during the call.
            unsafe {
                encoder.setFragmentTexture_atIndex(Some(&textures.luma), 0);
                encoder.setFragmentTexture_atIndex(Some(&textures.chroma), 1);
                encoder.setFragmentSamplerState_atIndex(Some(&self.sampler), 0);
                encoder.setFragmentBytes_length_atIndex(extent, size_of_val(&textures.extent), 0);
                encoder.drawPrimitives_vertexStart_vertexCount(
                    MTLPrimitiveType::TriangleStrip,
                    0,
                    QUAD_VERTICES,
                );
            }
        } else if let (Some(rect), Some(Shown::Uploaded), Some(planes)) =
            (picture, self.shown, planes)
        {
            encoder.setRenderPipelineState(&self.picture);
            quad(&encoder, rect, size);
            // SAFETY: the textures and sampler are those the shader reads,
            // at the indices it reads them from.
            unsafe {
                encoder.setFragmentTexture_atIndex(Some(&planes.luma.texture), 0);
                encoder.setFragmentTexture_atIndex(Some(&planes.blue.texture), 1);
                encoder.setFragmentTexture_atIndex(Some(&planes.red.texture), 2);
                encoder.setFragmentSamplerState_atIndex(Some(&self.sampler), 0);
                encoder.drawPrimitives_vertexStart_vertexCount(
                    MTLPrimitiveType::TriangleStrip,
                    0,
                    QUAD_VERTICES,
                );
            }
        }
        if let (Some(overlay), Some(texture)) = (overlay, &self.overlay_texture) {
            encoder.setRenderPipelineState(&self.overlay);
            let rect = PixelRect {
                left: overlay.left,
                top: overlay.top,
                width: overlay.width as f64,
                height: overlay.height as f64,
            };
            quad(&encoder, rect, size);
            // SAFETY: as for the picture.
            unsafe {
                encoder.setFragmentTexture_atIndex(Some(&texture.texture.texture), 0);
                encoder.setFragmentSamplerState_atIndex(Some(&self.sampler), 0);
                encoder.drawPrimitives_vertexStart_vertexCount(
                    MTLPrimitiveType::TriangleStrip,
                    0,
                    QUAD_VERTICES,
                );
            }
        }
        encoder.endEncoding();
        let drawable: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&*drawable);
        let presents = Arc::clone(&self.presents);
        let presented = RcBlock::new(move |_drawable: NonNull<ProtocolObject<dyn MTLDrawable>>| {
            presents.presented();
        });
        self.presents.waiting.fetch_add(1, Ordering::AcqRel);
        // SAFETY: the block is valid for the call, which copies it, and the
        // drawable is not presented yet.
        unsafe { drawable.addPresentedHandler(RcBlock::as_ptr(&presented)) };
        commands.presentDrawable(drawable);
        commands.commit();
        self.committed_at = Some(Instant::now());
        Ok(Drawing::Drawn)
    }

    /// Uploads `overlay`'s image, if it is not the one uploaded already.
    fn take_overlay(&mut self, overlay: Overlay<'_>) {
        let size = (overlay.width, overlay.height);
        if overlay.rgba.len() != overlay.width * overlay.height * RGBA_BYTES {
            return;
        }
        if self
            .overlay_texture
            .as_ref()
            .is_some_and(|texture| texture.texture.size == size && texture.rgba == overlay.rgba)
        {
            return;
        }
        let Some(texture) = texture(&self.device, MTLPixelFormat::RGBA8Unorm, size) else {
            return;
        };
        fill(&texture, overlay.rgba, RGBA_BYTES);
        self.overlay_texture = Some(OverlayTexture {
            texture,
            rgba: overlay.rgba.to_vec(),
        });
    }
}

impl Drop for MetalVideo {
    fn drop(&mut self) {
        if let Some(link) = &self.frame_rate_link {
            link.invalidate();
        }
    }
}

/// Keeps `layer` to the bounds of `root`, the view's layer, at `scale`
/// pixels to a point, which the window's resizing and moving between
/// displays change, without animating the change.
fn fit_layer(root: &CALayer, layer: &CAMetalLayer, scale: f64) {
    let bounds = root.bounds();
    if layer.frame() == bounds && layer.contentsScale() == scale {
        return;
    }
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    layer.setFrame(bounds);
    layer.setContentsScale(scale);
    CATransaction::commit();
}

/// How a pipeline writes its colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Blending {
    /// Over what is there.
    Opaque,
    /// Blended by their straight alpha.
    Alpha,
}

/// Returns the pipeline that draws a quad with the fragment shader
/// `fragment` of `library`.
fn pipeline(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    fragment: &str,
    blending: Blending,
) -> Result<Retained<ProtocolObject<dyn MTLRenderPipelineState>>, String> {
    let function = |name: &str| {
        library
            .newFunctionWithName(&NSString::from_str(name))
            .ok_or_else(|| format!("the video shaders have no {name} function"))
    };
    let descriptor = MTLRenderPipelineDescriptor::new();
    let (vertex, fragment_function) = (function("qdp_quad")?, function(fragment)?);
    descriptor.setVertexFunction(Some(&vertex));
    descriptor.setFragmentFunction(Some(&fragment_function));
    // SAFETY: a pipeline has a first colour attachment.
    let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    attachment.setPixelFormat(DRAWABLE_FORMAT);
    if blending == Blending::Alpha {
        attachment.setBlendingEnabled(true);
        attachment.setSourceRGBBlendFactor(MTLBlendFactor::SourceAlpha);
        attachment.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        attachment.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    }
    device
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .map_err(|error| format!("cannot make the {fragment} pipeline: {error}"))
}

/// Returns a sampler that blends neighbouring pixels and clamps at the
/// edges.
fn linear_sampler(
    device: &ProtocolObject<dyn MTLDevice>,
) -> Result<Retained<ProtocolObject<dyn MTLSamplerState>>, String> {
    let descriptor = MTLSamplerDescriptor::new();
    descriptor.setMinFilter(MTLSamplerMinMagFilter::Linear);
    descriptor.setMagFilter(MTLSamplerMinMagFilter::Linear);
    descriptor.setSAddressMode(MTLSamplerAddressMode::ClampToEdge);
    descriptor.setTAddressMode(MTLSamplerAddressMode::ClampToEdge);
    device
        .newSamplerStateWithDescriptor(&descriptor)
        .ok_or_else(|| "cannot make a Metal sampler".to_owned())
}

/// Returns a cache that shows `device` the planes of pixel buffers as
/// textures, or `None` if Core Video cannot make one.
fn texture_cache(
    device: &ProtocolObject<dyn MTLDevice>,
) -> Option<CFRetained<CVMetalTextureCache>> {
    let mut cache: *mut CVMetalTextureCache = std::ptr::null_mut();
    // SAFETY: no attributes are given, and `cache` is valid for writing.
    let status =
        unsafe { CVMetalTextureCache::create(None, None, device, None, NonNull::from(&mut cache)) };
    if status != kCVReturnSuccess {
        debug!(status, "Core Video cannot make a Metal texture cache");
        return None;
    }
    // SAFETY: the cache was created above with a +1 count.
    NonNull::new(cache).map(|cache| unsafe { CFRetained::from_raw(cache) })
}

/// A Metal texture.
type MetalTexture = Retained<ProtocolObject<dyn MTLTexture>>;

/// The textures showing Metal an NV12 pixel buffer's planes: Core Video's,
/// then Metal's of the luma and the chroma plane.
type PlaneTextures = ([CFRetained<CVMetalTexture>; 2], MetalTexture, MetalTexture);

/// Returns textures showing `buffer`'s two planes, without copying them.
fn plane_textures(
    cache: &CVMetalTextureCache,
    buffer: &CVPixelBuffer,
) -> Result<PlaneTextures, String> {
    let (luma_core, luma) = plane_texture(cache, buffer, LUMA_PLANE, MTLPixelFormat::R8Unorm)?;
    let (chroma_core, chroma) =
        plane_texture(cache, buffer, CHROMA_PLANE, MTLPixelFormat::RG8Unorm)?;
    Ok(([luma_core, chroma_core], luma, chroma))
}

/// Returns a texture of `format` showing plane `plane` of `buffer`.
fn plane_texture(
    cache: &CVMetalTextureCache,
    buffer: &CVPixelBuffer,
    plane: usize,
    format: MTLPixelFormat,
) -> Result<(CFRetained<CVMetalTexture>, MetalTexture), String> {
    let width = CVPixelBufferGetWidthOfPlane(buffer, plane);
    let height = CVPixelBufferGetHeightOfPlane(buffer, plane);
    let mut texture: *mut CVMetalTexture = std::ptr::null_mut();
    // SAFETY: the buffer is an NV12 pixel buffer, whose plane `plane` has
    // the size and a layout `format` matches; `texture` is valid for
    // writing.
    let status = unsafe {
        CVMetalTextureCache::create_texture_from_image(
            None,
            cache,
            buffer,
            None,
            format,
            width,
            height,
            plane,
            NonNull::from(&mut texture),
        )
    };
    if status != kCVReturnSuccess {
        return Err(format!(
            "CVMetalTextureCacheCreateTextureFromImage failed with {status}"
        ));
    }
    let texture = NonNull::new(texture).ok_or("Core Video made no texture")?;
    // SAFETY: the texture was created above with a +1 count.
    let texture = unsafe { CFRetained::from_raw(texture) };
    let metal =
        CVMetalTextureGetTexture(&texture).ok_or("Core Video's texture has no Metal one")?;
    Ok((texture, metal))
}

/// Returns a texture of `format` and `size` that shaders read and the
/// processor fills, or `None` if Metal cannot make one.
fn texture(
    device: &ProtocolObject<dyn MTLDevice>,
    format: MTLPixelFormat,
    size: (usize, usize),
) -> Option<SizedTexture> {
    if size.0 == 0 || size.1 == 0 {
        return None;
    }
    // SAFETY: the format is a colour one, the size is above zero, and
    // there are no mipmaps.
    let descriptor = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            format, size.0, size.1, false,
        )
    };
    descriptor.setUsage(MTLTextureUsage::ShaderRead);
    let texture = device.newTextureWithDescriptor(&descriptor)?;
    Some(SizedTexture { size, texture })
}

/// Copies `pixels`, rows of `bytes_per_pixel` bytes a pixel, into
/// `texture`, if they fill it.
fn fill(texture: &SizedTexture, pixels: &[u8], bytes_per_pixel: usize) {
    let (width, height) = texture.size;
    let row = width * bytes_per_pixel;
    if pixels.len() < row * height {
        return;
    }
    let Some(bytes) = NonNull::new(pixels.as_ptr().cast_mut().cast::<c_void>()) else {
        return;
    };
    let region = MTLRegion {
        origin: MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize {
            width,
            height,
            depth: 1,
        },
    };
    // SAFETY: `pixels` holds `height` rows of `row` bytes, which Metal only
    // reads during the call.
    unsafe {
        texture
            .texture
            .replaceRegion_mipmapLevel_withBytes_bytesPerRow(region, 0, bytes, row);
    }
}

/// Places the quad the encoder draws next at `rect` in a drawable of `size`
/// pixels.
fn quad(encoder: &ProtocolObject<dyn MTLRenderCommandEncoder>, rect: PixelRect, size: (u32, u32)) {
    let (width, height) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    // Device coordinates run from -1 to 1, up the drawable.
    let x = |pixels: f64| (pixels / width * 2.0 - 1.0) as f32;
    let y = |pixels: f64| (1.0 - pixels / height * 2.0) as f32;
    let edges: [f32; 4] = [
        x(rect.left),
        y(rect.top),
        x(rect.left + rect.width),
        y(rect.top + rect.height),
    ];
    let Some(bytes) = NonNull::new(edges.as_ptr().cast_mut().cast::<c_void>()) else {
        return;
    };
    // SAFETY: the edges are the `float4` the vertex shader reads from
    // buffer 0, copied during the call.
    unsafe {
        encoder.setVertexBytes_length_atIndex(bytes, size_of_val(&edges), 0);
    }
}

/// Returns a display link on `view` that asks its display for the video's
/// frame rates, and its target, or `None` before macOS 14, which has no such
/// links on views.
fn frame_rate_link(
    view: &NSView,
    main_thread: MainThreadMarker,
) -> Option<(Retained<CADisplayLink>, Retained<FrameRateDemand>)> {
    let selector = sel!(displayLinkWithTarget:selector:);
    if !view.respondsToSelector(selector) {
        return None;
    }
    let target = FrameRateDemand::new(main_thread);
    let target_object: &AnyObject = &target;
    // SAFETY: the target answers `step:`, which takes the link, and lives
    // as long as the link.
    let link = unsafe { view.displayLinkWithTarget_selector(target_object, sel!(step:)) };
    link.setPreferredFrameRateRange(FRAME_RATES);
    // SAFETY: the main run loop and its common modes are valid.
    unsafe {
        link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes);
    }
    Some((link, target))
}
