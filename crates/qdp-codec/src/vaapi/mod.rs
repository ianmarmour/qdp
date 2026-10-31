//! Hardware AV1 encoding through VA-API, the video interface of Linux GPU
//! drivers: Mesa's for AMD (RDNA 3 and later), and Intel's (Arc and later).
//!
//! The encoder sends the sequence and frame headers it wants as packed
//! headers, which drivers either copy into the stream (Intel) or read the
//! frame's parameters from (Mesa). Drivers may pad frames to their alignment,
//! as RDNA 3 pads 1080 rows to 1082; the frame headers always carry the
//! stream's size as the render size, so receivers show only that.
//!
//! VA-API is a C interface, so this module and its `ffi` module use `unsafe`:
//! each block states the contract it relies on, from libva's documentation.

#![allow(unsafe_code)]

mod ffi;

use std::ffi::{CStr, c_int, c_void};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use tracing::debug;

use self::ffi::{
    ConfigAttrib, Image, ImageFormat, LibVa, PackedHeaderParameters, PictureParameters,
    SequenceParameters, TileGroupParameters, VaDisplay, VaId, VaStatus, Zeroed,
};
use crate::headers::{
    ALL_REFERENCE_SLOTS, CDEF_DAMPING_MINUS_3, FrameHeader, FrameKind, INTERPOLATION_EIGHTTAP,
    ORDER_HINT_BITS, PREVIOUS_FRAME_SLOT, PRIMARY_REF_LAST, PRIMARY_REF_NONE, REFS_PER_FRAME,
    StreamFormat, TileLayout, TxMode, frame_header_obu, sequence_header_obu,
};
use crate::{
    CodecError, EncodedFrame, EncoderSettings, KeyFrameRequest, VideoEncoder, Yuv420Frame,
    level_for, tile_count, to_qdp_temporal_unit,
};

/// The directory of the GPUs' device nodes.
const DRI_DIRECTORY: &str = "/dev/dri";

/// The prefix of render nodes, which allow video encoding without display
/// privileges.
const RENDER_NODE_PREFIX: &str = "renderD";

/// The number of reconstructed surfaces: the previous frame, which the next
/// frame refers to, and the frame being encoded.
const RECONSTRUCTED_SURFACES: usize = 2;

/// The key frame period given to the driver. QDP asks for key frames itself,
/// so this only needs to be long.
const INTRA_PERIOD: u32 = 7200;

/// `superres_scale_denominator` without super-resolution (`SUPERRES_NUM`).
const SUPERRES_DENOMINATOR_NONE: u8 = 8;

/// The largest deblocking filter level.
const MAX_LOOP_FILTER_LEVEL: u8 = 63;

/// The quantizer steps per deblocking filter level, a simple rule that filters
/// more as quality drops.
const QINDEX_PER_FILTER_LEVEL: u8 = 8;

/// The `TileSizeBytes` used when the driver does not say.
const DEFAULT_TILE_SIZE_BYTES: u8 = 4;

/// The coded buffer holds this many times a raw frame's size.
const CODED_BUFFER_RAW_FRAMES: usize = 2;

/// The smallest coded buffer, in bytes.
const MIN_CODED_BUFFER_SIZE: usize = 1 << 20;

/// `transfer_characteristics` and the other colour fields are fixed, so the
/// only `VA_RT_FORMAT` the encoder uses is 8-bit 4:2:0.
const RT_FORMAT: u32 = ffi::RT_FORMAT_YUV420;

/// An initialized VA display on one GPU.
struct Display {
    /// libva's functions.
    va: &'static LibVa,
    /// The display.
    handle: VaDisplay,
    /// The GPU's render node, which must stay open while the display is used.
    _device: File,
    /// The node's path, for messages.
    path: PathBuf,
}

impl Display {
    /// Opens and initializes the display of the render node at `path`.
    fn open(va: &'static LibVa, path: &Path) -> Result<Self, CodecError> {
        let device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| encoder_error(format!("cannot open {}: {error}", path.display())))?;
        // SAFETY: the descriptor is open, and stays open as long as the
        // display, which `Drop` terminates first.
        let handle = unsafe { (va.get_display_drm)(device.as_raw_fd()) };
        if handle.is_null() {
            return Err(encoder_error(format!(
                "{} has no VA-API driver",
                path.display()
            )));
        }
        let (mut major, mut minor) = (0, 0);
        // SAFETY: `handle` is a display from `vaGetDisplayDRM`.
        let status = unsafe { (va.initialize)(handle, &mut major, &mut minor) };
        let display = Self {
            va,
            handle,
            _device: device,
            path: path.to_owned(),
        };
        display.check(status, "vaInitialize")?;
        debug!(path = %path.display(), major, minor, "VA-API initialized");
        Ok(display)
    }

    /// Returns an error for a failed call.
    fn check(&self, status: VaStatus, what: &str) -> Result<(), CodecError> {
        if status == ffi::VA_STATUS_SUCCESS {
            return Ok(());
        }
        // SAFETY: `vaErrorStr` returns a static NUL-terminated string.
        let message = unsafe { CStr::from_ptr((self.va.error_str)(status)) };
        Err(encoder_error(format!(
            "{what} failed on {}: {}",
            self.path.display(),
            message.to_string_lossy()
        )))
    }

    /// Returns the driver's description of itself.
    fn vendor(&self) -> String {
        // SAFETY: the display is initialized; the string is owned by libva.
        unsafe {
            let vendor = (self.va.query_vendor_string)(self.handle);
            if vendor.is_null() {
                String::new()
            } else {
                CStr::from_ptr(vendor).to_string_lossy().into_owned()
            }
        }
    }

    /// Returns the AV1 encoding entrypoint the driver offers, if any.
    fn av1_entrypoint(&self) -> Option<c_int> {
        // SAFETY: the display is initialized.
        let capacity = unsafe { (self.va.max_num_entrypoints)(self.handle) };
        let mut entrypoints = vec![0; usize::try_from(capacity).ok()?];
        let mut count = 0;
        // SAFETY: `entrypoints` holds `capacity` entries, as libva requires.
        let status = unsafe {
            (self.va.query_config_entrypoints)(
                self.handle,
                ffi::PROFILE_AV1_MAIN,
                entrypoints.as_mut_ptr(),
                &mut count,
            )
        };
        if status != ffi::VA_STATUS_SUCCESS {
            return None;
        }
        entrypoints.truncate(usize::try_from(count).unwrap_or(0));
        [ffi::ENTRYPOINT_ENCODE, ffi::ENTRYPOINT_ENCODE_LOW_POWER]
            .into_iter()
            .find(|entrypoint| entrypoints.contains(entrypoint))
    }

    /// Creates a parameter or data buffer holding `size` bytes at `data`.
    fn create_buffer(
        &self,
        context: VaId,
        buffer_type: c_int,
        data: *mut c_void,
        size: usize,
    ) -> Result<VaId, CodecError> {
        let mut id = ffi::VA_INVALID_ID;
        let size = u32::try_from(size).map_err(|_| encoder_error("buffer too large"))?;
        // SAFETY: `data` points to `size` readable bytes, which libva copies.
        let status = unsafe {
            (self.va.create_buffer)(self.handle, context, buffer_type, size, 1, data, &mut id)
        };
        self.check(status, "vaCreateBuffer")?;
        Ok(id)
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: the display is initialized and nothing uses it afterwards.
        unsafe { (self.va.terminate)(self.handle) };
    }
}

/// Buffers submitted with one picture, destroyed once it has been submitted.
struct PictureBuffers<'a> {
    /// The display the buffers belong to.
    display: &'a Display,
    /// The buffers.
    ids: Vec<VaId>,
}

impl PictureBuffers<'_> {
    /// Adds a buffer holding a copy of `value`.
    fn add<T: Copy>(
        &mut self,
        context: VaId,
        buffer_type: c_int,
        value: &T,
    ) -> Result<(), CodecError> {
        let mut copy = *value;
        let id = self.display.create_buffer(
            context,
            buffer_type,
            (&raw mut copy).cast(),
            size_of::<T>(),
        )?;
        self.ids.push(id);
        Ok(())
    }

    /// Adds a packed header: its parameters, then its bytes.
    fn add_packed_header(
        &mut self,
        context: VaId,
        header_type: u32,
        bytes: &[u8],
    ) -> Result<(), CodecError> {
        let parameters = PackedHeaderParameters {
            header_type,
            bit_length: u32::try_from(bytes.len() * 8)
                .map_err(|_| encoder_error("packed header too large"))?,
            ..PackedHeaderParameters::zeroed()
        };
        self.add(context, ffi::BUFFER_PACKED_HEADER_PARAMETERS, &parameters)?;
        let mut data = bytes.to_vec();
        let id = self.display.create_buffer(
            context,
            ffi::BUFFER_PACKED_HEADER_DATA,
            data.as_mut_ptr().cast(),
            data.len(),
        )?;
        self.ids.push(id);
        Ok(())
    }
}

impl Drop for PictureBuffers<'_> {
    fn drop(&mut self) {
        for &id in &self.ids {
            // SAFETY: each buffer was created on this display and is no longer
            // used once its picture has been submitted.
            unsafe { (self.display.va.destroy_buffer)(self.display.handle, id) };
        }
    }
}

/// What the driver can do, as its configuration attributes say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DriverSupport {
    /// The packed headers the driver accepts.
    packed_headers: u32,
    /// The transform mode the encoder uses.
    tx_mode: TxMode,
    /// `TileSizeBytes` in the driver's tile groups.
    tile_size_bytes: u8,
}

/// Returns an encoder error.
fn encoder_error(message: impl Into<String>) -> CodecError {
    CodecError::Encoder(message.into())
}

/// A hardware AV1 encoder using VA-API.
///
/// Each call to [`VideoEncoder::encode`] uploads the frame, encodes it and
/// waits for the result, which takes a few milliseconds on current GPUs.
pub struct VaapiEncoder {
    /// The GPU's display.
    display: Display,
    /// The encoding configuration.
    config: VaId,
    /// The encoding context.
    context: VaId,
    /// The surface frames are uploaded to.
    input: VaId,
    /// The surfaces the driver reconstructs frames into, used in turn.
    reconstructed: [VaId; RECONSTRUCTED_SURFACES],
    /// The buffer the driver writes the stream to.
    coded: VaId,
    /// The stream's size and level.
    format: StreamFormat,
    /// The tiles of every frame.
    tiles: TileLayout,
    /// What the driver supports.
    support: DriverSupport,
    /// `base_q_idx` of every frame.
    base_q_idx: u8,
    /// The sequence header, for key frames whose output lacks one.
    sequence_header: Vec<u8>,
    /// The index of the reconstructed surface holding the previous frame, if
    /// any.
    previous: Option<usize>,
    /// The number of frames encoded.
    frames: u64,
    /// The driver and device, for messages.
    description: String,
}

impl std::fmt::Debug for VaapiEncoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VaapiEncoder")
            .field("description", &self.description)
            .field("format", &self.format)
            .finish_non_exhaustive()
    }
}

impl VaapiEncoder {
    /// Opens the first GPU whose driver can encode AV1.
    pub fn new(settings: EncoderSettings) -> Result<Self, CodecError> {
        let va = ffi::libva().map_err(encoder_error)?;
        let mut nodes: Vec<PathBuf> = std::fs::read_dir(DRI_DIRECTORY)
            .map_err(|error| encoder_error(format!("cannot list {DRI_DIRECTORY}: {error}")))?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(RENDER_NODE_PREFIX)
            })
            .map(|entry| entry.path())
            .collect();
        nodes.sort();
        let mut reasons = Vec::new();
        for node in nodes {
            match Display::open(va, &node).and_then(|display| Self::with_display(display, settings))
            {
                Ok(encoder) => return Ok(encoder),
                Err(error) => reasons.push(error.to_string()),
            }
        }
        if reasons.is_empty() {
            reasons.push(format!("no GPU render nodes in {DRI_DIRECTORY}"));
        }
        Err(encoder_error(reasons.join("; ")))
    }

    /// Sets up encoding on an opened display.
    fn with_display(display: Display, settings: EncoderSettings) -> Result<Self, CodecError> {
        let entrypoint = display.av1_entrypoint().ok_or_else(|| {
            encoder_error(format!(
                "{} ({}) cannot encode AV1",
                display.path.display(),
                display.vendor()
            ))
        })?;
        let width = u32::try_from(settings.width).map_err(|_| CodecError::FrameSize)?;
        let height = u32::try_from(settings.height).map_err(|_| CodecError::FrameSize)?;
        let (support, mut attributes) = Self::configure(&display, entrypoint)?;

        let mut config = ffi::VA_INVALID_ID;
        // SAFETY: `attributes` is a valid array of its length.
        let status = unsafe {
            (display.va.create_config)(
                display.handle,
                ffi::PROFILE_AV1_MAIN,
                entrypoint,
                attributes.as_mut_ptr(),
                attributes.len() as c_int,
                &mut config,
            )
        };
        display.check(status, "vaCreateConfig")?;
        // From here on, `Drop` releases whatever has been created.
        let tiles = TileLayout::new(width, height, tile_count(settings.width, settings.height));
        let format = StreamFormat {
            width,
            height,
            level: level_for(
                width,
                height,
                settings.frame_rate,
                tiles.count_and_columns(),
            ),
        };
        let description = format!(
            "VA-API on {} ({})",
            display.path.display(),
            display.vendor()
        );
        let mut encoder = Self {
            display,
            config,
            context: ffi::VA_INVALID_ID,
            input: ffi::VA_INVALID_ID,
            reconstructed: [ffi::VA_INVALID_ID; RECONSTRUCTED_SURFACES],
            coded: ffi::VA_INVALID_ID,
            format,
            tiles,
            support,
            base_q_idx: settings.quantizer.max(1),
            sequence_header: sequence_header_obu(&format),
            previous: None,
            frames: 0,
            description,
        };
        encoder.create_resources(settings)?;
        debug!(
            encoder = encoder.description,
            level = format.level,
            tiles = tiles.count(),
            packed_headers = support.packed_headers,
            "hardware encoder ready"
        );
        Ok(encoder)
    }

    /// Reads the driver's capabilities and returns them with the attributes
    /// to create the configuration with.
    fn configure(
        display: &Display,
        entrypoint: c_int,
    ) -> Result<(DriverSupport, Vec<ConfigAttrib>), CodecError> {
        let mut queried = [
            ffi::ATTRIB_RT_FORMAT,
            ffi::ATTRIB_RATE_CONTROL,
            ffi::ATTRIB_PACKED_HEADERS,
            ffi::ATTRIB_AV1_EXT2,
        ]
        .map(|attrib_type| ConfigAttrib {
            attrib_type,
            value: 0,
        });
        // SAFETY: `queried` is a valid array of its length.
        let status = unsafe {
            (display.va.get_config_attributes)(
                display.handle,
                ffi::PROFILE_AV1_MAIN,
                entrypoint,
                queried.as_mut_ptr(),
                queried.len() as c_int,
            )
        };
        display.check(status, "vaGetConfigAttributes")?;
        let [rt_format, rate_control, packed_headers, ext2] = queried.map(|attrib| attrib.value);
        let supported = |value: u32| value != ffi::VA_ATTRIB_NOT_SUPPORTED;
        if !supported(rt_format) || rt_format & RT_FORMAT == 0 {
            return Err(encoder_error("the driver cannot encode 8-bit 4:2:0 AV1"));
        }
        if !supported(rate_control) || rate_control & ffi::RATE_CONTROL_CQP == 0 {
            return Err(encoder_error(
                "the driver has no constant quantizer AV1 encoding",
            ));
        }
        let packed_headers = if supported(packed_headers) {
            packed_headers & (ffi::PACKED_HEADER_SEQUENCE | ffi::PACKED_HEADER_PICTURE)
        } else {
            0
        };
        let (tx_mode, tile_size_bytes) = if supported(ext2) {
            let tx_modes = ext2 >> ffi::av1_ext2::TX_MODE_SUPPORT_SHIFT;
            let tx_mode = if tx_modes & ffi::av1_ext2::TX_MODE_SELECT_SUPPORTED != 0 {
                TxMode::Select
            } else {
                TxMode::Largest
            };
            let tile_size_bytes = (ext2 & ffi::av1_ext2::TILE_SIZE_BYTES_MINUS_1_MASK) as u8 + 1;
            (tx_mode, tile_size_bytes)
        } else {
            (TxMode::Select, DEFAULT_TILE_SIZE_BYTES)
        };
        let mut attributes = vec![
            ConfigAttrib {
                attrib_type: ffi::ATTRIB_RT_FORMAT,
                value: RT_FORMAT,
            },
            ConfigAttrib {
                attrib_type: ffi::ATTRIB_RATE_CONTROL,
                value: ffi::RATE_CONTROL_CQP,
            },
        ];
        if packed_headers != 0 {
            attributes.push(ConfigAttrib {
                attrib_type: ffi::ATTRIB_PACKED_HEADERS,
                value: packed_headers,
            });
        }
        let support = DriverSupport {
            packed_headers,
            tx_mode,
            tile_size_bytes,
        };
        Ok((support, attributes))
    }

    /// Creates the surfaces, the context and the coded buffer.
    fn create_resources(&mut self, settings: EncoderSettings) -> Result<(), CodecError> {
        let va = self.display.va;
        let mut surfaces = [ffi::VA_INVALID_ID; RECONSTRUCTED_SURFACES + 1];
        // SAFETY: `surfaces` has room for the number requested; no attributes
        // are passed.
        let status = unsafe {
            (va.create_surfaces)(
                self.display.handle,
                RT_FORMAT,
                self.format.width,
                self.format.height,
                surfaces.as_mut_ptr(),
                surfaces.len() as u32,
                std::ptr::null_mut(),
                0,
            )
        };
        self.display.check(status, "vaCreateSurfaces")?;
        self.input = surfaces[0];
        self.reconstructed.copy_from_slice(&surfaces[1..]);

        // SAFETY: the configuration and surfaces belong to this display.
        let status = unsafe {
            (va.create_context)(
                self.display.handle,
                self.config,
                self.format.width as c_int,
                self.format.height as c_int,
                ffi::PROGRESSIVE,
                surfaces.as_mut_ptr(),
                surfaces.len() as c_int,
                &mut self.context,
            )
        };
        self.display.check(status, "vaCreateContext")?;

        let raw_size = settings.width * settings.height * 3 / 2;
        let coded_size = (raw_size * CODED_BUFFER_RAW_FRAMES).max(MIN_CODED_BUFFER_SIZE);
        self.coded = self.display.create_buffer(
            self.context,
            ffi::BUFFER_CODED,
            std::ptr::null_mut(),
            coded_size,
        )?;
        Ok(())
    }

    /// Copies `frame` into the input surface, converting it to NV12.
    fn upload(&self, frame: &Yuv420Frame) -> Result<(), CodecError> {
        let va = self.display.va;
        let display = self.display.handle;
        let mut image = Image::zeroed();
        // SAFETY: the input surface belongs to this display.
        let derived =
            unsafe { (va.derive_image)(display, self.input, &mut image) } == ffi::VA_STATUS_SUCCESS;
        let derived = if derived && image.format.fourcc != ffi::FOURCC_NV12 {
            // SAFETY: the image was just derived.
            unsafe { (va.destroy_image)(display, image.image_id) };
            false
        } else {
            derived
        };
        if !derived {
            let mut format = ImageFormat {
                fourcc: ffi::FOURCC_NV12,
                byte_order: ffi::LSB_FIRST,
                bits_per_pixel: ffi::NV12_BITS_PER_PIXEL,
                ..ImageFormat::zeroed()
            };
            // SAFETY: `format` and `image` are valid for writing.
            let status = unsafe {
                (va.create_image)(
                    display,
                    &mut format,
                    self.format.width as c_int,
                    self.format.height as c_int,
                    &mut image,
                )
            };
            self.display.check(status, "vaCreateImage")?;
        }
        let result = self.fill_image(&image, frame).and_then(|()| {
            if derived {
                return Ok(());
            }
            let (width, height) = (self.format.width, self.format.height);
            // SAFETY: the image and surface belong to this display and have
            // the same size.
            let status = unsafe {
                (va.put_image)(
                    display,
                    self.input,
                    image.image_id,
                    0,
                    0,
                    width,
                    height,
                    0,
                    0,
                    width,
                    height,
                )
            };
            self.display.check(status, "vaPutImage")
        });
        // SAFETY: the image is no longer used.
        unsafe { (va.destroy_image)(display, image.image_id) };
        result
    }

    /// Writes `frame` into an NV12 image's buffer.
    fn fill_image(&self, image: &Image, frame: &Yuv420Frame) -> Result<(), CodecError> {
        let (width, height) = (frame.width(), frame.height());
        let chroma_width = frame.chroma_width();
        let pitch = |plane: usize| image.pitches[plane] as usize;
        let offset = |plane: usize| image.offsets[plane] as usize;
        let luma_end = offset(0) + pitch(0) * (height - 1) + width;
        let chroma_end = offset(1) + pitch(1) * (frame.chroma_height() - 1) + 2 * chroma_width;
        let size = image.data_size as usize;
        if pitch(0) < width || pitch(1) < 2 * chroma_width || luma_end > size || chroma_end > size {
            return Err(encoder_error("the driver's NV12 image is too small"));
        }
        let mut mapped: *mut c_void = std::ptr::null_mut();
        // SAFETY: the image's buffer belongs to this display.
        let status =
            unsafe { (self.display.va.map_buffer)(self.display.handle, image.buf, &mut mapped) };
        self.display.check(status, "vaMapBuffer")?;
        // SAFETY: a mapped image buffer holds `data_size` writable bytes until
        // it is unmapped below.
        let data = unsafe { std::slice::from_raw_parts_mut(mapped.cast::<u8>(), size) };
        for (row, source) in frame.y().chunks_exact(width).enumerate() {
            let start = offset(0) + row * pitch(0);
            data[start..start + width].copy_from_slice(source);
        }
        let chroma_rows = frame
            .u()
            .chunks_exact(chroma_width)
            .zip(frame.v().chunks_exact(chroma_width));
        for (row, (u, v)) in chroma_rows.enumerate() {
            let start = offset(1) + row * pitch(1);
            let destination = &mut data[start..start + 2 * chroma_width];
            for ((pair, &u), &v) in destination.chunks_exact_mut(2).zip(u).zip(v) {
                pair[0] = u;
                pair[1] = v;
            }
        }
        // SAFETY: the buffer was mapped above and `data` is no longer used.
        let status = unsafe { (self.display.va.unmap_buffer)(self.display.handle, image.buf) };
        self.display.check(status, "vaUnmapBuffer")
    }

    /// Returns the sequence parameters.
    fn sequence_parameters(&self) -> SequenceParameters {
        SequenceParameters {
            seq_level_idx: self.format.level,
            intra_period: INTRA_PERIOD,
            ip_period: 1,
            seq_fields: ffi::seq_fields::ENABLE_ORDER_HINT
                | ffi::seq_fields::ENABLE_CDEF
                | ffi::seq_fields::SUBSAMPLING_X
                | ffi::seq_fields::SUBSAMPLING_Y,
            order_hint_bits_minus_1: (ORDER_HINT_BITS - 1) as u8,
            ..SequenceParameters::zeroed()
        }
    }

    /// Returns the picture parameters of a frame encoded into the
    /// reconstructed surface `current`.
    fn picture_parameters(&self, header: &FrameHeader, current: usize) -> PictureParameters {
        let mut parameters = PictureParameters {
            frame_width_minus_1: (self.format.width - 1) as u16,
            frame_height_minus_1: (self.format.height - 1) as u16,
            reconstructed_frame: self.reconstructed[current],
            coded_buf: self.coded,
            reference_frames: [ffi::VA_INVALID_ID; 8],
            ref_frame_idx: [PREVIOUS_FRAME_SLOT; REFS_PER_FRAME],
            primary_ref_frame: PRIMARY_REF_NONE,
            order_hint: header.order_hint,
            refresh_frame_flags: ALL_REFERENCE_SLOTS,
            filter_level: [header.loop_filter_level; 2],
            filter_level_u: header.loop_filter_level,
            filter_level_v: header.loop_filter_level,
            superres_scale_denominator: SUPERRES_DENOMINATOR_NONE,
            interpolation_filter: INTERPOLATION_EIGHTTAP,
            base_qindex: header.base_q_idx,
            mode_control_flags: (match header.tx_mode {
                TxMode::Largest => 1,
                TxMode::Select => 2,
            }) << ffi::TX_MODE_SHIFT,
            tile_cols: header.tiles.cols as u8,
            tile_rows: header.tiles.rows as u8,
            cdef_damping_minus_3: CDEF_DAMPING_MINUS_3,
            tile_group_obu_hdr_info: ffi::TILE_GROUP_HAS_SIZE_FIELD,
            ..PictureParameters::zeroed()
        };
        let columns = header.tiles.cols as usize - 1;
        parameters.width_in_sbs_minus_1[..columns].fill((header.tiles.col_width_sb - 1) as u16);
        let rows = header.tiles.rows as usize - 1;
        parameters.height_in_sbs_minus_1[..rows].fill((header.tiles.row_height_sb - 1) as u16);
        match (header.kind, self.previous) {
            (FrameKind::Inter, Some(previous)) => {
                // Every slot holds the previous frame.
                parameters.reference_frames = [self.reconstructed[previous]; 8];
                parameters.primary_ref_frame = PRIMARY_REF_LAST;
                parameters.ref_frame_ctrl_l0 = ffi::SEARCH_LAST_FRAME_ONLY;
                parameters.picture_flags = 1 << ffi::picture_flags::FRAME_TYPE_SHIFT;
            }
            _ => {
                parameters.picture_flags = ffi::picture_flags::ERROR_RESILIENT_MODE;
            }
        }
        parameters
    }

    /// Submits one frame to the driver.
    fn submit(&self, header: &FrameHeader, current: usize) -> Result<(), CodecError> {
        let va = self.display.va;
        let display = self.display.handle;
        let mut buffers = PictureBuffers {
            display: &self.display,
            ids: Vec::new(),
        };
        let key = header.kind == FrameKind::Key;
        if key {
            buffers.add(
                self.context,
                ffi::BUFFER_SEQUENCE_PARAMETERS,
                &self.sequence_parameters(),
            )?;
        }
        buffers.add(
            self.context,
            ffi::BUFFER_PICTURE_PARAMETERS,
            &self.picture_parameters(header, current),
        )?;
        if key && self.support.packed_headers & ffi::PACKED_HEADER_SEQUENCE != 0 {
            buffers.add_packed_header(
                self.context,
                ffi::PACKED_HEADER_TYPE_SEQUENCE,
                &self.sequence_header,
            )?;
        }
        if self.support.packed_headers & ffi::PACKED_HEADER_PICTURE != 0 {
            let frame_header = frame_header_obu(&self.format, header);
            buffers.add_packed_header(
                self.context,
                ffi::PACKED_HEADER_TYPE_PICTURE,
                &frame_header,
            )?;
        }
        let tile_group = TileGroupParameters {
            tg_start: 0,
            tg_end: (self.tiles.count() - 1) as u8,
            ..TileGroupParameters::zeroed()
        };
        buffers.add(self.context, ffi::BUFFER_TILE_GROUP_PARAMETERS, &tile_group)?;

        // SAFETY: the context, surface and buffers belong to this display.
        unsafe {
            let status = (va.begin_picture)(display, self.context, self.input);
            self.display.check(status, "vaBeginPicture")?;
            let status = (va.render_picture)(
                display,
                self.context,
                buffers.ids.as_mut_ptr(),
                buffers.ids.len() as c_int,
            );
            if let Err(error) = self.display.check(status, "vaRenderPicture") {
                (va.end_picture)(display, self.context);
                return Err(error);
            }
            let status = (va.end_picture)(display, self.context);
            self.display.check(status, "vaEndPicture")?;
            let status = (va.sync_surface)(display, self.input);
            self.display.check(status, "vaSyncSurface")
        }
    }

    /// Returns the bytes the driver wrote to the coded buffer.
    fn read_coded(&self) -> Result<Vec<u8>, CodecError> {
        let va = self.display.va;
        let mut mapped: *mut c_void = std::ptr::null_mut();
        // SAFETY: the coded buffer belongs to this display.
        let status = unsafe { (va.map_buffer)(self.display.handle, self.coded, &mut mapped) };
        self.display.check(status, "vaMapBuffer")?;
        let mut bytes = Vec::new();
        let mut overflow = false;
        let mut segment = mapped.cast::<ffi::CodedBufferSegment>().cast_const();
        while !segment.is_null() {
            // SAFETY: a mapped coded buffer is a list of segments, each with
            // `size` readable bytes at `buf`, valid until unmapped.
            let current = unsafe { &*segment };
            overflow |= current.status & ffi::CODED_BUFFER_OVERFLOW != 0;
            if current.size > 0 && !current.buf.is_null() {
                // SAFETY: as above.
                let data = unsafe {
                    std::slice::from_raw_parts(current.buf.cast::<u8>(), current.size as usize)
                };
                bytes.extend_from_slice(data);
            }
            segment = current.next.cast::<ffi::CodedBufferSegment>().cast_const();
        }
        // SAFETY: the buffer was mapped above and is no longer read.
        let status = unsafe { (va.unmap_buffer)(self.display.handle, self.coded) };
        self.display.check(status, "vaUnmapBuffer")?;
        if overflow {
            return Err(encoder_error(
                "the encoded frame did not fit the coded buffer",
            ));
        }
        Ok(bytes)
    }
}

impl VideoEncoder for VaapiEncoder {
    fn encode(
        &mut self,
        frame: &Yuv420Frame,
        request: KeyFrameRequest,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        if frame.width() != self.format.width as usize
            || frame.height() != self.format.height as usize
        {
            return Err(CodecError::FrameSize);
        }
        let kind = match (request, self.previous) {
            (KeyFrameRequest::Auto, Some(_)) => FrameKind::Inter,
            _ => FrameKind::Key,
        };
        let current = self
            .previous
            .map_or(0, |previous| (previous + 1) % RECONSTRUCTED_SURFACES);
        let header = FrameHeader {
            kind,
            order_hint: (self.frames % (1 << ORDER_HINT_BITS)) as u8,
            base_q_idx: self.base_q_idx,
            loop_filter_level: (self.base_q_idx / QINDEX_PER_FILTER_LEVEL)
                .min(MAX_LOOP_FILTER_LEVEL),
            tx_mode: self.support.tx_mode,
            tiles: self.tiles,
            tile_size_bytes: self.support.tile_size_bytes,
        };
        self.upload(frame)?;
        // A failed frame leaves no usable reference, so the next is a key frame.
        self.previous = None;
        self.submit(&header, current)?;
        let bytes = self.read_coded()?;
        self.previous = Some(current);
        let is_key_frame = kind == FrameKind::Key;
        let temporal_unit = to_qdp_temporal_unit(&bytes, is_key_frame, &self.sequence_header)?;
        let input_index = self.frames;
        self.frames += 1;
        Ok(vec![EncodedFrame {
            input_index,
            temporal_unit,
            is_key_frame,
        }])
    }

    fn description(&self) -> &str {
        &self.description
    }
}

impl Drop for VaapiEncoder {
    fn drop(&mut self) {
        let va = self.display.va;
        let display = self.display.handle;
        // SAFETY: every object belongs to this display and is no longer used;
        // invalid identifiers, for objects never created, are ignored.
        unsafe {
            if self.coded != ffi::VA_INVALID_ID {
                (va.destroy_buffer)(display, self.coded);
            }
            if self.context != ffi::VA_INVALID_ID {
                (va.destroy_context)(display, self.context);
            }
            let mut surfaces = [self.input, self.reconstructed[0], self.reconstructed[1]];
            if self.input != ffi::VA_INVALID_ID {
                (va.destroy_surfaces)(display, surfaces.as_mut_ptr(), surfaces.len() as c_int);
            }
            (va.destroy_config)(display, self.config);
        }
    }
}
