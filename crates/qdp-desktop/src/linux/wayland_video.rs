//! Shows decoded pictures that the GPU holds in a Wayland window without
//! copying them: each of the decoder's surfaces becomes a buffer of the
//! compositor's, shown in a subsurface over the window that the compositor
//! scales and converts to RGB, as it does a video player's.
//!
//! The window's own surface stays what the window draws on the processor:
//! the background around the picture. A cursor the window draws itself goes
//! in a second subsurface, over the picture. Neither subsurface takes input,
//! so the window's surface gets it all.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::os::fd::{AsFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;
use std::time::Instant;

use qdp_codec::{SurfaceKey, VaapiPicture};
use tracing::{debug, info};
use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
};
use wayland_protocols::wp::color_representation::v1::client::wp_color_representation_manager_v1::{
    self, WpColorRepresentationManagerV1,
};
use wayland_protocols::wp::color_representation::v1::client::wp_color_representation_surface_v1::{
    Coefficients, Range, WpColorRepresentationSurfaceV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{
    self, ZwpLinuxBufferParamsV1,
};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::{
    self, ZwpLinuxDmabufV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::{Overlay, PixelRect, Scene};

/// The `zwp_linux_dmabuf_v1` version bound: the last whose `modifier`
/// events list what the compositor imports.
const DMABUF_VERSION: u32 = 3;

/// How many round trips the compositor may take to say whether it imported
/// a buffer.
const IMPORT_ROUND_TRIPS: usize = 10;

/// The buffers of surfaces no picture uses that are kept, beyond which the
/// oldest session's go.
const MAX_IMPORTED: usize = 32;

/// The bytes of an ARGB8888 pixel.
const PIXEL_BYTES: usize = 4;

/// A layer over the window: a surface placed as a subsurface of the
/// window's, with a viewport that scales its buffer.
struct Layer {
    /// The surface.
    surface: WlSurface,
    /// Its place over the window's surface.
    subsurface: WlSubsurface,
    /// Its scaling.
    viewport: WpViewport,
    /// Where it was placed last, in the window's logical coordinates.
    position: Option<(i32, i32)>,
}

impl Layer {
    /// Makes a layer over `parent` that takes no input.
    fn new(globals: &Globals, parent: &WlSurface, qh: &QueueHandle<State>) -> Self {
        let surface = globals.compositor.create_surface(qh, ());
        let subsurface = globals
            .subcompositor
            .get_subsurface(&surface, parent, qh, ());
        subsurface.set_desync();
        let nothing = globals.compositor.create_region(qh, ());
        surface.set_input_region(Some(&nothing));
        nothing.destroy();
        let viewport = globals.viewporter.get_viewport(&surface, qh, ());
        Self {
            surface,
            subsurface,
            viewport,
            position: None,
        }
    }

    /// Places the layer at `(x, y)`, which takes effect once the window's
    /// surface is committed.
    fn place(&mut self, x: i32, y: i32) {
        if self.position != Some((x, y)) {
            self.subsurface.set_position(x, y);
            self.position = Some((x, y));
        }
    }

    /// Shows nothing.
    fn clear(&self) {
        self.surface.attach(None, 0, 0);
        self.surface.commit();
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        self.viewport.destroy();
        self.subsurface.destroy();
        self.surface.destroy();
    }
}

/// The compositor's interfaces the presenter uses.
struct Globals {
    /// Makes surfaces and regions.
    compositor: WlCompositor,
    /// Makes subsurfaces.
    subcompositor: WlSubcompositor,
    /// Imports DMA-BUFs as buffers.
    dmabuf: ZwpLinuxDmabufV1,
    /// Scales surfaces.
    viewporter: WpViewporter,
    /// Makes buffers in shared memory, for the cursor.
    shm: WlShm,
    /// Says how a buffer's YCbCr converts to RGB, if the compositor lets
    /// clients say.
    color_representation: Option<WpColorRepresentationManagerV1>,
}

/// What the compositor said, gathered as its events are dispatched.
#[derive(Default)]
struct State {
    /// The DRM formats and modifiers the compositor imports.
    modifiers: Vec<(u32, u64)>,
    /// The YCbCr coefficients and ranges the compositor converts, as their
    /// protocol values.
    representations: Vec<(u32, u32)>,
    /// Whether the buffer being imported was, once the compositor said.
    imported: Option<Result<WlBuffer, ()>>,
    /// The buffers the compositor stopped reading.
    released: Vec<ObjectId>,
}

/// The cursor shown over the picture.
struct ShownOverlay {
    /// What was drawn into the buffer.
    image: Overlay,
    /// The buffer.
    buffer: WlBuffer,
}

/// Shows decoded pictures that the GPU holds in a Wayland window, as a
/// subsurface over it that the compositor draws.
pub struct WaylandVideo {
    /// The window's connection.
    connection: Connection,
    /// The presenter's own queue on it.
    queue: EventQueue<State>,
    /// What the compositor said.
    state: State,
    /// The compositor's interfaces.
    globals: Globals,
    /// The window's surface.
    parent: WlSurface,
    /// The picture's layer.
    video: Layer,
    /// The cursor's layer, over the picture's.
    overlay: Layer,
    /// The compositor's buffer of each decoder surface shown so far.
    imported: HashMap<SurfaceKey, WlBuffer>,
    /// The surface each of those buffers is of.
    buffer_keys: HashMap<ObjectId, SurfaceKey>,
    /// The pictures whose buffers the compositor may still read, which keep
    /// the decoder from decoding into them.
    held: HashMap<SurfaceKey, VaapiPicture>,
    /// The surface shown, if one is.
    attached: Option<SurfaceKey>,
    /// Where the picture was shown last, in the window's logical
    /// coordinates.
    picture_rect: Option<(i32, i32, i32, i32)>,
    /// The cursor shown, if one is.
    shown_overlay: Option<ShownOverlay>,
    /// Cursor buffers replaced, kept until the compositor releases them.
    retired: Vec<WlBuffer>,
}

impl std::fmt::Debug for WaylandVideo {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WaylandVideo")
            .field("attached", &self.attached)
            .finish_non_exhaustive()
    }
}

impl WaylandVideo {
    /// Shows pictures over the window whose surface is `surface`, on the
    /// connection `display`.
    ///
    /// # Safety
    ///
    /// `display` and `surface` must be a live `wl_display` and the window's
    /// `wl_surface` on it, which outlive the returned value.
    pub unsafe fn attach(
        display: NonNull<c_void>,
        surface: NonNull<c_void>,
    ) -> Result<Self, String> {
        // SAFETY: the caller guarantees the display is live and outlives
        // the connection.
        let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<State>(&connection)
            .map_err(|error| format!("cannot list the compositor's interfaces: {error}"))?;
        let qh = queue.handle();
        let bind_error =
            |name: &'static str| move |error| format!("the compositor has no {name}: {error}");
        let bound = Globals {
            compositor: globals
                .bind(&qh, 1..=6, ())
                .map_err(bind_error("wl_compositor"))?,
            subcompositor: globals
                .bind(&qh, 1..=1, ())
                .map_err(bind_error("wl_subcompositor"))?,
            dmabuf: globals
                .bind(&qh, DMABUF_VERSION..=DMABUF_VERSION, ())
                .map_err(bind_error("zwp_linux_dmabuf_v1 version 3"))?,
            viewporter: globals
                .bind(&qh, 1..=1, ())
                .map_err(bind_error("wp_viewporter"))?,
            shm: globals.bind(&qh, 1..=1, ()).map_err(bind_error("wl_shm"))?,
            color_representation: globals.bind(&qh, 1..=1, ()).ok(),
        };
        // SAFETY: the caller guarantees the surface is the window's, live on
        // this display.
        let parent_id =
            unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }
                .map_err(|error| format!("the window's surface is not usable: {error}"))?;
        let parent = WlSurface::from_id(&connection, parent_id)
            .map_err(|error| format!("the window's surface is not usable: {error}"))?;
        let video = Layer::new(&bound, &parent, &qh);
        let overlay = Layer::new(&bound, &parent, &qh);
        let mut state = State::default();
        queue
            .roundtrip(&mut state)
            .map_err(|error| format!("the compositor did not answer: {error}"))?;
        if let Some(manager) = &bound.color_representation {
            let representation = manager.get_surface(&video.surface, &qh, ());
            if state
                .representations
                .contains(&(Coefficients::Bt709 as u32, Range::Limited as u32))
            {
                // QDP's standard dynamic range streams are BT.709 with limited
                // range.
                representation.set_coefficients_and_range(Coefficients::Bt709, Range::Limited);
            }
        }
        info!(
            formats = state.modifiers.len(),
            color_representation = bound.color_representation.is_some(),
            "the compositor shows decoded pictures where the GPU holds them"
        );
        Ok(Self {
            connection,
            queue,
            state,
            globals: bound,
            parent,
            video,
            overlay,
            imported: HashMap::new(),
            buffer_keys: HashMap::new(),
            held: HashMap::new(),
            attached: None,
            picture_rect: None,
            shown_overlay: None,
            retired: Vec::new(),
        })
    }

    /// Shows `picture`, if there is a new one, where `scene` places it, with
    /// its overlay, in a window whose interface scale is `scale`, and commits
    /// the window's surface, as a window does each time it draws. Returns
    /// when a new picture was handed to the compositor, or why it could not
    /// be, so that it is drawn on the processor instead.
    pub fn show(
        &mut self,
        picture: Option<VaapiPicture>,
        scene: &Scene,
        scale: f64,
    ) -> Result<Option<Instant>, String> {
        self.collect_released();
        let shown = match picture {
            Some(picture) => {
                let key = picture.surface_key();
                picture.wait().map_err(|error| error.to_string())?;
                let buffer = self.buffer_for(&picture)?;
                self.video.surface.attach(Some(&buffer), 0, 0);
                self.video.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
                let (width, height) = picture.visible_size();
                self.video
                    .viewport
                    .set_source(0.0, 0.0, width as f64, height as f64);
                self.held.insert(key, picture);
                self.attached = Some(key);
                self.picture_rect = None;
                true
            }
            None => false,
        };
        if self.attached.is_some() {
            if let Some(rect) = scene.picture.map(|rect| logical(rect, scale)) {
                if self.picture_rect != Some(rect) || shown {
                    let (x, y, width, height) = rect;
                    self.video
                        .viewport
                        .set_destination(width.max(1), height.max(1));
                    self.video.place(x, y);
                    self.picture_rect = Some(rect);
                    self.video.surface.commit();
                }
            }
        }
        self.show_overlay(scene.overlay.as_ref(), scale)?;
        // The commit moves the layers, and applies the configuration the
        // window acknowledged, which only a commit does.
        self.parent.commit();
        self.connection
            .flush()
            .map_err(|error| format!("cannot reach the compositor: {error}"))?;
        Ok(shown.then(Instant::now))
    }

    /// Stops showing pictures, so that the window's own surface shows what
    /// the processor draws.
    pub fn hide(&mut self) {
        self.collect_released();
        if self.attached.take().is_some() {
            self.video.clear();
            self.picture_rect = None;
        }
        if let Some(shown) = self.shown_overlay.take() {
            self.overlay.clear();
            self.retired.push(shown.buffer);
        }
        if let Err(error) = self.connection.flush() {
            debug!(%error, "cannot reach the compositor");
        }
    }

    /// Lets go of the pictures and cursor buffers the compositor stopped
    /// reading.
    fn collect_released(&mut self) {
        if let Err(error) = self.queue.dispatch_pending(&mut self.state) {
            debug!(%error, "cannot read the compositor's events");
        }
        for id in self.state.released.drain(..) {
            if let Some(&key) = self.buffer_keys.get(&id) {
                if self.attached != Some(key) {
                    self.held.remove(&key);
                }
            } else if let Some(index) = self.retired.iter().position(|buffer| buffer.id() == id) {
                self.retired.swap_remove(index).destroy();
            }
        }
    }

    /// Returns the compositor's buffer of `picture`'s surface, importing it
    /// the first time.
    fn buffer_for(&mut self, picture: &VaapiPicture) -> Result<WlBuffer, String> {
        let key = picture.surface_key();
        if let Some(buffer) = self.imported.get(&key) {
            return Ok(buffer.clone());
        }
        if self.imported.len() >= MAX_IMPORTED {
            // A new session's surfaces replaced the old ones: their buffers
            // go, but for those still shown or read.
            let keep: Vec<SurfaceKey> = self.held.keys().copied().collect();
            self.imported.retain(|key, buffer| {
                let kept = keep.contains(key);
                if !kept {
                    buffer.destroy();
                }
                kept
            });
            self.buffer_keys.retain(|_, key| keep.contains(key));
        }
        let surface = picture.export().map_err(|error| error.to_string())?;
        if !self
            .state
            .modifiers
            .contains(&(surface.drm_format, surface.modifier))
        {
            return Err(format!(
                "the compositor does not show pictures of format {:#x} and modifier {:#x}",
                surface.drm_format, surface.modifier
            ));
        }
        let qh = self.queue.handle();
        let params = self.globals.dmabuf.create_params(&qh, ());
        for (index, plane) in surface.planes.iter().enumerate() {
            params.add(
                surface.objects[plane.object].as_fd(),
                index as u32,
                plane.offset,
                plane.pitch,
                (surface.modifier >> 32) as u32,
                surface.modifier as u32,
            );
        }
        self.state.imported = None;
        params.create(
            surface.width as i32,
            surface.height as i32,
            surface.drm_format,
            zwp_linux_buffer_params_v1::Flags::empty(),
        );
        for _ in 0..IMPORT_ROUND_TRIPS {
            if self.state.imported.is_some() {
                break;
            }
            self.queue
                .roundtrip(&mut self.state)
                .map_err(|error| format!("the compositor did not answer: {error}"))?;
        }
        params.destroy();
        let buffer = match self.state.imported.take() {
            Some(Ok(buffer)) => buffer,
            Some(Err(())) => {
                return Err("the compositor could not import a decoded picture".to_owned());
            }
            None => return Err("the compositor did not import a decoded picture".to_owned()),
        };
        debug!(
            width = surface.width,
            height = surface.height,
            modifier = surface.modifier,
            "a decoder surface is shown by the compositor"
        );
        self.buffer_keys.insert(buffer.id(), key);
        self.imported.insert(key, buffer.clone());
        Ok(buffer)
    }

    /// Shows `overlay` over the picture, or nothing.
    fn show_overlay(&mut self, overlay: Option<&Overlay>, scale: f64) -> Result<(), String> {
        let Some(overlay) = overlay else {
            if let Some(shown) = self.shown_overlay.take() {
                self.overlay.clear();
                self.retired.push(shown.buffer);
            }
            return Ok(());
        };
        let same_image = self.shown_overlay.as_ref().is_some_and(|shown| {
            shown.image.width == overlay.width
                && shown.image.height == overlay.height
                && std::sync::Arc::ptr_eq(&shown.image.rgba, &overlay.rgba)
        });
        if !same_image {
            let buffer = self.overlay_buffer(overlay)?;
            self.overlay.surface.attach(Some(&buffer), 0, 0);
            self.overlay.surface.damage_buffer(0, 0, i32::MAX, i32::MAX);
            self.overlay.viewport.set_destination(
                ((overlay.width as f64 / scale).round() as i32).max(1),
                ((overlay.height as f64 / scale).round() as i32).max(1),
            );
            self.overlay.surface.commit();
            if let Some(old) = self.shown_overlay.replace(ShownOverlay {
                image: overlay.clone(),
                buffer,
            }) {
                self.retired.push(old.buffer);
            }
        }
        self.overlay.place(
            (overlay.left / scale).round() as i32,
            (overlay.top / scale).round() as i32,
        );
        Ok(())
    }

    /// Returns a shared-memory buffer holding `overlay`'s image.
    fn overlay_buffer(&self, overlay: &Overlay) -> Result<WlBuffer, String> {
        let (width, height) = (overlay.width, overlay.height);
        let stride = width * PIXEL_BYTES;
        let mut pixels = Vec::with_capacity(stride * height);
        for rgba in overlay.rgba.chunks_exact(PIXEL_BYTES).take(width * height) {
            // ARGB8888 is premultiplied, little-endian: blue first.
            let alpha = u16::from(rgba[3]);
            let premultiply = |value: u8| ((u16::from(value) * alpha + 127) / 255) as u8;
            pixels.extend_from_slice(&[
                premultiply(rgba[2]),
                premultiply(rgba[1]),
                premultiply(rgba[0]),
                rgba[3],
            ]);
        }
        pixels.resize(stride * height, 0);
        let file = shared_memory(&pixels)?;
        let qh = self.queue.handle();
        let size = i32::try_from(pixels.len()).map_err(|_| "the cursor is too large")?;
        let pool = self.globals.shm.create_pool(file.as_fd(), size, &qh, ());
        let buffer = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            stride as i32,
            wl_shm::Format::Argb8888,
            &qh,
            (),
        );
        pool.destroy();
        Ok(buffer)
    }
}

impl Drop for WaylandVideo {
    fn drop(&mut self) {
        self.video.clear();
        self.overlay.clear();
        for buffer in self.imported.values() {
            buffer.destroy();
        }
        if let Some(shown) = self.shown_overlay.take() {
            shown.buffer.destroy();
        }
        for buffer in self.retired.drain(..) {
            buffer.destroy();
        }
        let _ = self.connection.flush();
    }
}

/// Returns `rect`, in a window's pixels, in its logical coordinates at
/// interface scale `scale`: left, top, width and height.
fn logical(rect: PixelRect, scale: f64) -> (i32, i32, i32, i32) {
    let scaled = |value: f64| (value / scale).round() as i32;
    (
        scaled(rect.left),
        scaled(rect.top),
        scaled(rect.width),
        scaled(rect.height),
    )
}

/// Returns a file of shared memory holding `bytes`, for the compositor to
/// map.
fn shared_memory(bytes: &[u8]) -> Result<File, String> {
    // SAFETY: the name is NUL-terminated; the descriptor returned, if any,
    // is new and owned here.
    let fd = unsafe { libc::memfd_create(c"qdp-cursor".as_ptr(), libc::MFD_CLOEXEC) };
    if fd < 0 {
        return Err(format!(
            "cannot make shared memory: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: `fd` is a new descriptor nothing else owns.
    let mut file = File::from(unsafe { OwnedFd::from_raw_fd(fd) });
    file.write_all(bytes)
        .map_err(|error| format!("cannot fill shared memory: {error}"))?;
    Ok(file)
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwpLinuxDmabufV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwp_linux_dmabuf_v1::Event::Modifier {
            format,
            modifier_hi,
            modifier_lo,
        } = event
        {
            let modifier = u64::from(modifier_hi) << 32 | u64::from(modifier_lo);
            state.modifiers.push((format, modifier));
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.imported = match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => Some(Ok(buffer)),
            zwp_linux_buffer_params_v1::Event::Failed => Some(Err(())),
            _ => return,
        };
    }

    event_created_child!(State, ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (WlBuffer, ()),
    ]);
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(
        state: &mut Self,
        buffer: &WlBuffer,
        event: wl_buffer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_buffer::Event::Release = event {
            state.released.push(buffer.id());
        }
    }
}

impl Dispatch<WpColorRepresentationManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &WpColorRepresentationManagerV1,
        event: wp_color_representation_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_color_representation_manager_v1::Event::SupportedCoefficientsAndRanges {
            coefficients,
            range,
        } = event
        {
            let coefficients = match coefficients {
                WEnum::Value(value) => value as u32,
                WEnum::Unknown(value) => value,
            };
            let range = match range {
                WEnum::Value(value) => value as u32,
                WEnum::Unknown(value) => value,
            };
            state.representations.push((coefficients, range));
        }
    }
}

delegate_noop!(State: WlCompositor);
delegate_noop!(State: WlSubcompositor);
delegate_noop!(State: WlSubsurface);
delegate_noop!(State: WlRegion);
delegate_noop!(State: WpViewporter);
delegate_noop!(State: WpViewport);
delegate_noop!(State: WlShmPool);
delegate_noop!(State: WpColorRepresentationSurfaceV1);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore WlShm);
