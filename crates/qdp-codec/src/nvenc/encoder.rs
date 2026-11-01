//! An AV1 encoder on NVENC, made for streaming with the least delay: the
//! fastest preset tuned for ultra-low latency, P frames only, no lookahead,
//! key frames only when asked for, and each frame at the quantizer rate
//! control gives (Section 5.5). Every key frame carries the sequence header,
//! frames are divided into the tiles of Section 6.4.4, and the colour
//! description is that of the other encoders: BT.709 primaries and matrix,
//! sRGB transfer, limited range.
//!
//! Frames are copied into an NV12 input buffer NVENC holds, and encoded one
//! at a time, synchronously, on a Direct3D 11 device of the NVIDIA GPU.
//! Recovering from a loss starts afresh with a key frame.
//!
//! As it opens, the encoder encodes a black key frame and checks that its
//! sequence header is one clients take; a driver that writes otherwise is
//! not used, and the host encodes in software instead.

use std::ffi::{CStr, c_void};
use std::ptr;

use tracing::{info, warn};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{Interface, PCSTR, s, w};

use super::ffi::{self, FunctionList, Handle, Status};
use crate::backend::VideoEncoder;
use crate::d3d11va::create_for_vendor;
use crate::obu::{ObuType, parse_obus};
use crate::stream_check::check_first_key_frame;
use crate::{
    CodecError, EncodedFrame, EncoderSettings, KeyFrameRequest, NO_LEVEL, TileCount, Yuv420Frame,
    level_for, tile_count, to_qdp_temporal_unit,
};

/// NVIDIA's PCI vendor identifier.
const NVIDIA_VENDOR: u32 = 0x10de;

/// The most tile columns frames are divided into; more tiles go in rows.
const MAX_TILE_COLUMNS: u32 = 4;

/// The lowest quantizer used: 0 would make frames lossless.
const MIN_QUANTIZER: u8 = 1;

/// The driver's NVENC library, loaded from System32 only, so that no copy
/// elsewhere on the search path is loaded in its place.
struct Library(HMODULE);

impl Library {
    /// Loads the library.
    fn load() -> Result<Self, CodecError> {
        // SAFETY: the name is a valid string; the library's initialization
        // has no contract of ours.
        let module =
            unsafe { LoadLibraryExW(w!("nvEncodeAPI64.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
                .map_err(|_| {
                    unavailable("no NVIDIA driver with NVENC: nvEncodeAPI64.dll is missing")
                })?;
        Ok(Self(module))
    }

    /// Returns the address of the function `name` exports.
    fn function(&self, name: PCSTR) -> Result<unsafe extern "system" fn() -> isize, CodecError> {
        // SAFETY: the module is loaded and the name a valid string.
        unsafe { GetProcAddress(self.0, name) }.ok_or_else(|| {
            // SAFETY: the name is a valid string.
            let name = unsafe { name.to_string() }.unwrap_or_default();
            unavailable(&format!("the NVIDIA driver's NVENC has no {name}"))
        })
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: the module was loaded by `load`, and nothing it gave is
        // used once the encoder holding it is dropped.
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

/// Returns that hardware encoding is unavailable, saying why.
fn unavailable(reason: &str) -> CodecError {
    CodecError::HardwareUnavailable(reason.to_owned())
}

/// Returns how frames of `width × height` are divided into tiles: at least
/// the tiles of Section 6.4.4, in at most [`MAX_TILE_COLUMNS`] columns, all
/// counts powers of 2 as NVENC divides frames uniformly.
fn tile_grid(width: usize, height: usize) -> (u32, u32) {
    let tiles = tile_count(width, height) as u32;
    let columns = tiles.min(MAX_TILE_COLUMNS);
    (columns, tiles / columns)
}

/// An AV1 encoder on NVENC.
pub(crate) struct NvencEncoder {
    /// NVENC's functions.
    api: Box<FunctionList>,
    /// The encode session.
    session: Handle,
    /// The input buffer frames are copied into.
    input: Handle,
    /// The bitstream buffer frames are encoded into.
    bitstream: Handle,
    /// The encoding parameters, which `setup` points to.
    config: Box<ffi::Config>,
    /// The encoder's setup, kept to reconfigure it.
    setup: Box<ffi::InitializeParams>,
    /// The frames' width.
    width: usize,
    /// The frames' height.
    height: usize,
    /// The quantizer frames are encoded at.
    quantizer: u8,
    /// The quantizer rate control asks for, from the next frame.
    wanted_quantizer: u8,
    /// The index of the next frame passed.
    next_index: u64,
    /// The stream's sequence header OBU, from its first key frame.
    sequence_header: Vec<u8>,
    /// Which encoder this is, for logs.
    description: String,
    /// The device NVENC encodes on, kept while the session uses it.
    _device: ID3D11Device,
    /// The library, kept loaded while its functions are used.
    _library: Library,
}

impl NvencEncoder {
    /// Opens an encoder of `settings` on the first NVIDIA GPU.
    ///
    /// # Errors
    ///
    /// Fails with [`CodecError::HardwareUnavailable`] without an NVIDIA GPU
    /// that encodes AV1 and a driver for it, and with other errors if NVENC
    /// refuses the encoder.
    pub(crate) fn new(settings: EncoderSettings) -> Result<Self, CodecError> {
        let (width, height) = (settings.width, settings.height);
        if width % 2 != 0 || height % 2 != 0 {
            return Err(unavailable("NVENC encodes frames of even sizes only"));
        }
        let library = Library::load()?;
        let max_version = library.function(s!("NvEncodeAPIGetMaxSupportedVersion"))?;
        // SAFETY: the export has this signature, `ffi::GetMaxSupportedVersion`.
        let max_version: ffi::GetMaxSupportedVersion = unsafe { std::mem::transmute(max_version) };
        let mut version = 0;
        // SAFETY: the version is written before the call returns.
        if unsafe { max_version(&mut version) } != ffi::SUCCESS
            || version < ffi::API_VERSION_REPORTED
        {
            return Err(unavailable(
                "the NVIDIA driver is too old to encode AV1: version 522 or later is needed",
            ));
        }
        let create_instance = library.function(s!("NvEncodeAPICreateInstance"))?;
        // SAFETY: the export has this signature, `ffi::CreateInstance`.
        let create_instance: ffi::CreateInstance = unsafe { std::mem::transmute(create_instance) };
        // SAFETY: the list is valid zeroed.
        let mut api: Box<FunctionList> = Box::new(unsafe { ffi::zeroed() });
        api.version = ffi::FUNCTION_LIST_VER;
        // SAFETY: the list is valid to write, with its version set.
        if unsafe { create_instance(&mut *api) } != ffi::SUCCESS {
            return Err(unavailable("the NVIDIA driver's NVENC gave no functions"));
        }
        let (device, gpu) =
            create_for_vendor(NVIDIA_VENDOR).map_err(|_| unavailable("there is no NVIDIA GPU"))?;
        let session = open_session(&api, &device)?;
        let (columns, rows) = tile_grid(width, height);
        let level = level_for(
            width as u32,
            height as u32,
            settings.frame_rate,
            TileCount {
                tiles: columns * rows,
                columns,
            },
        );
        let mut encoder = Self {
            api,
            session,
            input: ptr::null_mut(),
            bitstream: ptr::null_mut(),
            // SAFETY: the structures are valid zeroed.
            config: Box::new(unsafe { ffi::zeroed() }),
            // SAFETY: as above.
            setup: Box::new(unsafe { ffi::zeroed() }),
            width,
            height,
            quantizer: settings.quantizer.max(MIN_QUANTIZER),
            wanted_quantizer: settings.quantizer.max(MIN_QUANTIZER),
            next_index: 0,
            sequence_header: Vec::new(),
            description: format!("NVENC on {gpu}"),
            _device: device,
            _library: library,
        };
        // From here on, `Drop` releases whatever has been created.
        if level == NO_LEVEL {
            return Err(unavailable("the frames are too large for any AV1 level"));
        }
        encoder.check_av1()?;
        encoder.initialize(settings.frame_rate, level, (columns, rows))?;
        encoder.create_buffers()?;
        encoder.check_stream(level)?;
        info!(
            width,
            height,
            level,
            tile_columns = columns,
            tile_rows = rows,
            encoder = encoder.description,
            "encoding AV1 with NVENC"
        );
        Ok(encoder)
    }

    /// Returns an error for `status` from `what`, with NVENC's explanation,
    /// or nothing if it succeeded.
    fn check(&self, status: Status, what: &str) -> Result<(), CodecError> {
        if status == ffi::SUCCESS {
            return Ok(());
        }
        let detail = self
            .api
            .get_last_error_string
            .and_then(|last_error| {
                // SAFETY: the session is open; the string is NVENC's, valid
                // until its next call.
                let text = unsafe { last_error(self.session) };
                // SAFETY: a non-null string NVENC returns ends with a nul.
                (!text.is_null()).then(|| {
                    unsafe { CStr::from_ptr(text) }
                        .to_string_lossy()
                        .into_owned()
                })
            })
            .unwrap_or_default();
        Err(CodecError::Encoder(format!(
            "NVENC's {what} failed with status {status}: {detail}"
        )))
    }

    /// Returns NVENC's function, or an error if the driver has none.
    fn function<F: Copy>(function: Option<F>, name: &str) -> Result<F, CodecError> {
        function.ok_or_else(|| unavailable(&format!("the NVIDIA driver's NVENC has no {name}")))
    }

    /// Fails unless the GPU encodes AV1.
    fn check_av1(&self) -> Result<(), CodecError> {
        let count_guids =
            Self::function(self.api.get_encode_guid_count, "nvEncGetEncodeGUIDCount")?;
        let get_guids = Self::function(self.api.get_encode_guids, "nvEncGetEncodeGUIDs")?;
        let mut count = 0;
        // SAFETY: the session is open; the count is written before the call
        // returns.
        self.check(
            unsafe { count_guids(self.session, &mut count) },
            "codec count",
        )?;
        let mut guids = vec![ffi::Guid::default(); count as usize];
        let mut written = 0;
        // SAFETY: the array holds `count` GUIDs.
        let status = unsafe { get_guids(self.session, guids.as_mut_ptr(), count, &mut written) };
        self.check(status, "codec list")?;
        guids.truncate(written as usize);
        if guids.contains(&ffi::CODEC_AV1_GUID) {
            Ok(())
        } else {
            Err(unavailable(
                "this NVIDIA GPU does not encode AV1: GeForce RTX 40 and later do",
            ))
        }
    }

    /// Sets the encoder up for frames at `frame_rate` of `level`, in
    /// `tiles` columns and rows, from the fastest preset tuned for ultra-low
    /// latency.
    fn initialize(
        &mut self,
        frame_rate: u32,
        level: u8,
        tiles: (u32, u32),
    ) -> Result<(), CodecError> {
        let preset_config = Self::function(
            self.api.get_encode_preset_config_ex,
            "nvEncGetEncodePresetConfigEx",
        )?;
        let initialize = Self::function(self.api.initialize_encoder, "nvEncInitializeEncoder")?;
        // SAFETY: the structure is valid zeroed; it is large, so boxed.
        let mut preset: Box<ffi::PresetConfig> = Box::new(unsafe { ffi::zeroed() });
        preset.version = ffi::PRESET_CONFIG_VER;
        preset.preset_cfg.version = ffi::CONFIG_VER;
        // SAFETY: the session is open; the preset is written before the call
        // returns.
        let status = unsafe {
            preset_config(
                self.session,
                ffi::CODEC_AV1_GUID,
                ffi::PRESET_P1_GUID,
                ffi::TUNING_ULTRA_LOW_LATENCY,
                &mut *preset,
            )
        };
        self.check(status, "preset")?;
        *self.config = preset.preset_cfg;
        configure(&mut self.config, level, tiles, self.quantizer);

        let setup = &mut *self.setup;
        setup.version = ffi::INITIALIZE_PARAMS_VER;
        setup.encode_guid = ffi::CODEC_AV1_GUID;
        setup.preset_guid = ffi::PRESET_P1_GUID;
        setup.encode_width = self.width as u32;
        setup.encode_height = self.height as u32;
        setup.dar_width = self.width as u32;
        setup.dar_height = self.height as u32;
        setup.frame_rate_num = frame_rate.max(1);
        setup.frame_rate_den = 1;
        setup.enable_encode_async = 0;
        setup.enable_ptd = 1;
        setup.encode_config = &mut *self.config;
        setup.max_encode_width = self.width as u32;
        setup.max_encode_height = self.height as u32;
        setup.tuning_info = ffi::TUNING_ULTRA_LOW_LATENCY;
        // SAFETY: the session is open, and the setup and the configuration
        // it points to are valid for the call.
        let status = unsafe { initialize(self.session, &mut *self.setup) };
        self.check(status, "initialization")
    }

    /// Makes the input and bitstream buffers.
    fn create_buffers(&mut self) -> Result<(), CodecError> {
        let create_input = Self::function(self.api.create_input_buffer, "nvEncCreateInputBuffer")?;
        let create_bitstream = Self::function(
            self.api.create_bitstream_buffer,
            "nvEncCreateBitstreamBuffer",
        )?;
        // SAFETY: the structure is valid zeroed.
        let mut input: ffi::CreateInputBuffer = unsafe { ffi::zeroed() };
        input.version = ffi::CREATE_INPUT_BUFFER_VER;
        input.width = self.width as u32;
        input.height = self.height as u32;
        input.memory_heap = ffi::MEMORY_HEAP_AUTOSELECT;
        input.buffer_fmt = ffi::BUFFER_FORMAT_NV12;
        // SAFETY: the session is initialized; the buffer is written before
        // the call returns.
        let status = unsafe { create_input(self.session, &mut input) };
        self.check(status, "input buffer")?;
        self.input = input.input_buffer;
        // SAFETY: the structure is valid zeroed.
        let mut bitstream: ffi::CreateBitstreamBuffer = unsafe { ffi::zeroed() };
        bitstream.version = ffi::CREATE_BITSTREAM_BUFFER_VER;
        // SAFETY: as above.
        let status = unsafe { create_bitstream(self.session, &mut bitstream) };
        self.check(status, "bitstream buffer")?;
        self.bitstream = bitstream.bitstream_buffer;
        Ok(())
    }

    /// Encodes a black key frame and checks that it suits clients, as
    /// [`check_first_key_frame`] does. The stream starts afresh with its
    /// first frame.
    fn check_stream(&mut self, level: u8) -> Result<(), CodecError> {
        let pitch = self.upload(&Yuv420Frame::black(self.width, self.height))?;
        let (packet, is_key_frame) = self
            .encode_input(0, pitch, true)?
            .ok_or_else(|| unavailable("NVENC held back its first frame"))?;
        let unit = to_qdp_temporal_unit(&packet, is_key_frame, &[])?;
        self.sequence_header = check_first_key_frame(&unit, self.width, self.height, level)
            .map_err(|problem| {
                unavailable(&format!("NVENC's stream does not suit clients: {problem}"))
            })?;
        Ok(())
    }

    /// Encodes from now on at the quantizer rate control asked for, if it
    /// changed; a failure keeps the one before.
    fn apply_quantizer(&mut self) {
        if self.wanted_quantizer == self.quantizer {
            return;
        }
        let Some(reconfigure) = self.api.reconfigure_encoder else {
            return;
        };
        let quantizer = u32::from(self.wanted_quantizer);
        self.config.rc_params.const_qp = ffi::Qp {
            inter_p: quantizer,
            inter_b: quantizer,
            intra: quantizer,
        };
        // SAFETY: the structure is valid zeroed; it is large, so boxed.
        let mut params: Box<ffi::ReconfigureParams> = Box::new(unsafe { ffi::zeroed() });
        params.version = ffi::RECONFIGURE_PARAMS_VER;
        params.re_init_encode_params = *self.setup;
        // SAFETY: the session is initialized; the setup points to the
        // configuration, valid for the call.
        let status = unsafe { reconfigure(self.session, &mut *params) };
        match self.check(status, "reconfiguration") {
            Ok(()) => self.quantizer = self.wanted_quantizer,
            Err(error) => {
                warn!(%error, "keeping the quantizer");
                self.wanted_quantizer = self.quantizer;
            }
        }
    }

    /// Copies `frame` into the input buffer as NV12, returning the buffer's
    /// pitch.
    fn upload(&mut self, frame: &Yuv420Frame) -> Result<u32, CodecError> {
        let lock = Self::function(self.api.lock_input_buffer, "nvEncLockInputBuffer")?;
        let unlock = Self::function(self.api.unlock_input_buffer, "nvEncUnlockInputBuffer")?;
        // SAFETY: the structure is valid zeroed.
        let mut locked: ffi::LockInputBuffer = unsafe { ffi::zeroed() };
        locked.version = ffi::LOCK_INPUT_BUFFER_VER;
        locked.input_buffer = self.input;
        // SAFETY: the buffer is the session's; the lock is written before the
        // call returns.
        let status = unsafe { lock(self.session, &mut locked) };
        self.check(status, "input lock")?;
        let pitch = locked.pitch as usize;
        let (width, height) = (self.width, self.height);
        let (chroma_width, chroma_height) = (frame.chroma_width(), frame.chroma_height());
        // SAFETY: the locked NV12 buffer holds `height` rows of luma and
        // `height / 2` rows of interleaved chroma, `pitch` bytes apart, at
        // least `width` bytes each, until unlocked.
        let pixels = unsafe {
            std::slice::from_raw_parts_mut(
                locked.buffer_data_ptr.cast::<u8>(),
                pitch * (height + chroma_height),
            )
        };
        for (row, line) in frame.y().chunks_exact(width).enumerate() {
            pixels[row * pitch..row * pitch + width].copy_from_slice(line);
        }
        let chroma = &mut pixels[pitch * height..];
        let rows = frame
            .u()
            .chunks_exact(chroma_width)
            .zip(frame.v().chunks_exact(chroma_width));
        for (row, (u, v)) in rows.enumerate().take(chroma_height) {
            let line = &mut chroma[row * pitch..row * pitch + chroma_width * 2];
            for (pair, (&u, &v)) in line.chunks_exact_mut(2).zip(u.iter().zip(v)) {
                pair[0] = u;
                pair[1] = v;
            }
        }
        // SAFETY: the buffer was locked above.
        let status = unsafe { unlock(self.session, self.input) };
        self.check(status, "input unlock")?;
        Ok(locked.pitch)
    }

    /// Encodes the input buffer's frame, `index`, as a key frame if `key`,
    /// returning the encoded picture and whether it is a key frame, or
    /// nothing if NVENC holds it back.
    fn encode_input(
        &mut self,
        index: u64,
        pitch: u32,
        key: bool,
    ) -> Result<Option<(Vec<u8>, bool)>, CodecError> {
        let encode = Self::function(self.api.encode_picture, "nvEncEncodePicture")?;
        let lock = Self::function(self.api.lock_bitstream, "nvEncLockBitstream")?;
        let unlock = Self::function(self.api.unlock_bitstream, "nvEncUnlockBitstream")?;
        // SAFETY: the structure is valid zeroed; it is large, so boxed.
        let mut picture: Box<ffi::PicParams> = Box::new(unsafe { ffi::zeroed() });
        picture.version = ffi::PIC_PARAMS_VER;
        picture.input_width = self.width as u32;
        picture.input_height = self.height as u32;
        picture.input_pitch = pitch;
        picture.encode_pic_flags = if key { ffi::PIC_FLAG_FORCE_IDR } else { 0 };
        picture.frame_idx = index as u32;
        picture.input_time_stamp = index;
        picture.input_buffer = self.input;
        picture.output_bitstream = self.bitstream;
        picture.buffer_fmt = ffi::BUFFER_FORMAT_NV12;
        picture.picture_struct = ffi::PIC_STRUCT_FRAME;
        // SAFETY: the session is initialized and the buffers are its own.
        let status = unsafe { encode(self.session, &mut *picture) };
        if status == ffi::ERR_NEED_MORE_INPUT {
            return Ok(None);
        }
        self.check(status, "encoding")?;
        // SAFETY: the structure is valid zeroed.
        let mut locked: ffi::LockBitstream = unsafe { ffi::zeroed() };
        locked.version = ffi::LOCK_BITSTREAM_VER;
        locked.output_bitstream = self.bitstream;
        // SAFETY: the bitstream buffer is the session's; locking waits for
        // the encoding and writes the lock before returning.
        let status = unsafe { lock(self.session, &mut locked) };
        self.check(status, "bitstream lock")?;
        // SAFETY: the locked buffer holds `bitstream_size_in_bytes` bytes
        // until unlocked.
        let packet = unsafe {
            std::slice::from_raw_parts(
                locked.bitstream_buffer_ptr.cast::<u8>(),
                locked.bitstream_size_in_bytes as usize,
            )
        }
        .to_vec();
        // SAFETY: the buffer was locked above.
        let status = unsafe { unlock(self.session, self.bitstream) };
        self.check(status, "bitstream unlock")?;
        Ok(Some((packet, locked.picture_type == ffi::PIC_TYPE_IDR)))
    }
}

/// Opens an encode session on `device`.
fn open_session(api: &FunctionList, device: &ID3D11Device) -> Result<Handle, CodecError> {
    let open = api
        .open_encode_session_ex
        .ok_or_else(|| unavailable("the NVIDIA driver's NVENC has no nvEncOpenEncodeSessionEx"))?;
    // SAFETY: the structure is valid zeroed.
    let mut params: ffi::OpenEncodeSessionExParams = unsafe { ffi::zeroed() };
    params.version = ffi::OPEN_ENCODE_SESSION_EX_PARAMS_VER;
    params.device_type = ffi::DEVICE_TYPE_DIRECTX;
    params.device = device.as_raw();
    params.api_version = ffi::API_VERSION;
    let mut session: Handle = ptr::null_mut();
    // SAFETY: the device is valid while the session is open, as the encoder
    // keeps it; the session is written before the call returns.
    let status = unsafe { open(&mut params, &mut session) };
    if status != ffi::SUCCESS || session.is_null() {
        return Err(unavailable(&format!(
            "NVENC refused a session on this GPU (status {status}), as when all its sessions are taken"
        )));
    }
    Ok(session)
}

/// Sets `config`, a preset's, to stream AV1 at `level` in `tiles` columns
/// and rows, at a constant `quantizer`, with no B frames, no lookahead and
/// key frames only when asked for.
fn configure(config: &mut ffi::Config, level: u8, tiles: (u32, u32), quantizer: u8) {
    config.profile_guid = ffi::AV1_PROFILE_MAIN_GUID;
    config.gop_length = ffi::INFINITE_GOP_LENGTH;
    config.frame_interval_p = 1;
    let quantizer = u32::from(quantizer);
    let rate_control = &mut config.rc_params;
    rate_control.version = ffi::RC_PARAMS_VER;
    rate_control.rate_control_mode = ffi::RC_CONST_QP;
    rate_control.flags &= !ffi::RC_ADAPTIVE;
    rate_control.lookahead_depth = 0;
    rate_control.const_qp = ffi::Qp {
        inter_p: quantizer,
        inter_b: quantizer,
        intra: quantizer,
    };
    // SAFETY: the preset is AV1's, so its union holds AV1's parameters.
    let av1 = unsafe { &mut config.encode_codec_config.av1 };
    av1.level = u32::from(level);
    av1.tier = ffi::TIER_AV1_MAIN;
    av1.flags &= !(ffi::AV1_UNWANTED
        | ffi::AV1_PADDING_TILES_GRAIN
        | ffi::AV1_BIT_DEPTHS
        | (ffi::AV1_CHROMA_FORMAT_MASK << ffi::AV1_CHROMA_FORMAT_SHIFT));
    av1.flags |=
        ffi::AV1_REPEAT_SEQUENCE_HEADER | (ffi::AV1_CHROMA_420 << ffi::AV1_CHROMA_FORMAT_SHIFT);
    av1.idr_period = ffi::INFINITE_GOP_LENGTH;
    av1.intra_refresh_period = 0;
    av1.intra_refresh_cnt = 0;
    av1.num_tile_columns = tiles.0;
    av1.num_tile_rows = tiles.1;
    av1.tile_widths = ptr::null_mut();
    av1.tile_heights = ptr::null_mut();
    av1.max_temporal_layers_minus1 = 0;
    av1.color_primaries = ffi::COLOR_PRIMARIES_BT709;
    av1.transfer_characteristics = ffi::TRANSFER_SRGB;
    av1.matrix_coefficients = ffi::MATRIX_BT709;
    av1.color_range = 0;
    av1.chroma_sample_position = 0;
    av1.use_b_frames_as_ref = ffi::BFRAME_REF_MODE_DISABLED;
    av1.film_grain_params = ptr::null_mut::<c_void>();
}

impl VideoEncoder for NvencEncoder {
    fn encode(
        &mut self,
        frame: &Yuv420Frame,
        request: KeyFrameRequest,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        if frame.width() != self.width || frame.height() != self.height {
            return Err(CodecError::FrameSize);
        }
        let index = self.next_index;
        self.next_index += 1;
        self.apply_quantizer();
        let pitch = self.upload(frame)?;
        // The first frame follows the stream check's, and a recovery frame
        // starts afresh: NVENC is not asked which frames the receiver holds.
        let key = index == 0 || request.restarts();
        let Some((packet, is_key_frame)) = self.encode_input(index, pitch, key)? else {
            return Ok(Vec::new());
        };
        if is_key_frame && self.sequence_header.is_empty() {
            self.sequence_header = parse_obus(&packet)?
                .iter()
                .find(|obu| obu.obu_type == ObuType::SequenceHeader)
                .map(|obu| obu.bytes.to_vec())
                .ok_or_else(|| {
                    CodecError::Encoder("NVENC's key frame has no sequence header".to_owned())
                })?;
        }
        let temporal_unit = to_qdp_temporal_unit(&packet, is_key_frame, &self.sequence_header)?;
        Ok(vec![EncodedFrame {
            input_index: index,
            temporal_unit,
            is_key_frame,
            recovered_from: None,
        }])
    }

    fn set_quantizer(&mut self, quantizer: u8) {
        self.wanted_quantizer = quantizer.max(MIN_QUANTIZER);
    }

    fn description(&self) -> &str {
        &self.description
    }
}

impl Drop for NvencEncoder {
    fn drop(&mut self) {
        // SAFETY: the buffers and session are the encoder's own, destroyed
        // once each, buffers first; null ones were never made.
        unsafe {
            if let (false, Some(destroy)) = (self.input.is_null(), self.api.destroy_input_buffer) {
                destroy(self.session, self.input);
            }
            if let (false, Some(destroy)) =
                (self.bitstream.is_null(), self.api.destroy_bitstream_buffer)
            {
                destroy(self.session, self.bitstream);
            }
            if let Some(destroy) = self.api.destroy_encoder {
                destroy(self.session);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames are divided into at least Section 6.4.4's tiles, in columns
    /// first.
    #[test]
    fn divides_frames_into_tiles() {
        assert_eq!(tile_grid(1920, 1080), (2, 1));
        assert_eq!(tile_grid(2560, 1440), (4, 1));
        assert_eq!(tile_grid(3840, 2160), (4, 2));
    }
}
