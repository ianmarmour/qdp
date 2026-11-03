//! A picture the VA-API decoder decoded, left in its surface: a window can
//! draw it from there, and anything else reads it into memory.

use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::debug;

use super::Display;
use super::ffi::{self, DrmPrimeSurfaceDescriptor, Image, ImageFormat, VaId, Zeroed};
use crate::{CodecError, Yuv420Frame};

/// A display a decoder decodes on, shared with its pictures.
pub(super) struct DecoderDisplay {
    /// The display.
    pub(super) display: Display,
    /// Whether `vaDeriveImage` failed on its surfaces, as it does where
    /// they are tiled, so that pictures are copied out with `vaGetImage`
    /// instead.
    derive_fails: AtomicBool,
}

impl DecoderDisplay {
    /// Shares `display` with a decoder's pictures.
    pub(super) fn new(display: Display) -> Self {
        Self {
            display,
            derive_fails: AtomicBool::new(false),
        }
    }
}

/// The surfaces of a decoder's session, destroyed once neither the session
/// nor any picture uses them.
pub(super) struct SurfacePool {
    /// The display they belong to.
    pub(super) device: Arc<DecoderDisplay>,
    /// The surfaces.
    pub(super) surfaces: Vec<VaId>,
}

impl Drop for SurfacePool {
    fn drop(&mut self) {
        let display = &self.device.display;
        // SAFETY: the surfaces were created on this display, and the session
        // destroyed its context, and no picture holds them, before this.
        unsafe {
            (display.va.destroy_surfaces)(
                display.handle,
                self.surfaces.as_mut_ptr(),
                self.surfaces.len() as i32,
            );
        }
    }
}

/// Which surface a picture is in: the same for every picture of one surface,
/// and different for every other surface that exists at the same time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceKey {
    /// The surfaces' pool.
    pool: usize,
    /// The surface in it.
    index: usize,
}

/// A picture's surface, shared as DMA-BUFs for a compositor to show: its
/// planes, in one layer.
#[derive(Debug)]
pub struct ExportedSurface {
    /// The surface's width, which may exceed the picture's.
    pub width: u32,
    /// The surface's height.
    pub height: u32,
    /// The DRM format of the layer: `NV12`.
    pub drm_format: u32,
    /// The DRM format modifier: how the planes are tiled.
    pub modifier: u64,
    /// The buffers the planes are in.
    pub objects: Vec<OwnedFd>,
    /// Each plane: its buffer, its offset in it, and the bytes from one of
    /// its rows to the next.
    pub planes: Vec<ExportedPlane>,
}

/// One plane of an [`ExportedSurface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportedPlane {
    /// Which of the surface's buffers the plane is in.
    pub object: usize,
    /// The plane's offset in it, in bytes.
    pub offset: u32,
    /// The bytes from one of its rows to the next.
    pub pitch: u32,
}

/// A decoded picture, in the surface the GPU decoded it into.
pub struct VaapiPicture {
    /// The surfaces, the picture's among them.
    pool: Arc<SurfacePool>,
    /// The picture's surface.
    index: usize,
    /// The width and height shown.
    visible: (usize, usize),
    /// Keeps the decoder from decoding into the surface while the picture
    /// is used.
    _hold: Arc<()>,
}

impl std::fmt::Debug for VaapiPicture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaapiPicture")
            .field("surface", &self.surface())
            .field("visible", &self.visible)
            .finish_non_exhaustive()
    }
}

impl VaapiPicture {
    /// Returns the picture of `pool`'s surface `index`, `visible` pixels of
    /// it shown, which `hold` keeps from being decoded into.
    pub(super) fn new(
        pool: Arc<SurfacePool>,
        index: usize,
        visible: (usize, usize),
        hold: Arc<()>,
    ) -> Self {
        Self {
            pool,
            index,
            visible,
            _hold: hold,
        }
    }

    /// Returns the width and height shown, in pixels.
    pub fn visible_size(&self) -> (usize, usize) {
        self.visible
    }

    /// Returns the picture cut to at most `width × height` pixels.
    pub fn cropped(mut self, width: usize, height: usize) -> Self {
        self.visible = (self.visible.0.min(width), self.visible.1.min(height));
        self
    }

    /// Returns which surface the picture is in, so that a compositor's
    /// buffer for the surface can be made once and used again.
    pub fn surface_key(&self) -> SurfaceKey {
        SurfaceKey {
            pool: Arc::as_ptr(&self.pool) as usize,
            index: self.index,
        }
    }

    /// Waits until the GPU has finished decoding the picture.
    pub fn wait(&self) -> Result<(), CodecError> {
        let display = self.display();
        // SAFETY: the surface was created on this display and is held.
        let status = unsafe { (display.va.sync_surface)(display.handle, self.surface()) };
        display.check(status, "vaSyncSurface").map_err(unavailable)
    }

    /// Shares the picture's surface as DMA-BUFs, its planes in one layer,
    /// for a compositor to show. The buffers stay valid as long as the
    /// surface, which lasts while any picture of it does.
    pub fn export(&self) -> Result<ExportedSurface, CodecError> {
        let display = self.display();
        let mut descriptor = DrmPrimeSurfaceDescriptor::zeroed();
        // SAFETY: the surface is held, and the descriptor is valid to write.
        let status = unsafe {
            (display.va.export_surface_handle)(
                display.handle,
                self.surface(),
                ffi::MEM_TYPE_DRM_PRIME_2,
                ffi::EXPORT_READ_ONLY | ffi::EXPORT_COMPOSED_LAYERS,
                (&raw mut descriptor).cast(),
            )
        };
        display
            .check(status, "vaExportSurfaceHandle")
            .map_err(unavailable)?;
        let object_count = (descriptor.num_objects as usize).min(descriptor.objects.len());
        // SAFETY: libva gives the caller each object's descriptor to own.
        let objects = descriptor.objects[..object_count]
            .iter()
            .map(|object| unsafe { OwnedFd::from_raw_fd(object.fd) })
            .collect::<Vec<_>>();
        if descriptor.num_layers != 1 {
            return Err(CodecError::HardwareUnavailable(format!(
                "the decoder's surface was exported in {} layers",
                descriptor.num_layers
            )));
        }
        let layer = &descriptor.layers[0];
        let plane_count = (layer.num_planes as usize).min(layer.offset.len());
        let planes = (0..plane_count)
            .map(|plane| ExportedPlane {
                object: layer.object_index[plane] as usize,
                offset: layer.offset[plane],
                pitch: layer.pitch[plane],
            })
            .collect::<Vec<_>>();
        if planes.iter().any(|plane| plane.object >= objects.len()) {
            return Err(CodecError::HardwareUnavailable(
                "the decoder's surface names a buffer it did not export".to_owned(),
            ));
        }
        let modifier =
            descriptor.objects[planes.first().map_or(0, |plane| plane.object)].drm_format_modifier;
        Ok(ExportedSurface {
            width: descriptor.width,
            height: descriptor.height,
            drm_format: layer.drm_format,
            modifier,
            objects,
            planes,
        })
    }

    /// Returns the picture's surface.
    fn surface(&self) -> VaId {
        self.pool.surfaces[self.index]
    }

    /// Returns the picture's display.
    fn display(&self) -> &Display {
        &self.pool.device.display
    }

    /// Copies the picture into memory, once the GPU has finished it.
    pub fn to_memory(&self) -> Result<Yuv420Frame, CodecError> {
        self.wait()?;
        let derive_fails = &self.pool.device.derive_fails;
        if !derive_fails.load(Ordering::Relaxed) {
            match self.derived_image() {
                Ok(image) => return self.read(image),
                Err(error) => {
                    debug!(%error, "decoded pictures are copied out of their surfaces");
                    derive_fails.store(true, Ordering::Relaxed);
                }
            }
        }
        let image = self.copied_image()?;
        self.read(image)
    }

    /// Returns an image of the surface's own memory, if the driver can give
    /// one in NV12.
    fn derived_image(&self) -> Result<Image, CodecError> {
        let display = self.display();
        let mut image = Image::zeroed();
        // SAFETY: the surface is held; `image` is valid to write.
        let status =
            unsafe { (display.va.derive_image)(display.handle, self.surface(), &mut image) };
        display
            .check(status, "vaDeriveImage")
            .map_err(unavailable)?;
        if image.format.fourcc != ffi::FOURCC_NV12 {
            self.destroy(&image);
            return Err(CodecError::HardwareUnavailable(
                "the decoder's surfaces are not NV12".to_owned(),
            ));
        }
        Ok(image)
    }

    /// Returns an NV12 image holding a copy of the visible picture.
    fn copied_image(&self) -> Result<Image, CodecError> {
        let display = self.display();
        let (width, height) = self.visible;
        // NV12 holds whole chroma samples, so the copy is of even size.
        let (width, height) = (width.next_multiple_of(2), height.next_multiple_of(2));
        let mut format = ImageFormat {
            fourcc: ffi::FOURCC_NV12,
            byte_order: ffi::LSB_FIRST,
            bits_per_pixel: ffi::NV12_BITS_PER_PIXEL,
            ..ImageFormat::zeroed()
        };
        let mut image = Image::zeroed();
        // SAFETY: `format` and `image` are valid to read and write.
        let status = unsafe {
            (display.va.create_image)(
                display.handle,
                &mut format,
                width as i32,
                height as i32,
                &mut image,
            )
        };
        display
            .check(status, "vaCreateImage")
            .map_err(unavailable)?;
        // SAFETY: the surface is held and the image is as large as the
        // region copied.
        let status = unsafe {
            (display.va.get_image)(
                display.handle,
                self.surface(),
                0,
                0,
                width as u32,
                height as u32,
                image.image_id,
            )
        };
        if let Err(error) = display.check(status, "vaGetImage") {
            self.destroy(&image);
            return Err(unavailable(error));
        }
        Ok(image)
    }

    /// Reads the visible picture out of `image`, and destroys it.
    fn read(&self, image: Image) -> Result<Yuv420Frame, CodecError> {
        let display = self.display();
        let mut data = std::ptr::null_mut();
        // SAFETY: the image's buffer belongs to this display.
        let status = unsafe { (display.va.map_buffer)(display.handle, image.buf, &mut data) };
        let frame = display
            .check(status, "vaMapBuffer")
            .map_err(unavailable)
            .and_then(|()| {
                // SAFETY: the mapped buffer holds `data_size` bytes until it
                // is unmapped below.
                let bytes = unsafe {
                    std::slice::from_raw_parts(data.cast::<u8>(), image.data_size as usize)
                };
                let plane = |index: usize| {
                    let offset = image.offsets[index] as usize;
                    (
                        bytes.get(offset..).unwrap_or_default(),
                        image.pitches[index] as usize,
                    )
                };
                let (width, height) = self.visible;
                Yuv420Frame::from_nv12(width, height, plane(0), plane(1)).ok_or_else(|| {
                    CodecError::HardwareUnavailable(
                        "the decoder's image is smaller than its picture".to_owned(),
                    )
                })
            });
        if !data.is_null() {
            // SAFETY: the buffer was mapped above.
            unsafe { (display.va.unmap_buffer)(display.handle, image.buf) };
        }
        self.destroy(&image);
        frame
    }

    /// Destroys `image`.
    fn destroy(&self, image: &Image) {
        let display = self.display();
        // SAFETY: the image was created on this display and is no longer
        // used.
        unsafe { (display.va.destroy_image)(display.handle, image.image_id) };
    }
}

/// Returns `error`, from a VA-API call, as the error that hands the stream to
/// software decoding.
pub(super) fn unavailable(error: CodecError) -> CodecError {
    match error {
        CodecError::Encoder(message) => CodecError::HardwareUnavailable(message),
        other => other,
    }
}
