//! Hardware AV1 encoding through Media Foundation, with the encoder a GPU's
//! driver registers there: AMD's on Radeon RX 7000 and later, Intel's on Arc
//! and later. NVIDIA registers none, and NVENC encodes on its GPUs instead.
//!
//! The encoder is set up for streaming with the least delay: low-latency
//! mode, no B frames, key frames only when asked for, and each frame at the
//! quantizer rate control gives (Section 5.5); it is asked for the tiles of
//! Section 6.4.4 and the colour description of the other encoders. Hardware
//! encoders are asynchronous: each frame is handed over once the encoder
//! asks for one, and its encoding taken once the encoder has it, so that
//! frames are encoded one at a time, as they come. Recovering from a loss
//! starts afresh with a key frame.
//!
//! As it opens, the encoder encodes a black key frame, which must suit
//! clients as [`check_first_key_frame`] checks; an encoder that does not,
//! or that holds frames back, is not used, and the host encodes in software
//! instead.
//!
//! Media Foundation is a COM interface, so this module uses `unsafe`: each
//! block states the contract it relies on, from Microsoft's documentation.

#![allow(unsafe_code)]

use std::cell::Cell;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use tracing::{debug, info};
use windows::Win32::Media::MediaFoundation::{
    CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVDefaultBPictureCount,
    CODECAPI_AVEncMPVGOPSize, CODECAPI_AVEncTileColumns, CODECAPI_AVEncTileRows,
    CODECAPI_AVEncVideoEncodeQP, CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode,
    ICodecAPI, IMFActivate, IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFTransform, MEError,
    METransformHaveOutput, METransformNeedInput, MF_E_NO_EVENTS_AVAILABLE,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_EVENT_FLAG_NO_WAIT, MF_LOW_LATENCY, MF_MT_AVG_BITRATE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_MPEG2_PROFILE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_MT_TRANSFER_FUNCTION, MF_MT_VIDEO_NOMINAL_RANGE, MF_MT_VIDEO_PRIMARIES, MF_MT_YUV_MATRIX,
    MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFNominalRange_16_235,
    MFSTARTUP_NOSOCKET, MFSampleExtension_CleanPoint, MFStartup, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER, MFT_FRIENDLY_NAME_Attribute,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFT_OUTPUT_STREAM_PROVIDES_SAMPLES, MFT_REGISTER_TYPE_INFO, MFTEnumEx, MFVideoFormat_AV1,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFVideoPrimaries_BT709,
    MFVideoTransFunc_sRGB, MFVideoTransferMatrix_BT709, eAVEncAV1VProfile_Main_420_8,
    eAVEncCommonRateControlMode_Quality,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{GUID, Interface, PWSTR};

use crate::backend::VideoEncoder;
use crate::obu::{ObuType, parse_obus};
use crate::stream_check::{check_first_key_frame, tile_grid};
use crate::{
    CodecError, EncodedFrame, EncoderSettings, KeyFrameRequest, NO_LEVEL, TileCount, Yuv420Frame,
    level_for, to_qdp_temporal_unit,
};

/// The lowest quantizer used: 0 would make frames lossless.
const MIN_QUANTIZER: u8 = 1;

/// The longest the encoder may take to ask for a frame or encode one.
const ENCODE_TIMEOUT: Duration = Duration::from_secs(2);

/// How long to wait for the encoder by yielding the processor, before
/// sleeping between looks: most frames take less.
const SPIN: Duration = Duration::from_millis(2);

/// How long to sleep between looks at the encoder's events once spinning
/// is over.
const POLL_INTERVAL: Duration = Duration::from_micros(250);

/// The bit rate the output type states, which some encoders want although
/// they encode at the quantizer given: a generous one for streaming.
const NOMINAL_BIT_RATE: u32 = 50_000_000;

/// The group of pictures asked for: as long as encoders take, so that key
/// frames come only when asked for.
const GOP_SIZE: u32 = i32::MAX as u32;

/// Media Foundation's time units in a second.
const TICKS_PER_SECOND: i64 = 10_000_000;

/// The bit an `AV1CodecConfigurationRecord` begins with, which raw OBUs
/// never do, as their forbidden bit is clear.
const CONFIGURATION_RECORD_MARKER: u8 = 0x80;

/// The bytes of an `AV1CodecConfigurationRecord` before its OBUs.
const CONFIGURATION_RECORD_HEADER: usize = 4;

/// Starts Media Foundation once, for the process.
fn start_media_foundation() -> Result<(), CodecError> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    STARTED
        .get_or_init(|| {
            // SAFETY: the call has no contract; Media Foundation stays started
            // for the process.
            unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }
                .map_err(|error| format!("Media Foundation does not start: {error}"))
        })
        .clone()
        .map_err(CodecError::HardwareUnavailable)
}

/// Joins this thread to COM's multithreaded apartment, once, as Media
/// Foundation's objects need; a thread already in an apartment stays in it.
fn join_com() {
    thread_local! {
        /// Whether this thread joined.
        static JOINED: Cell<bool> = const { Cell::new(false) };
    }
    if !JOINED.replace(true) {
        // SAFETY: the call has no other contract; the thread stays joined
        // for its life.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }
}

/// Returns that hardware encoding is unavailable, saying why.
fn unavailable(reason: &str) -> CodecError {
    CodecError::HardwareUnavailable(reason.to_owned())
}

/// Returns a Media Foundation failure of `what`.
fn failed(what: &str, error: &windows::core::Error) -> CodecError {
    CodecError::Encoder(format!("Media Foundation's {what} failed: {error}"))
}

/// Returns `high` and `low` packed into one attribute value, as frame sizes
/// and rates are.
fn packed(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// Returns the hardware encoders from NV12 to AV1, best first.
fn hardware_encoders() -> Result<Vec<IMFActivate>, CodecError> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_AV1,
    };
    let mut found: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0;
    // SAFETY: the types are valid for the call; the array and count are
    // written before it returns.
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&input),
            Some(&output),
            &mut found,
            &mut count,
        )
    }
    .map_err(|error| failed("encoder search", &error))?;
    if found.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: the array holds `count` activation objects, which are taken
    // over here, each once; the array itself is then freed.
    let encoders = unsafe {
        let encoders = (0..count as usize)
            .filter_map(|index| found.add(index).read())
            .collect();
        CoTaskMemFree(Some(found.cast_const().cast()));
        encoders
    };
    Ok(encoders)
}

/// Returns the name an activation object gives its encoder.
fn friendly_name(activate: &IMFActivate) -> String {
    let mut name = PWSTR::null();
    let mut length = 0;
    // SAFETY: the name and its length are written before the call returns;
    // the name is freed once read.
    unsafe {
        if activate
            .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &mut name, &mut length)
            .is_err()
        {
            return "a hardware AV1 encoder".to_owned();
        }
        let text = name.to_string().unwrap_or_default();
        CoTaskMemFree(Some(name.as_ptr().cast_const().cast()));
        text
    }
}

/// An AV1 encoder through Media Foundation.
pub(crate) struct MediaFoundationEncoder {
    /// The encoder.
    transform: IMFTransform,
    /// The encoder's events.
    events: IMFMediaEventGenerator,
    /// The encoder's settings.
    codec: ICodecAPI,
    /// Whether the encoder gives its own output samples.
    provides_samples: bool,
    /// The size of output samples the encoder wants, otherwise.
    output_size: u32,
    /// The frames the encoder asked for and has not been given.
    wanted_frames: u32,
    /// The frames' width.
    width: usize,
    /// The frames' height.
    height: usize,
    /// The frames per second.
    frame_rate: u32,
    /// The quantizer frames are encoded at.
    quantizer: u8,
    /// The quantizer rate control asks for, from the next frame.
    wanted_quantizer: u8,
    /// The index of the next frame passed.
    next_index: u64,
    /// The stream's sequence header OBU.
    sequence_header: Vec<u8>,
    /// Which encoder this is, for logs.
    description: String,
}

impl MediaFoundationEncoder {
    /// Opens an encoder of `settings` with the best hardware AV1 encoder
    /// Media Foundation has.
    ///
    /// # Errors
    ///
    /// Fails with [`CodecError::HardwareUnavailable`] without such an
    /// encoder, or one whose stream suits clients.
    pub(crate) fn new(settings: EncoderSettings) -> Result<Self, CodecError> {
        let (width, height) = (settings.width, settings.height);
        if width % 2 != 0 || height % 2 != 0 {
            return Err(unavailable(
                "hardware encoders encode frames of even sizes only",
            ));
        }
        start_media_foundation()?;
        join_com();
        let activate = hardware_encoders()?
            .into_iter()
            .next()
            .ok_or_else(|| unavailable("no GPU driver registers a hardware AV1 encoder"))?;
        let name = friendly_name(&activate);
        // SAFETY: the activation object is valid; the encoder it makes is the
        // caller's.
        let transform: IMFTransform = unsafe { activate.ActivateObject() }
            .map_err(|error| failed("encoder activation", &error))?;
        // SAFETY: the encoder is valid; asynchronous encoders must be
        // unlocked before use, and low latency asked for before the types.
        unsafe {
            let attributes = transform
                .GetAttributes()
                .map_err(|error| failed("encoder attributes", &error))?;
            if attributes.GetUINT32(&MF_TRANSFORM_ASYNC) != Ok(1) {
                return Err(unavailable(&format!("{name} is not a hardware encoder")));
            }
            attributes
                .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                .map_err(|error| failed("encoder unlock", &error))?;
            let _ = attributes.SetUINT32(&MF_LOW_LATENCY, 1);
        }
        let codec: ICodecAPI = transform
            .cast()
            .map_err(|error| failed("encoder settings", &error))?;
        let events: IMFMediaEventGenerator = transform
            .cast()
            .map_err(|error| failed("encoder events", &error))?;
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
        if level == NO_LEVEL {
            return Err(unavailable("the frames are too large for any AV1 level"));
        }
        let quantizer = settings.quantizer.max(MIN_QUANTIZER);
        let mut encoder = Self {
            transform,
            events,
            codec,
            provides_samples: false,
            output_size: 0,
            wanted_frames: 0,
            width,
            height,
            frame_rate: settings.frame_rate.max(1),
            quantizer,
            wanted_quantizer: quantizer,
            next_index: 0,
            sequence_header: Vec::new(),
            description: format!("Media Foundation's {name}"),
        };
        encoder.configure((columns, rows))?;
        encoder.set_types()?;
        encoder.start()?;
        encoder.check_stream(level)?;
        info!(
            width,
            height,
            level,
            encoder = encoder.description,
            "encoding AV1 with Media Foundation"
        );
        Ok(encoder)
    }

    /// Sets `value` of the encoder setting `api`, saying whether the
    /// encoder took it.
    fn set(&self, api: &GUID, value: VARIANT) -> bool {
        // SAFETY: the setting's GUID and value are valid for the call.
        unsafe { self.codec.SetValue(api, &value) }.is_ok()
    }

    /// Asks the encoder for streaming at a constant quantizer, in `tiles`
    /// columns and rows, with no B frames and key frames only when asked
    /// for; encoders that do not take a setting other than the rate control
    /// mode keep their own, which the stream check judges.
    fn configure(&mut self, tiles: (u32, u32)) -> Result<(), CodecError> {
        if !self.set(
            &CODECAPI_AVEncCommonRateControlMode,
            VARIANT::from(eAVEncCommonRateControlMode_Quality.0 as u32),
        ) {
            return Err(unavailable(&format!(
                "{} does not encode at a constant quantizer",
                self.description
            )));
        }
        let settings = [
            (
                "low latency",
                CODECAPI_AVLowLatencyMode,
                VARIANT::from(true),
            ),
            (
                "no B frames",
                CODECAPI_AVEncMPVDefaultBPictureCount,
                VARIANT::from(0_u32),
            ),
            (
                "group of pictures",
                CODECAPI_AVEncMPVGOPSize,
                VARIANT::from(GOP_SIZE),
            ),
            (
                "tile columns",
                CODECAPI_AVEncTileColumns,
                VARIANT::from(tiles.0),
            ),
            ("tile rows", CODECAPI_AVEncTileRows, VARIANT::from(tiles.1)),
        ];
        for (what, api, value) in settings {
            if !self.set(&api, value) {
                debug!(setting = what, encoder = self.description, "not taken");
            }
        }
        self.apply_quantizer(true);
        Ok(())
    }

    /// Returns a video media type of `subtype`, of the frames' size and
    /// rate, progressive, square pixels, and the colour description of the
    /// other encoders in limited range.
    fn media_type(&self, subtype: &GUID) -> Result<IMFMediaType, CodecError> {
        // SAFETY: the type is made here; the attributes are valid.
        unsafe {
            let media_type = MFCreateMediaType().map_err(|error| failed("media type", &error))?;
            let attributes = [
                media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video),
                media_type.SetGUID(&MF_MT_SUBTYPE, subtype),
                media_type.SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    packed(self.width as u32, self.height as u32),
                ),
                media_type.SetUINT64(&MF_MT_FRAME_RATE, packed(self.frame_rate, 1)),
                media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, packed(1, 1)),
                media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32),
                media_type.SetUINT32(&MF_MT_VIDEO_PRIMARIES, MFVideoPrimaries_BT709.0 as u32),
                media_type.SetUINT32(&MF_MT_TRANSFER_FUNCTION, MFVideoTransFunc_sRGB.0 as u32),
                media_type.SetUINT32(&MF_MT_YUV_MATRIX, MFVideoTransferMatrix_BT709.0 as u32),
                media_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, MFNominalRange_16_235.0 as u32),
            ];
            for result in attributes {
                result.map_err(|error| failed("media type", &error))?;
            }
            Ok(media_type)
        }
    }

    /// Sets the encoder's output type, AV1 profile 0, and then its input
    /// type, NV12, as encoders want them in that order.
    fn set_types(&mut self) -> Result<(), CodecError> {
        let output = self.media_type(&MFVideoFormat_AV1)?;
        let input = self.media_type(&MFVideoFormat_NV12)?;
        // SAFETY: the types are valid; the encoder takes them in turn.
        unsafe {
            output
                .SetUINT32(&MF_MT_AVG_BITRATE, NOMINAL_BIT_RATE)
                .and_then(|()| {
                    output.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncAV1VProfile_Main_420_8.0 as u32)
                })
                .map_err(|error| failed("output type", &error))?;
            self.transform.SetOutputType(0, &output, 0).map_err(|_| {
                unavailable(&format!("{} refuses AV1 of this size", self.description))
            })?;
            self.transform
                .SetInputType(0, &input, 0)
                .map_err(|_| unavailable(&format!("{} refuses NV12 input", self.description)))?;
            let info = self
                .transform
                .GetOutputStreamInfo(0)
                .map_err(|error| failed("output stream", &error))?;
            self.provides_samples = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
            self.output_size = info.cbSize;
        }
        Ok(())
    }

    /// Starts the stream, after which the encoder asks for frames.
    fn start(&mut self) -> Result<(), CodecError> {
        // SAFETY: the encoder's types are set.
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .and_then(|()| {
                    self.transform
                        .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                })
        }
        .map_err(|error| failed("stream start", &error))
    }

    /// Encodes a black key frame and checks that it suits clients, as
    /// [`check_first_key_frame`] does, keeping its sequence header, or the
    /// output type's if the frame has none. The stream starts afresh with
    /// its first frame.
    fn check_stream(&mut self, level: u8) -> Result<(), CodecError> {
        let (packet, is_key_frame) = self
            .encode_frame(&Yuv420Frame::black(self.width, self.height), 0, true)
            .map_err(|error| {
                unavailable(&format!("{} cannot encode: {error}", self.description))
            })?;
        let fallback = self.output_type_sequence_header().unwrap_or_default();
        let unit = to_qdp_temporal_unit(&packet, is_key_frame, &fallback)?;
        self.sequence_header = check_first_key_frame(&unit, self.width, self.height, level)
            .map_err(|problem| {
                unavailable(&format!(
                    "{}'s stream does not suit clients: {problem}",
                    self.description
                ))
            })?;
        Ok(())
    }

    /// Returns the sequence header OBU the output type carries, raw or in
    /// an `AV1CodecConfigurationRecord`, if it carries one.
    fn output_type_sequence_header(&self) -> Option<Vec<u8>> {
        // SAFETY: the encoder's output type is set; the blob is copied.
        let blob = unsafe {
            let output = self.transform.GetOutputCurrentType(0).ok()?;
            let size = output.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).ok()?;
            let mut blob = vec![0; size as usize];
            output
                .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None)
                .ok()?;
            blob
        };
        let obus = match blob.first() {
            Some(first) if first & CONFIGURATION_RECORD_MARKER != 0 => {
                blob.get(CONFIGURATION_RECORD_HEADER..)?
            }
            _ => &blob[..],
        };
        parse_obus(obus)
            .ok()?
            .iter()
            .find(|obu| obu.obu_type == ObuType::SequenceHeader)
            .map(|obu| obu.bytes.to_vec())
    }

    /// Asks the encoder for the quantizer rate control asked for, from the
    /// next frame, if it changed or `always`; a quantizer the encoder does
    /// not take leaves the one before.
    fn apply_quantizer(&mut self, always: bool) {
        if !always && self.wanted_quantizer == self.quantizer {
            return;
        }
        let quantizer = u64::from(self.wanted_quantizer);
        // The default quantizer, then those of I, P and B frames.
        let value = quantizer | (quantizer << 16) | (quantizer << 32) | (quantizer << 48);
        if self.set(&CODECAPI_AVEncVideoEncodeQP, VARIANT::from(value)) {
            self.quantizer = self.wanted_quantizer;
        } else {
            debug!(encoder = self.description, "quantizer not taken");
            self.wanted_quantizer = self.quantizer;
        }
    }

    /// Returns the encoder's next event's type and status, waiting for it
    /// up to [`ENCODE_TIMEOUT`].
    fn next_event(&self) -> Result<(u32, windows::core::HRESULT), CodecError> {
        let started = Instant::now();
        loop {
            // SAFETY: the encoder's event queue is valid; the event is the
            // caller's.
            match unsafe { self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => {
                    // SAFETY: the event is valid.
                    let kind =
                        unsafe { event.GetType() }.map_err(|error| failed("event", &error))?;
                    // SAFETY: as above.
                    let status =
                        unsafe { event.GetStatus() }.map_err(|error| failed("event", &error))?;
                    return Ok((kind, status));
                }
                Err(error) if error.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    let waited = started.elapsed();
                    if waited > ENCODE_TIMEOUT {
                        return Err(CodecError::Encoder(format!(
                            "{} gave nothing for {} ms",
                            self.description,
                            waited.as_millis()
                        )));
                    }
                    if waited < SPIN {
                        thread::yield_now();
                    } else {
                        thread::sleep(POLL_INTERVAL);
                    }
                }
                Err(error) => return Err(failed("event", &error)),
            }
        }
    }

    /// Returns `frame` as an NV12 sample at frame `index`.
    fn sample(&self, frame: &Yuv420Frame, index: u64) -> Result<IMFSample, CodecError> {
        let (width, height) = (self.width, self.height);
        let (chroma_width, chroma_height) = (frame.chroma_width(), frame.chroma_height());
        let size = width * height + chroma_width * 2 * chroma_height;
        // SAFETY: the buffer is made here with `size` bytes, written while
        // locked; the sample is made here and given the buffer.
        unsafe {
            let buffer =
                MFCreateMemoryBuffer(size as u32).map_err(|error| failed("buffer", &error))?;
            let mut data = std::ptr::null_mut();
            buffer
                .Lock(&mut data, None, None)
                .map_err(|error| failed("buffer lock", &error))?;
            let pixels = std::slice::from_raw_parts_mut(data, size);
            let (luma, chroma) = pixels.split_at_mut(width * height);
            luma.copy_from_slice(frame.y());
            for ((pair, &u), &v) in chroma.chunks_exact_mut(2).zip(frame.u()).zip(frame.v()) {
                pair[0] = u;
                pair[1] = v;
            }
            buffer
                .Unlock()
                .and_then(|()| buffer.SetCurrentLength(size as u32))
                .map_err(|error| failed("buffer", &error))?;
            let sample = MFCreateSample().map_err(|error| failed("sample", &error))?;
            let duration = TICKS_PER_SECOND / i64::from(self.frame_rate);
            sample
                .AddBuffer(&buffer)
                .and_then(|()| sample.SetSampleTime(index as i64 * duration))
                .and_then(|()| sample.SetSampleDuration(duration))
                .map_err(|error| failed("sample", &error))?;
            Ok(sample)
        }
    }

    /// Encodes `frame`, frame `index`, as a key frame if `key`, returning
    /// the encoded picture and whether it is a key frame.
    fn encode_frame(
        &mut self,
        frame: &Yuv420Frame,
        index: u64,
        key: bool,
    ) -> Result<(Vec<u8>, bool), CodecError> {
        let sample = self.sample(frame, index)?;
        while self.wanted_frames == 0 {
            match self.next_event()? {
                (kind, _) if kind == METransformNeedInput.0 as u32 => self.wanted_frames += 1,
                (kind, status) if kind == MEError.0 as u32 => {
                    return Err(CodecError::Encoder(format!(
                        "{} failed: {status:?}",
                        self.description
                    )));
                }
                _ => {}
            }
        }
        if key && !self.set(&CODECAPI_AVEncVideoForceKeyFrame, VARIANT::from(1_u32)) {
            debug!(encoder = self.description, "key frame not forced");
        }
        // SAFETY: the encoder asked for a frame; the sample is valid.
        unsafe { self.transform.ProcessInput(0, &sample, 0) }
            .map_err(|error| failed("input", &error))?;
        self.wanted_frames -= 1;
        loop {
            match self.next_event()? {
                (kind, _) if kind == METransformNeedInput.0 as u32 => self.wanted_frames += 1,
                (kind, _) if kind == METransformHaveOutput.0 as u32 => {
                    if let Some(output) = self.take_output()? {
                        return Ok(output);
                    }
                }
                (kind, status) if kind == MEError.0 as u32 => {
                    return Err(CodecError::Encoder(format!(
                        "{} failed: {status:?}",
                        self.description
                    )));
                }
                _ => {}
            }
        }
    }

    /// Takes the encoder's output: the encoded picture and whether it is a
    /// key frame, or nothing if the encoder changed its output type, after
    /// which it says again that it has output.
    fn take_output(&mut self) -> Result<Option<(Vec<u8>, bool)>, CodecError> {
        // SAFETY: the samples are made here or given by the encoder, and each
        // taken over once; the buffer is read while locked.
        unsafe {
            let given = if self.provides_samples {
                None
            } else {
                let buffer = MFCreateMemoryBuffer(self.output_size.max(1))
                    .map_err(|error| failed("output buffer", &error))?;
                let sample = MFCreateSample().map_err(|error| failed("output sample", &error))?;
                sample
                    .AddBuffer(&buffer)
                    .map_err(|error| failed("output sample", &error))?;
                Some(sample)
            };
            let mut output = [MFT_OUTPUT_DATA_BUFFER {
                pSample: std::mem::ManuallyDrop::new(given),
                ..Default::default()
            }];
            let mut status = 0;
            let result = self.transform.ProcessOutput(0, &mut output, &mut status);
            let sample = std::mem::ManuallyDrop::take(&mut output[0].pSample);
            drop(std::mem::ManuallyDrop::take(&mut output[0].pEvents));
            match result {
                Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    let changed = self
                        .transform
                        .GetOutputAvailableType(0, 0)
                        .and_then(|output_type| self.transform.SetOutputType(0, &output_type, 0));
                    changed.map_err(|error| failed("output type change", &error))?;
                    return Ok(None);
                }
                Err(error) => return Err(failed("output", &error)),
                Ok(()) => {}
            }
            let sample =
                sample.ok_or_else(|| CodecError::Encoder("no output sample".to_owned()))?;
            let is_key_frame = sample.GetUINT32(&MFSampleExtension_CleanPoint) == Ok(1);
            let buffer = sample
                .ConvertToContiguousBuffer()
                .map_err(|error| failed("output buffer", &error))?;
            let mut data = std::ptr::null_mut();
            let mut length = 0;
            buffer
                .Lock(&mut data, None, Some(&mut length))
                .map_err(|error| failed("output lock", &error))?;
            let packet = std::slice::from_raw_parts(data, length as usize).to_vec();
            buffer
                .Unlock()
                .map_err(|error| failed("output unlock", &error))?;
            Ok(Some((packet, is_key_frame)))
        }
    }
}

impl VideoEncoder for MediaFoundationEncoder {
    fn encode(
        &mut self,
        frame: &Yuv420Frame,
        request: KeyFrameRequest,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        if frame.width() != self.width || frame.height() != self.height {
            return Err(CodecError::FrameSize);
        }
        join_com();
        let index = self.next_index;
        self.next_index += 1;
        self.apply_quantizer(false);
        // The first frame follows the stream check's, and a recovery frame
        // starts afresh.
        let key = index == 0 || request.restarts();
        let (packet, is_key_frame) = self.encode_frame(frame, index + 1, key)?;
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
