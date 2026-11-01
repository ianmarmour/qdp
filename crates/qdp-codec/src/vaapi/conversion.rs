//! Encoding frames the GPU holds. Their DMA-BUFs are imported as VA
//! surfaces, and the GPU's video processor converts them into the
//! encoder's NV12 input, scaling them on the way, so that no pixel passes
//! through the processor.
//!
//! The processor is told that both sides are BT.709, so that it converts
//! only the colour matrix and range: from full-range RGB to the
//! limited-range YUV that QDP streams signal. Desktop pixels are taken as
//! BT.709 values, as the processor-side conversion takes them too.

use std::ffi::{c_int, c_void};
use std::os::fd::AsRawFd;

use tracing::info;

use super::ffi::{
    self, DrmPrimeLayer, DrmPrimeObject, DrmPrimeSurfaceDescriptor, ProcColorProperties,
    ProcPipelineParameters, SurfaceAttrib, VaId, Zeroed,
};
use super::{Display, PictureBuffers};
use crate::{BufferIdentity, CodecError, DmaBufFrame, DmaBufLayout, Rgb32Order};

/// The most imported buffers kept. Compositors share a screen through a
/// few buffers they reuse, so this keeps all of them, and bounds the
/// surfaces kept for buffers that were replaced.
const MAX_IMPORTS: usize = 16;

/// The colour of the output outside the converted frame, which covers all
/// of it: opaque black.
const BACKGROUND_COLOR: u32 = 0xff00_0000;

/// `DRM_FORMAT_XRGB8888`: blue, green, red and an unused byte in memory.
const DRM_FORMAT_XRGB8888: u32 = u32::from_le_bytes(*b"XR24");

/// `DRM_FORMAT_XBGR8888`: red, green, blue and an unused byte in memory.
const DRM_FORMAT_XBGR8888: u32 = u32::from_le_bytes(*b"XB24");

/// The VA and DRM formats of 32-bit pixels whose colour bytes come in
/// `order`, ignoring the fourth byte.
fn formats(order: Rgb32Order) -> (u32, u32) {
    match order {
        Rgb32Order::Bgr => (ffi::FOURCC_BGRX, DRM_FORMAT_XRGB8888),
        Rgb32Order::Rgb => (ffi::FOURCC_RGBX, DRM_FORMAT_XBGR8888),
    }
}

/// Returns the converted error of a call that failed on a frame the GPU
/// holds: the caller then sends frames in memory instead.
fn unsupported(error: CodecError) -> CodecError {
    match error {
        CodecError::Encoder(message) => CodecError::DmaBufUnsupported(message),
        other => other,
    }
}

/// A buffer imported as a surface.
#[derive(Debug, Clone, Copy)]
struct Import {
    /// The buffer, and where the frame is in it.
    key: (BufferIdentity, DmaBufLayout),
    /// The surface showing it.
    surface: VaId,
}

/// The video processor converting frames the GPU holds into the encoder's
/// input surface.
///
/// It holds VA objects of the encoder's display, which
/// [`GpuConversion::destroy`] releases before the display ends.
#[derive(Debug)]
pub(super) struct GpuConversion {
    /// The processing configuration.
    config: VaId,
    /// The processing context, rendering to the encoder's input surface.
    context: VaId,
    /// The output: the encoder's input surface.
    output: VaId,
    /// The buffers imported lately, the most recently used last.
    imports: Vec<Import>,
    /// Whether a frame has been converted yet, which is logged once.
    converted: bool,
}

impl GpuConversion {
    /// Sets up conversion into `output`, a surface of `width × height`
    /// pixels, if the driver can import DMA-BUFs and convert them. Returns
    /// why not otherwise.
    pub(super) fn new(
        display: &Display,
        output: VaId,
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        if !cfg!(target_pointer_width = "64") {
            // `ProcPipelineParameters` has the layout of 64-bit targets.
            return Err("video processing needs a 64-bit build".to_owned());
        }
        if !display.has_entrypoint(ffi::PROFILE_NONE, ffi::ENTRYPOINT_VIDEO_PROC) {
            return Err("the driver has no video processor".to_owned());
        }
        let va = display.va;
        let mut config = ffi::VA_INVALID_ID;
        // SAFETY: no attributes are passed.
        let status = unsafe {
            (va.create_config)(
                display.handle,
                ffi::PROFILE_NONE,
                ffi::ENTRYPOINT_VIDEO_PROC,
                std::ptr::null_mut(),
                0,
                &mut config,
            )
        };
        display
            .check(status, "vaCreateConfig (video processing)")
            .map_err(|error| error.to_string())?;
        let mut conversion = Self {
            config,
            context: ffi::VA_INVALID_ID,
            output,
            imports: Vec::new(),
            converted: false,
        };
        if let Err(reason) = conversion.check_import_support(display) {
            conversion.destroy(display);
            return Err(reason);
        }
        let mut targets = [output];
        // SAFETY: the configuration and surface belong to this display.
        let status = unsafe {
            (va.create_context)(
                display.handle,
                config,
                width as c_int,
                height as c_int,
                ffi::PROGRESSIVE,
                targets.as_mut_ptr(),
                targets.len() as c_int,
                &mut conversion.context,
            )
        };
        if let Err(error) = display.check(status, "vaCreateContext (video processing)") {
            conversion.destroy(display);
            return Err(error.to_string());
        }
        Ok(conversion)
    }

    /// Checks that surfaces of the processing configuration can be imported
    /// from DMA-BUFs of 32-bit RGB pixels.
    fn check_import_support(&self, display: &Display) -> Result<(), String> {
        let attributes = display
            .surface_attributes(self.config)
            .map_err(|error| error.to_string())?;
        let memory_types = attributes
            .iter()
            .find(|attribute| attribute.attrib_type == ffi::SURFACE_ATTRIB_MEMORY_TYPE)
            .and_then(SurfaceAttrib::integer_value)
            .unwrap_or(0) as u32;
        if memory_types & ffi::MEM_TYPE_DRM_PRIME_2 == 0 {
            return Err("the video processor cannot import DMA-BUFs".to_owned());
        }
        let pixel_formats: Vec<u32> = attributes
            .iter()
            .filter(|attribute| attribute.attrib_type == ffi::SURFACE_ATTRIB_PIXEL_FORMAT)
            .filter_map(SurfaceAttrib::integer_value)
            .map(|fourcc| fourcc as u32)
            .collect();
        // A driver that lists no formats is taken at its word that it
        // imports; one that lists formats must list the ones used.
        let missing = [ffi::FOURCC_BGRX, ffi::FOURCC_RGBX]
            .into_iter()
            .any(|fourcc| !pixel_formats.contains(&fourcc));
        if !pixel_formats.is_empty() && missing {
            return Err("the video processor cannot read 32-bit RGB".to_owned());
        }
        Ok(())
    }

    /// Converts `frame` into the output surface, scaling it to the
    /// output's size, and waits until the output holds it.
    pub(super) fn convert(
        &mut self,
        display: &Display,
        frame: &DmaBufFrame,
    ) -> Result<(), CodecError> {
        let surface = self.import(display, frame).map_err(unsupported)?;
        let parameters = ProcPipelineParameters {
            surface,
            surface_color_standard: ffi::COLOR_STANDARD_BT709,
            output_background_color: BACKGROUND_COLOR,
            output_color_standard: ffi::COLOR_STANDARD_BT709,
            input_color_properties: ProcColorProperties {
                color_range: ffi::SOURCE_RANGE_FULL,
                ..bt709()
            },
            output_color_properties: ProcColorProperties {
                color_range: ffi::SOURCE_RANGE_REDUCED,
                ..bt709()
            },
            ..ProcPipelineParameters::zeroed()
        };
        let mut buffers = PictureBuffers {
            display,
            ids: Vec::new(),
        };
        buffers
            .add(
                self.context,
                ffi::BUFFER_PROC_PIPELINE_PARAMETERS,
                &parameters,
            )
            .map_err(unsupported)?;
        let va = display.va;
        // SAFETY: the context and output belong to this display.
        let status = unsafe { (va.begin_picture)(display.handle, self.context, self.output) };
        display
            .check(status, "vaBeginPicture (video processing)")
            .map_err(unsupported)?;
        // SAFETY: the context and buffer belong to this display; the
        // parameters point to nothing.
        let status = unsafe {
            (va.render_picture)(
                display.handle,
                self.context,
                buffers.ids.as_mut_ptr(),
                buffers.ids.len() as c_int,
            )
        };
        let rendered = display.check(status, "vaRenderPicture (video processing)");
        // A picture that was begun is ended, even when rendering failed.
        // SAFETY: the context belongs to this display and has a picture.
        let status = unsafe { (va.end_picture)(display.handle, self.context) };
        rendered
            .and_then(|()| display.check(status, "vaEndPicture (video processing)"))
            .map_err(unsupported)?;
        // The encoder reads the output next; drivers that do not order the
        // two themselves would otherwise read it half written.
        // SAFETY: the output belongs to this display.
        let status = unsafe { (va.sync_surface)(display.handle, self.output) };
        display
            .check(status, "vaSyncSurface (video processing)")
            .map_err(unsupported)?;
        if !self.converted {
            self.converted = true;
            let layout = frame.layout();
            info!(
                width = layout.width,
                height = layout.height,
                modifier = format_args!("{:#x}", layout.modifier),
                "encoding frames straight from the GPU"
            );
        }
        Ok(())
    }

    /// Returns the surface showing `frame`'s buffer, importing it if it is
    /// not imported yet.
    fn import(&mut self, display: &Display, frame: &DmaBufFrame) -> Result<VaId, CodecError> {
        let key = (frame.identity(), frame.layout());
        if let Some(index) = self.imports.iter().position(|import| import.key == key) {
            let import = self.imports.remove(index);
            self.imports.push(import);
            return Ok(import.surface);
        }
        let surface = import_surface(display, frame)?;
        if self.imports.len() == MAX_IMPORTS {
            let mut oldest = self.imports.remove(0).surface;
            // SAFETY: the surface belongs to this display, and no work on it
            // is pending, since each conversion waits for its output.
            unsafe { (display.va.destroy_surfaces)(display.handle, &mut oldest, 1) };
        }
        self.imports.push(Import { key, surface });
        Ok(surface)
    }

    /// Releases the context, the configuration and the imported surfaces.
    pub(super) fn destroy(&mut self, display: &Display) {
        let va = display.va;
        // SAFETY: every object belongs to this display and is no longer
        // used; invalid identifiers, for objects never created, are skipped.
        unsafe {
            if self.context != ffi::VA_INVALID_ID {
                (va.destroy_context)(display.handle, self.context);
                self.context = ffi::VA_INVALID_ID;
            }
            for import in self.imports.drain(..) {
                let mut surface = import.surface;
                (va.destroy_surfaces)(display.handle, &mut surface, 1);
            }
            if self.config != ffi::VA_INVALID_ID {
                (va.destroy_config)(display.handle, self.config);
                self.config = ffi::VA_INVALID_ID;
            }
        }
    }
}

/// BT.709 primaries, transfer and matrix, in an unspecified range.
fn bt709() -> ProcColorProperties {
    ProcColorProperties {
        colour_primaries: ffi::H273_BT709,
        transfer_characteristics: ffi::H273_BT709,
        matrix_coefficients: ffi::H273_BT709,
        ..ProcColorProperties::zeroed()
    }
}

/// Returns the descriptor importing `frame`'s buffer.
fn descriptor(frame: &DmaBufFrame) -> DrmPrimeSurfaceDescriptor {
    let layout = frame.layout();
    let (fourcc, drm_format) = formats(layout.order);
    let mut descriptor = DrmPrimeSurfaceDescriptor::zeroed();
    descriptor.fourcc = fourcc;
    descriptor.width = layout.width as u32;
    descriptor.height = layout.height as u32;
    descriptor.num_objects = 1;
    descriptor.objects[0] = DrmPrimeObject {
        fd: frame.descriptor().as_raw_fd(),
        size: u32::try_from(frame.size()).unwrap_or(u32::MAX),
        drm_format_modifier: layout.modifier,
    };
    // The planes a modifier adds, such as a compressed layout's metadata,
    // follow the pixels' in the same buffer.
    let mut offset = [layout.offset, 0, 0, 0];
    let mut pitch = [layout.pitch, 0, 0, 0];
    for (index, plane) in frame.extra_planes().iter().enumerate() {
        offset[index + 1] = plane.offset;
        pitch[index + 1] = plane.pitch;
    }
    descriptor.num_layers = 1;
    descriptor.layers[0] = DrmPrimeLayer {
        drm_format,
        num_planes: 1 + frame.extra_planes().len() as u32,
        object_index: [0; 4],
        offset,
        pitch,
    };
    descriptor
}

/// Imports `frame`'s buffer as a surface.
fn import_surface(display: &Display, frame: &DmaBufFrame) -> Result<VaId, CodecError> {
    let width = u32::try_from(frame.width()).map_err(|_| CodecError::FrameSize)?;
    let height = u32::try_from(frame.height()).map_err(|_| CodecError::FrameSize)?;
    let mut descriptor = descriptor(frame);
    // The memory type comes first: drivers read the descriptor as the
    // memory type already given says.
    let mut attributes = [
        SurfaceAttrib::integer(
            ffi::SURFACE_ATTRIB_MEMORY_TYPE,
            ffi::MEM_TYPE_DRM_PRIME_2 as i32,
        ),
        SurfaceAttrib::pointer(
            ffi::SURFACE_ATTRIB_EXTERNAL_BUFFER,
            (&raw mut descriptor).cast::<c_void>(),
        ),
    ];
    let mut surface = ffi::VA_INVALID_ID;
    // SAFETY: the attributes are valid for the call, the descriptor they
    // point to describes one surface, and its descriptor stays open
    // throughout; drivers do not keep or close it.
    let status = unsafe {
        (display.va.create_surfaces)(
            display.handle,
            ffi::RT_FORMAT_RGB32,
            width,
            height,
            &mut surface,
            1,
            attributes.as_mut_ptr().cast(),
            attributes.len() as u32,
        )
    };
    display.check(status, "vaCreateSurfaces (DMA-BUF import)")?;
    Ok(surface)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DRM_FORMAT_MOD_LINEAR;
    use std::fs::File;
    use std::os::fd::OwnedFd;

    /// The DRM formats match `drm_fourcc.h`.
    #[test]
    fn drm_formats_match_the_kernel_headers() {
        assert_eq!(DRM_FORMAT_XRGB8888, 0x3432_5258);
        assert_eq!(DRM_FORMAT_XBGR8888, 0x3432_4258);
    }

    /// An import describes one object holding one plane, in the formats of
    /// the frame's byte order.
    #[test]
    fn describes_one_plane() {
        let file = File::open("/dev/null").unwrap();
        let layout = DmaBufLayout {
            width: 3840,
            height: 2160,
            order: Rgb32Order::Bgr,
            modifier: DRM_FORMAT_MOD_LINEAR,
            offset: 256,
            pitch: 15_360,
        };
        let frame = DmaBufFrame::new(OwnedFd::from(file), layout).unwrap();
        let described = descriptor(&frame);
        assert_eq!(described.fourcc, ffi::FOURCC_BGRX);
        assert_eq!((described.width, described.height), (3840, 2160));
        assert_eq!(described.num_objects, 1);
        assert_eq!(described.objects[0].fd, frame.descriptor().as_raw_fd());
        assert_eq!(
            described.objects[0].drm_format_modifier,
            DRM_FORMAT_MOD_LINEAR
        );
        // /dev/null has no size, so the frame's extent stands in.
        assert_eq!(described.objects[0].size, 256 + 15_360 * 2160);
        assert_eq!(described.num_layers, 1);
        assert_eq!(described.layers[0].drm_format, DRM_FORMAT_XRGB8888);
        assert_eq!(described.layers[0].num_planes, 1);
        assert_eq!(described.layers[0].offset[0], 256);
        assert_eq!(described.layers[0].pitch[0], 15_360);
        let rgb = formats(Rgb32Order::Rgb);
        assert_eq!(rgb, (ffi::FOURCC_RGBX, DRM_FORMAT_XBGR8888));
    }

    /// The planes a modifier adds, such as a display's compression
    /// metadata, follow the pixels' in the one object.
    #[test]
    fn describes_the_planes_a_modifier_adds() {
        let file = File::open("/dev/null").unwrap();
        let layout = DmaBufLayout {
            width: 2560,
            height: 1440,
            order: Rgb32Order::Bgr,
            modifier: 0x0200_0000_0000_1234,
            offset: 0,
            pitch: 10_240,
        };
        let metadata = [
            crate::DmaBufPlane {
                offset: 14_745_600,
                pitch: 2560,
            },
            crate::DmaBufPlane {
                offset: 14_811_136,
                pitch: 1024,
            },
        ];
        let frame = DmaBufFrame::new(OwnedFd::from(file), layout)
            .unwrap()
            .with_extra_planes(&metadata);
        let described = descriptor(&frame);
        assert_eq!(described.num_objects, 1);
        assert_eq!(described.layers[0].num_planes, 3);
        assert_eq!(described.layers[0].object_index, [0; 4]);
        assert_eq!(described.layers[0].offset, [0, 14_745_600, 14_811_136, 0]);
        assert_eq!(described.layers[0].pitch, [10_240, 2560, 1024, 0]);
    }
}
