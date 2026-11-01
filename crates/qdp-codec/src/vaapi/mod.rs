//! Hardware AV1 encoding through VA-API, the video interface of Linux GPU
//! drivers: Mesa's for AMD (RDNA 3 and later), and Intel's (Arc and later).
//!
//! The encoder sends the sequence and frame headers it wants as packed
//! headers, which drivers either copy into the stream (Intel) or read the
//! frame's parameters from (Mesa). Drivers may pad frames to their alignment,
//! as RDNA 3 pads 1080 rows to 1082; the frame headers always carry the
//! stream's size as the render size, so receivers show only that.
//!
//! Frames in memory are written into the encoder's input surface by the
//! processor. Frames the GPU holds, as DMA-BUFs, are converted into it by
//! the GPU's video processor instead, where the driver has one that can
//! import them (see the `conversion` module).
//!
//! VA-API is a C interface, so this module and its `ffi` module use `unsafe`:
//! each block states the contract it relies on, from libva's documentation.

#![allow(unsafe_code)]

mod conversion;
mod ffi;
mod references;

use std::ffi::{CStr, c_int, c_void};
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use self::conversion::GpuConversion;
use self::ffi::{
    ConfigAttrib, Image, ImageFormat, LibVa, PackedHeaderParameters, PictureParameters,
    SequenceParameters, SurfaceAttrib, TileGroupParameters, VaDisplay, VaId, VaStatus, Zeroed,
};
use self::references::{Plan, RECONSTRUCTED_SURFACES, ReferenceSlots, SlotUse};
use crate::frame::workers;
use crate::headers::{
    ALL_REFERENCE_SLOTS, CDEF_DAMPING_MINUS_3, FrameHeader, FrameKind, INTERPOLATION_EIGHTTAP,
    ORDER_HINT_BITS, PRIMARY_REF_LAST, PRIMARY_REF_NONE, REFS_PER_FRAME, StreamFormat, TileLayout,
    TxMode, frame_header_obu, sequence_header_obu,
};
use crate::{
    CodecError, DmaBufFrame, EncodedFrame, EncoderSettings, KeyFrameRequest, VideoEncoder,
    Yuv420Frame, level_for, tile_count, to_qdp_temporal_unit,
};

/// The directory of the GPUs' device nodes.
const DRI_DIRECTORY: &str = "/dev/dri";

/// The prefix of render nodes, which allow video encoding without display
/// privileges.
const RENDER_NODE_PREFIX: &str = "renderD";

/// The first Mesa release that keeps the reference frames VA-API names
/// rather than only the previous frame: 24.3.
const MESA_ANY_REFERENCE: (u32, u32) = (24, 3);

/// How Mesa's VA-API driver begins its vendor string, before its version.
const MESA_VENDOR_PREFIX: &str = "Mesa Gallium driver ";

/// What Intel's VA-API driver's vendor string begins with; it keeps the
/// reference frames VA-API names.
const INTEL_VENDOR_PREFIX: &str = "Intel iHD driver";

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

/// How often the encoder logs how long its stages take.
const TIMING_INTERVAL: Duration = Duration::from_secs(1);

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

    /// Returns the entrypoints the driver offers for `profile`, none if it
    /// does not support the profile.
    fn entrypoints(&self, profile: c_int) -> Vec<c_int> {
        // SAFETY: the display is initialized.
        let capacity = unsafe { (self.va.max_num_entrypoints)(self.handle) };
        let Ok(capacity) = usize::try_from(capacity) else {
            return Vec::new();
        };
        let mut entrypoints = vec![0; capacity];
        let mut count = 0;
        // SAFETY: `entrypoints` holds `capacity` entries, as libva requires.
        let status = unsafe {
            (self.va.query_config_entrypoints)(
                self.handle,
                profile,
                entrypoints.as_mut_ptr(),
                &mut count,
            )
        };
        if status != ffi::VA_STATUS_SUCCESS {
            return Vec::new();
        }
        entrypoints.truncate(usize::try_from(count).unwrap_or(0).min(capacity));
        entrypoints
    }

    /// Returns whether the driver offers `entrypoint` for `profile`.
    fn has_entrypoint(&self, profile: c_int, entrypoint: c_int) -> bool {
        self.entrypoints(profile).contains(&entrypoint)
    }

    /// Returns the AV1 encoding entrypoint the driver offers, if any.
    fn av1_entrypoint(&self) -> Option<c_int> {
        let entrypoints = self.entrypoints(ffi::PROFILE_AV1_MAIN);
        [ffi::ENTRYPOINT_ENCODE, ffi::ENTRYPOINT_ENCODE_LOW_POWER]
            .into_iter()
            .find(|entrypoint| entrypoints.contains(entrypoint))
    }

    /// Returns the attributes surfaces of `config` can have.
    fn surface_attributes(&self, config: VaId) -> Result<Vec<SurfaceAttrib>, CodecError> {
        let mut count = 0;
        // SAFETY: with no list, libva only writes how many attributes there
        // are.
        let status = unsafe {
            (self.va.query_surface_attributes)(
                self.handle,
                config,
                std::ptr::null_mut(),
                &mut count,
            )
        };
        self.check(status, "vaQuerySurfaceAttributes")?;
        let empty = SurfaceAttrib::integer(0, 0);
        let mut attributes = vec![empty; count as usize];
        // SAFETY: `attributes` holds `count` entries.
        let status = unsafe {
            (self.va.query_surface_attributes)(
                self.handle,
                config,
                attributes.as_mut_ptr(),
                &mut count,
            )
        };
        self.check(status, "vaQuerySurfaceAttributes")?;
        attributes.truncate(count as usize);
        Ok(attributes)
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
    /// How inter frames may use the reference slots.
    slot_use: SlotUse,
}

/// Returns how a driver whose vendor string is `vendor` lets inter frames
/// use the reference slots: any slot, for Mesa 24.3 or later and Intel's
/// driver, which keep the reference frames VA-API names, or only the
/// previous frame's, for others.
fn slot_use(vendor: &str) -> SlotUse {
    if vendor.starts_with(INTEL_VENDOR_PREFIX) {
        return SlotUse::Rotating;
    }
    let mesa_version = vendor.strip_prefix(MESA_VENDOR_PREFIX).and_then(|rest| {
        let mut numbers = rest
            .split(|character: char| !character.is_ascii_digit())
            .map(str::parse::<u32>);
        Some((numbers.next()?.ok()?, numbers.next()?.ok()?))
    });
    match mesa_version {
        Some(version) if version >= MESA_ANY_REFERENCE => SlotUse::Rotating,
        _ => SlotUse::PreviousOnly,
    }
}

/// Returns an encoder error.
fn encoder_error(message: impl Into<String>) -> CodecError {
    CodecError::Encoder(message.into())
}

/// A hardware AV1 encoder using VA-API.
///
/// Each call to [`VideoEncoder::encode`] uploads the frame, encodes it and
/// waits for the result, which takes a few milliseconds on current GPUs.
/// [`VideoEncoder::encode_dma_buf`] converts a frame the GPU holds on the
/// GPU instead of uploading it.
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
    /// The frames in the reference slots, and their reconstructed surfaces.
    slots: ReferenceSlots,
    /// The number of frames encoded.
    frames: u64,
    /// The driver and device, for messages.
    description: String,
    /// The image frames are written into and then copied to the input
    /// surface, for drivers whose surfaces cannot be written directly. It is
    /// made once and kept, since making one for each frame allocates and
    /// faults in a frame's memory every time.
    staging: Option<Image>,
    /// How long the encoder's stages took lately.
    timing: StageTiming,
    /// Converts frames the GPU holds into the input surface, or why the
    /// driver cannot.
    gpu: Result<GpuConversion, String>,
}

/// How long an encoder's stages took over an interval, logged when it ends.
#[derive(Debug, Clone, Copy)]
struct StageTiming {
    /// When the interval started.
    started: Instant,
    /// The frames encoded in it.
    frames: u32,
    /// Writing frames into the input surface: uploading frames in memory,
    /// or converting frames the GPU holds.
    upload: Duration,
    /// Encoding them, until the GPU finished.
    encode: Duration,
    /// Reading the encoded frames back.
    read: Duration,
}

impl StageTiming {
    /// Starts an interval at `now`.
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            frames: 0,
            upload: Duration::ZERO,
            encode: Duration::ZERO,
            read: Duration::ZERO,
        }
    }

    /// Counts a frame whose stages took `upload`, `encode` and `read`, and
    /// logs the interval's averages once it has ended at `now`.
    fn record(&mut self, upload: Duration, encode: Duration, read: Duration, now: Instant) {
        self.frames += 1;
        self.upload += upload;
        self.encode += encode;
        self.read += read;
        if now.saturating_duration_since(self.started) < TIMING_INTERVAL {
            return;
        }
        let milliseconds =
            |total: Duration| format!("{:.2}", (total / self.frames).as_secs_f64() * 1000.0);
        debug!(
            frames = self.frames,
            upload_ms = milliseconds(self.upload),
            encode_ms = milliseconds(self.encode),
            read_ms = milliseconds(self.read),
            "VA-API encoder stages"
        );
        *self = Self::new(now);
    }
}

/// Where an NV12 image's planes lie in its buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Nv12Layout {
    /// Where the luma plane starts.
    luma_offset: usize,
    /// The bytes from one luma row to the next.
    luma_pitch: usize,
    /// Where the interleaved chroma plane starts.
    chroma_offset: usize,
    /// The bytes from one chroma row to the next.
    chroma_pitch: usize,
}

impl Nv12Layout {
    /// Returns the layout of `image`.
    fn of(image: &Image) -> Self {
        Self {
            luma_offset: image.offsets[0] as usize,
            luma_pitch: image.pitches[0] as usize,
            chroma_offset: image.offsets[1] as usize,
            chroma_pitch: image.pitches[1] as usize,
        }
    }
}

/// Writes `frame` into `data`, an NV12 buffer laid out as `layout`, its
/// chroma interleaved, in bands of rows on several threads.
fn write_nv12(frame: &Yuv420Frame, data: &mut [u8], layout: Nv12Layout) -> Result<(), CodecError> {
    let (width, height) = (frame.width(), frame.height());
    let (chroma_width, chroma_height) = (frame.chroma_width(), frame.chroma_height());
    let luma_end = layout.luma_offset + layout.luma_pitch * (height - 1) + width;
    let chroma_end =
        layout.chroma_offset + layout.chroma_pitch * (chroma_height - 1) + 2 * chroma_width;
    if layout.luma_pitch < width
        || layout.chroma_pitch < 2 * chroma_width
        || luma_end > layout.chroma_offset
        || chroma_end > data.len()
    {
        return Err(encoder_error("the driver's NV12 image is too small"));
    }
    let (luma, chroma) = data.split_at_mut(layout.chroma_offset);
    let luma = &mut luma[layout.luma_offset..];
    let bands = workers(width * height);
    let luma_rows = height.div_ceil(bands);
    let chroma_rows = chroma_height.div_ceil(bands);
    std::thread::scope(|scope| {
        for (band, destination) in luma.chunks_mut(luma_rows * layout.luma_pitch).enumerate() {
            let first = band * luma_rows;
            let rows = luma_rows.min(height.saturating_sub(first));
            scope.spawn(move || {
                for row in 0..rows {
                    let source = &frame.y()[(first + row) * width..][..width];
                    destination[row * layout.luma_pitch..][..width].copy_from_slice(source);
                }
            });
        }
        for (band, destination) in chroma
            .chunks_mut(chroma_rows * layout.chroma_pitch)
            .enumerate()
        {
            let first = band * chroma_rows;
            let rows = chroma_rows.min(chroma_height.saturating_sub(first));
            scope.spawn(move || {
                // Each row is interleaved in memory first and then copied
                // whole, since the mapped buffer may be uncached memory that
                // is slow to write a byte at a time.
                let mut interleaved = vec![0_u8; 2 * chroma_width];
                for row in 0..rows {
                    let start = (first + row) * chroma_width;
                    let u = &frame.u()[start..][..chroma_width];
                    let v = &frame.v()[start..][..chroma_width];
                    for ((pair, &u), &v) in interleaved.chunks_exact_mut(2).zip(u).zip(v) {
                        pair[0] = u;
                        pair[1] = v;
                    }
                    destination[row * layout.chroma_pitch..][..2 * chroma_width]
                        .copy_from_slice(&interleaved);
                }
            });
        }
    });
    Ok(())
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
            slots: ReferenceSlots::new(support.slot_use),
            frames: 0,
            description,
            staging: None,
            timing: StageTiming::new(Instant::now()),
            gpu: Err("not set up".to_owned()),
        };
        encoder.create_resources(settings)?;
        encoder.gpu = GpuConversion::new(&encoder.display, encoder.input, width, height);
        debug!(
            encoder = encoder.description,
            level = format.level,
            tiles = tiles.count(),
            packed_headers = support.packed_headers,
            slots = ?support.slot_use,
            gpu_frames = ?encoder.gpu.as_ref().err(),
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
        let slot_use = slot_use(&display.vendor());
        let support = DriverSupport {
            packed_headers,
            tx_mode,
            tile_size_bytes,
            slot_use,
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
        // The context renders to the input and every reconstructed surface.

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

    /// Copies `frame` into the input surface, converting it to NV12:
    /// directly into the surface's memory where the driver lets it be
    /// derived, or else through the staging image.
    fn upload(&mut self, frame: &Yuv420Frame) -> Result<(), CodecError> {
        let va = self.display.va;
        let display = self.display.handle;
        if self.staging.is_none() {
            let mut image = Image::zeroed();
            // SAFETY: the input surface belongs to this display.
            let derived = unsafe { (va.derive_image)(display, self.input, &mut image) }
                == ffi::VA_STATUS_SUCCESS;
            if derived && image.format.fourcc == ffi::FOURCC_NV12 {
                // The surface's own memory: written each frame and released
                // before the surface is encoded.
                let result = self.fill_image(&image, frame);
                // SAFETY: the image was just derived and is no longer used.
                unsafe { (va.destroy_image)(display, image.image_id) };
                return result;
            }
            if derived {
                // SAFETY: the image was just derived.
                unsafe { (va.destroy_image)(display, image.image_id) };
            }
            self.staging = Some(self.create_staging()?);
        }
        let Some(staging) = self.staging else {
            return Err(encoder_error("no staging image"));
        };
        self.fill_image(&staging, frame)?;
        let (width, height) = (self.format.width, self.format.height);
        // SAFETY: the image and surface belong to this display and have the
        // same size. The copy has finished by the time the next frame writes
        // the image, since each frame waits for its surface.
        let status = unsafe {
            (va.put_image)(
                display,
                self.input,
                staging.image_id,
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
    }

    /// Makes the NV12 image frames are staged in.
    fn create_staging(&self) -> Result<Image, CodecError> {
        let mut image = Image::zeroed();
        let mut format = ImageFormat {
            fourcc: ffi::FOURCC_NV12,
            byte_order: ffi::LSB_FIRST,
            bits_per_pixel: ffi::NV12_BITS_PER_PIXEL,
            ..ImageFormat::zeroed()
        };
        // SAFETY: `format` and `image` are valid for writing.
        let status = unsafe {
            (self.display.va.create_image)(
                self.display.handle,
                &mut format,
                self.format.width as c_int,
                self.format.height as c_int,
                &mut image,
            )
        };
        self.display.check(status, "vaCreateImage")?;
        Ok(image)
    }

    /// Writes `frame` into an NV12 image's buffer.
    fn fill_image(&self, image: &Image, frame: &Yuv420Frame) -> Result<(), CodecError> {
        let mut mapped: *mut c_void = std::ptr::null_mut();
        // SAFETY: the image's buffer belongs to this display.
        let status =
            unsafe { (self.display.va.map_buffer)(self.display.handle, image.buf, &mut mapped) };
        self.display.check(status, "vaMapBuffer")?;
        // SAFETY: a mapped image buffer holds `data_size` writable bytes until
        // it is unmapped below.
        let data = unsafe {
            std::slice::from_raw_parts_mut(mapped.cast::<u8>(), image.data_size as usize)
        };
        let written = write_nv12(frame, data, Nv12Layout::of(image));
        // SAFETY: the buffer was mapped above and `data` is no longer used.
        let status = unsafe { (self.display.va.unmap_buffer)(self.display.handle, image.buf) };
        written?;
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

    /// Returns the picture parameters of a frame encoded as `plan` says.
    fn picture_parameters(&self, header: &FrameHeader, plan: Plan) -> PictureParameters {
        let mut parameters = PictureParameters {
            frame_width_minus_1: (self.format.width - 1) as u16,
            frame_height_minus_1: (self.format.height - 1) as u16,
            reconstructed_frame: self.reconstructed[plan.surface()],
            coded_buf: self.coded,
            reference_frames: [ffi::VA_INVALID_ID; 8],
            ref_frame_idx: [0; REFS_PER_FRAME],
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
        match header.kind {
            FrameKind::Inter {
                reference_slot,
                refresh_frame_flags,
            } => {
                // Every slot's frame is named, so that the driver keeps each
                // for later frames to refer to; every reference names the
                // one the frame is predicted from.
                for (named, surface) in parameters
                    .reference_frames
                    .iter_mut()
                    .zip(self.slots.surfaces())
                {
                    if let Some(surface) = surface {
                        *named = self.reconstructed[surface];
                    }
                }
                parameters.ref_frame_idx = [reference_slot; REFS_PER_FRAME];
                parameters.refresh_frame_flags = refresh_frame_flags;
                parameters.primary_ref_frame = PRIMARY_REF_LAST;
                parameters.ref_frame_ctrl_l0 = ffi::SEARCH_LAST_FRAME_ONLY;
                parameters.picture_flags = 1 << ffi::picture_flags::FRAME_TYPE_SHIFT;
            }
            FrameKind::Key => {
                parameters.picture_flags = ffi::picture_flags::ERROR_RESILIENT_MODE;
            }
        }
        parameters
    }

    /// Submits one frame to the driver, encoded as `plan` says.
    fn submit(&self, header: &FrameHeader, plan: Plan) -> Result<(), CodecError> {
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
            &self.picture_parameters(header, plan),
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
        let started = Instant::now();
        self.upload(frame)?;
        self.encode_input(request, started)
    }

    fn takes_dma_bufs(&self) -> bool {
        self.gpu.is_ok()
    }

    fn encode_dma_buf(
        &mut self,
        frame: &DmaBufFrame,
        request: KeyFrameRequest,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        let started = Instant::now();
        match &mut self.gpu {
            Ok(conversion) => conversion.convert(&self.display, frame)?,
            Err(reason) => return Err(CodecError::DmaBufUnsupported(reason.clone())),
        }
        self.encode_input(request, started)
    }

    fn set_quantizer(&mut self, quantizer: u8) {
        // A `base_q_idx` of 0 would make frames lossless.
        self.base_q_idx = quantizer.max(1);
    }

    fn description(&self) -> &str {
        &self.description
    }
}

impl VaapiEncoder {
    /// Encodes the frame in the input surface, which was being written since
    /// `started`.
    fn encode_input(
        &mut self,
        request: KeyFrameRequest,
        started: Instant,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        let plan = self.slots.plan(request);
        let (kind, recovered_from) = match plan {
            Plan::Key { .. } => (FrameKind::Key, None),
            Plan::Inter {
                reference_slot,
                refresh_frame_flags,
                recovered_from,
                ..
            } => (
                FrameKind::Inter {
                    reference_slot,
                    refresh_frame_flags,
                },
                recovered_from,
            ),
        };
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
        let uploaded = Instant::now();
        // The picture parameters name the slots' frames, so the slots are
        // forgotten only once a frame fails: it leaves no usable reference,
        // and the next is a key frame.
        if let Err(error) = self.submit(&header, plan) {
            self.slots.clear();
            // A driver that refuses frames referring to any slot, whatever
            // its version says, gets frames that refer only to the previous
            // one, starting again from a key frame.
            if self.support.slot_use == SlotUse::Rotating && matches!(plan, Plan::Inter { .. }) {
                warn!(
                    %error,
                    "the driver refuses frames that refer to older ones; losses are repaired with key frames"
                );
                self.support.slot_use = SlotUse::PreviousOnly;
                self.slots = ReferenceSlots::new(SlotUse::PreviousOnly);
                return self.encode_input(KeyFrameRequest::Force, started);
            }
            return Err(error);
        }
        let encoded = Instant::now();
        let bytes = self.read_coded().inspect_err(|_| self.slots.clear())?;
        let read = Instant::now();
        self.timing
            .record(uploaded - started, encoded - uploaded, read - encoded, read);
        let is_key_frame = kind == FrameKind::Key;
        let temporal_unit = to_qdp_temporal_unit(&bytes, is_key_frame, &self.sequence_header)
            .inspect_err(|_| self.slots.clear())?;
        let input_index = self.frames;
        self.slots.encoded(plan, input_index);
        self.frames += 1;
        Ok(vec![EncodedFrame {
            input_index,
            temporal_unit,
            is_key_frame,
            recovered_from,
        }])
    }
}

impl Drop for VaapiEncoder {
    fn drop(&mut self) {
        if let Ok(conversion) = &mut self.gpu {
            // Its context renders to the input surface, so it goes first.
            conversion.destroy(&self.display);
        }
        let va = self.display.va;
        let display = self.display.handle;
        // SAFETY: every object belongs to this display and is no longer used;
        // invalid identifiers, for objects never created, are ignored.
        unsafe {
            if let Some(staging) = self.staging {
                (va.destroy_image)(display, staging.image_id);
            }
            if self.coded != ffi::VA_INVALID_ID {
                (va.destroy_buffer)(display, self.coded);
            }
            if self.context != ffi::VA_INVALID_ID {
                (va.destroy_context)(display, self.context);
            }
            let mut surfaces = [ffi::VA_INVALID_ID; RECONSTRUCTED_SURFACES + 1];
            surfaces[0] = self.input;
            surfaces[1..].copy_from_slice(&self.reconstructed);
            if self.input != ffi::VA_INVALID_ID {
                (va.destroy_surfaces)(display, surfaces.as_mut_ptr(), surfaces.len() as c_int);
            }
            (va.destroy_config)(display, self.config);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mesa from 24.3 and Intel's driver keep any reference frame named;
    /// older Mesa and other drivers follow only the previous frame.
    #[test]
    fn chooses_how_slots_are_used() {
        let rotating = [
            "Mesa Gallium driver 26.2.4 for AMD Radeon RX 7900 XTX (radeonsi, navi31, ACO)",
            "Mesa Gallium driver 24.3.0 for AMD Radeon RX 7600",
            "Mesa Gallium driver 25.0.0-devel for AMD Radeon 780M",
            "Intel iHD driver for Intel(R) Gen Graphics - 24.2.5 ()",
        ];
        for vendor in rotating {
            assert_eq!(slot_use(vendor), SlotUse::Rotating, "{vendor}");
        }
        let previous_only = [
            "Mesa Gallium driver 24.2.8 for AMD Radeon RX 7900 XTX",
            "Mesa Gallium driver 23.3.6 for AMD Radeon RX 7600",
            "Mesa Gallium driver for something",
            "Some other driver 99.0",
            "",
        ];
        for vendor in previous_only {
            assert_eq!(slot_use(vendor), SlotUse::PreviousOnly, "{vendor}");
        }
    }

    /// Writing in bands puts each plane where the layout says, padding left
    /// alone, for sizes large enough to split and odd enough not to split
    /// evenly.
    #[test]
    fn writes_nv12_in_bands() {
        let (width, height): (usize, usize) = (1282, 1081);
        let mut seed = 3_u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (seed >> 16) as u8
        };
        let (chroma_width, chroma_height) = (width.div_ceil(2), height.div_ceil(2));
        let frame = Yuv420Frame::from_planes(
            width,
            height,
            (0..width * height).map(|_| next()).collect(),
            (0..chroma_width * chroma_height).map(|_| next()).collect(),
            (0..chroma_width * chroma_height).map(|_| next()).collect(),
        )
        .unwrap();
        // Padded rows, and a gap between the planes, as drivers lay them out.
        let layout = Nv12Layout {
            luma_offset: 64,
            luma_pitch: 1344,
            chroma_offset: 64 + 1344 * 1088,
            chroma_pitch: 1344,
        };
        const PADDING: u8 = 0xee;
        let mut data = vec![PADDING; layout.chroma_offset + layout.chroma_pitch * chroma_height];
        write_nv12(&frame, &mut data, layout).unwrap();
        for row in 0..height {
            let start = layout.luma_offset + row * layout.luma_pitch;
            assert_eq!(
                &data[start..start + width],
                &frame.y()[row * width..][..width]
            );
            assert!(
                data[start + width..start + layout.luma_pitch]
                    .iter()
                    .all(|&byte| byte == PADDING)
            );
        }
        for row in 0..chroma_height {
            let start = layout.chroma_offset + row * layout.chroma_pitch;
            for column in 0..chroma_width {
                let index = row * chroma_width + column;
                assert_eq!(data[start + 2 * column], frame.u()[index]);
                assert_eq!(data[start + 2 * column + 1], frame.v()[index]);
            }
        }
        // A layout whose planes overlap is refused.
        let overlapping = Nv12Layout {
            chroma_offset: 1344 * 100,
            ..layout
        };
        assert!(write_nv12(&frame, &mut data, overlapping).is_err());
    }
}
