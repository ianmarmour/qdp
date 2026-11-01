//! AV1 decoding with Android's MediaCodec, straight into the surface the app
//! shows the video in.
//!
//! The app hands its surface over with [`set_video_surface`] before a session
//! decodes. A decoder renders into it without a copy: a decoded picture is a
//! [`SurfacePicture`], an output buffer the decoder still holds, which the app
//! shows by rendering it, or drops to pass over. The NDK's MediaCodec is a C
//! interface, so this module uses `unsafe`.

#![allow(unsafe_code)]

use std::ffi::{CStr, c_char};
use std::ptr::{self, NonNull};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ndk::native_window::NativeWindow;
use ndk_sys as ffi;
use tracing::info;

use crate::backend::VideoDecoder;
use crate::{CodecError, DecodedPicture, SequenceHeader, Yuv420Frame};

/// The MIME type of AV1 video.
const AV1_MIME: &CStr = c"video/av01";

/// The format's MIME type key.
const KEY_MIME: &CStr = c"mime";

/// The format's width key.
const KEY_WIDTH: &CStr = c"width";

/// The format's height key.
const KEY_HEIGHT: &CStr = c"height";

/// The format's crop rectangle key: the part of the picture shown.
const KEY_CROP: &CStr = c"crop";

/// The format's key for the codec configuration record (`av1C`).
const KEY_CODEC_SPECIFIC_DATA: &CStr = c"csd-0";

/// The format's key asking a decoder to hand out each frame as soon as it
/// is decoded, rather than hold some back (Android 11 and later).
const KEY_LOW_LATENCY: &CStr = c"low-latency";

/// The format's key for the codec's priority: 0 is real time.
const KEY_PRIORITY: &CStr = c"priority";

/// The real-time priority.
const REAL_TIME_PRIORITY: i32 = 0;

/// How long to wait for the decoder to take a temporal unit.
const INPUT_TIMEOUT: Duration = Duration::from_millis(50);

/// How long to wait for the decoder to hand out the frame it was given. A
/// decoder that takes longer holds frames back, which a stream that shows
/// every frame as soon as it can cannot use.
const OUTPUT_TIMEOUT: Duration = Duration::from_millis(100);

/// How often to look for the decoder's output while waiting for it.
const OUTPUT_POLL: Duration = Duration::from_millis(5);

/// The microseconds between the timestamps given to frames, which only
/// order them.
const FRAME_TIMESTAMP_STEP: u64 = 1000;

/// The surface the app shows the video in, once it has one.
static VIDEO_SURFACE: Mutex<Option<NativeWindow>> = Mutex::new(None);

/// Sets the surface decoders render into, or `None` once the app no longer
/// shows one. A decoder opened later renders into the new surface.
pub fn set_video_surface(surface: Option<NativeWindow>) {
    *VIDEO_SURFACE.lock().unwrap_or_else(PoisonError::into_inner) = surface;
}

/// Returns the surface decoders render into, if the app shows one.
pub fn video_surface() -> Option<NativeWindow> {
    VIDEO_SURFACE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// A MediaCodec decoder, shared by the decoder and the pictures it hands out.
struct Codec {
    /// The codec.
    raw: NonNull<ffi::AMediaCodec>,
}

// SAFETY: a MediaCodec may be called from any thread; its calls take its
// own lock.
unsafe impl Send for Codec {}
// SAFETY: as for `Send`.
unsafe impl Sync for Codec {}

impl Codec {
    /// Releases output buffer `index`, rendering it into the surface if
    /// `render`.
    fn release(&self, index: usize, render: bool) -> Result<(), CodecError> {
        // SAFETY: the codec is live and the index is one it handed out.
        let status =
            unsafe { ffi::AMediaCodec_releaseOutputBuffer(self.raw.as_ptr(), index, render) };
        check(status, "releasing a decoded picture")
    }
}

impl Drop for Codec {
    fn drop(&mut self) {
        // SAFETY: the codec is live, and nothing uses it once the last
        // reference is dropped.
        unsafe {
            ffi::AMediaCodec_stop(self.raw.as_ptr());
            ffi::AMediaCodec_delete(self.raw.as_ptr());
        }
    }
}

/// A picture MediaCodec decoded, still in the output buffer it holds, which
/// is rendered into the video surface when shown, and released unshown when
/// dropped.
pub struct SurfacePicture {
    /// The decoder holding it.
    codec: Arc<Codec>,
    /// Its output buffer.
    index: usize,
    /// The width and height shown, in pixels.
    visible: (usize, usize),
    /// Whether it was rendered, and so released.
    rendered: bool,
}

impl std::fmt::Debug for SurfacePicture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SurfacePicture")
            .field("index", &self.index)
            .field("visible", &self.visible)
            .finish_non_exhaustive()
    }
}

impl SurfacePicture {
    /// Returns the width and height shown, in pixels.
    pub fn visible_size(&self) -> (usize, usize) {
        self.visible
    }

    /// Returns the picture cut to at most `width × height` pixels.
    pub fn cropped(mut self, width: usize, height: usize) -> Self {
        self.visible = (self.visible.0.min(width), self.visible.1.min(height));
        self
    }

    /// Renders the picture into the video surface, which shows it at its
    /// next refresh.
    pub fn render(mut self) -> Result<(), CodecError> {
        self.rendered = true;
        self.codec.release(self.index, true)
    }
}

impl Drop for SurfacePicture {
    fn drop(&mut self) {
        if !self.rendered {
            let _ = self.codec.release(self.index, false);
        }
    }
}

/// Decodes AV1 with MediaCodec into the video surface.
pub struct MediaCodecDecoder {
    /// The decoder for the current sequence header, once one arrived.
    codec: Option<Arc<Codec>>,
    /// The sequence header OBU the decoder was opened for.
    sequence_header: Vec<u8>,
    /// The width and height of the pictures, as the decoder last said.
    visible: (usize, usize),
    /// The timestamp of the next temporal unit.
    timestamp: u64,
    /// Which decoder this is, for logs.
    description: String,
}

impl std::fmt::Debug for MediaCodecDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MediaCodecDecoder")
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

impl MediaCodecDecoder {
    /// Returns a decoder if the app shows a video surface to decode into.
    /// The device's decoder is opened once the first sequence header says
    /// what it decodes.
    pub fn new() -> Result<Self, CodecError> {
        if video_surface().is_none() {
            return Err(CodecError::HardwareUnavailable(
                "the app shows no surface to decode into".to_owned(),
            ));
        }
        Ok(Self {
            codec: None,
            sequence_header: Vec::new(),
            visible: (0, 0),
            timestamp: 0,
            description: "MediaCodec".to_owned(),
        })
    }

    /// Opens the device's AV1 decoder for `header`, rendering into the video
    /// surface.
    fn open(&mut self, header: &SequenceHeader) -> Result<(), CodecError> {
        self.codec = None;
        let surface = video_surface().ok_or_else(|| {
            CodecError::HardwareUnavailable("the app shows no surface to decode into".to_owned())
        })?;
        // SAFETY: the MIME type is a valid C string.
        let raw = NonNull::new(unsafe { ffi::AMediaCodec_createDecoderByType(AV1_MIME.as_ptr()) })
            .ok_or_else(|| {
                CodecError::HardwareUnavailable("this device has no AV1 decoder".to_owned())
            })?;
        let codec = Codec { raw };
        let format = Format::new()?;
        let width = i32::try_from(header.max_width).map_err(|_| CodecError::FrameSize)?;
        let height = i32::try_from(header.max_height).map_err(|_| CodecError::FrameSize)?;
        format.set_string(KEY_MIME, AV1_MIME);
        format.set_i32(KEY_WIDTH, width);
        format.set_i32(KEY_HEIGHT, height);
        format.set_i32(KEY_LOW_LATENCY, 1);
        format.set_i32(KEY_PRIORITY, REAL_TIME_PRIORITY);
        format.set_buffer(
            KEY_CODEC_SPECIFIC_DATA,
            &header.codec_configuration_record(),
        );
        // SAFETY: the codec, format and surface are live; there is no crypto.
        let status = unsafe {
            ffi::AMediaCodec_configure(
                codec.raw.as_ptr(),
                format.raw.as_ptr(),
                surface.ptr().as_ptr(),
                ptr::null_mut(),
                0,
            )
        };
        check(status, "configuring the AV1 decoder")?;
        // SAFETY: the codec is configured.
        check(
            unsafe { ffi::AMediaCodec_start(codec.raw.as_ptr()) },
            "starting the AV1 decoder",
        )?;
        self.description = format!("MediaCodec {}", codec_name(&codec));
        info!(
            decoder = self.description,
            width, height, "decoding AV1 with MediaCodec"
        );
        self.visible = (header.max_width as usize, header.max_height as usize);
        self.sequence_header = header.obu().to_vec();
        self.codec = Some(Arc::new(codec));
        Ok(())
    }

    /// Hands `temporal_unit` to `codec`.
    fn queue(&mut self, codec: &Codec, temporal_unit: &[u8]) -> Result<(), CodecError> {
        let timeout = i64::try_from(INPUT_TIMEOUT.as_micros()).unwrap_or(i64::MAX);
        // SAFETY: the codec is started.
        let index = unsafe { ffi::AMediaCodec_dequeueInputBuffer(codec.raw.as_ptr(), timeout) };
        let index = usize::try_from(index).map_err(|_| {
            CodecError::HardwareUnavailable("the AV1 decoder takes no more input".to_owned())
        })?;
        let mut capacity = 0usize;
        // SAFETY: the index is the input buffer the codec just handed out.
        let buffer =
            unsafe { ffi::AMediaCodec_getInputBuffer(codec.raw.as_ptr(), index, &mut capacity) };
        if buffer.is_null() || capacity < temporal_unit.len() {
            return Err(CodecError::HardwareUnavailable(format!(
                "the AV1 decoder's input buffer holds {capacity} bytes, not {}",
                temporal_unit.len()
            )));
        }
        // SAFETY: the buffer holds `capacity` bytes, at least the temporal
        // unit's, and nothing else writes to it until it is queued.
        unsafe { ptr::copy_nonoverlapping(temporal_unit.as_ptr(), buffer, temporal_unit.len()) };
        // SAFETY: the index is the input buffer just filled.
        let status = unsafe {
            ffi::AMediaCodec_queueInputBuffer(
                codec.raw.as_ptr(),
                index,
                0,
                temporal_unit.len(),
                self.timestamp,
                0,
            )
        };
        self.timestamp += FRAME_TIMESTAMP_STEP;
        check(status, "handing a frame to the AV1 decoder")
    }

    /// Waits for `codec` to hand out the picture it was last given.
    fn picture(&mut self, codec: &Arc<Codec>) -> Result<SurfacePicture, CodecError> {
        let deadline = Instant::now() + OUTPUT_TIMEOUT;
        let poll = i64::try_from(OUTPUT_POLL.as_micros()).unwrap_or(i64::MAX);
        loop {
            let mut info = ffi::AMediaCodecBufferInfo {
                offset: 0,
                size: 0,
                presentationTimeUs: 0,
                flags: 0,
            };
            // SAFETY: the codec is started, and `info` is written by the call.
            let index = unsafe {
                ffi::AMediaCodec_dequeueOutputBuffer(codec.raw.as_ptr(), &mut info, poll)
            };
            match index {
                index if index >= 0 => {
                    return Ok(SurfacePicture {
                        codec: Arc::clone(codec),
                        index: index as usize,
                        visible: self.visible,
                        rendered: false,
                    });
                }
                index if index == ffi::AMEDIACODEC_INFO_OUTPUT_FORMAT_CHANGED as isize => {
                    if let Some(visible) = output_size(codec) {
                        self.visible = visible;
                    }
                }
                index
                    if index == ffi::AMEDIACODEC_INFO_TRY_AGAIN_LATER as isize
                        || index == ffi::AMEDIACODEC_INFO_OUTPUT_BUFFERS_CHANGED as isize => {}
                status => {
                    return Err(CodecError::HardwareUnavailable(format!(
                        "the AV1 decoder failed with status {status}"
                    )));
                }
            }
            if Instant::now() >= deadline {
                return Err(CodecError::HardwareUnavailable(
                    "the AV1 decoder holds frames back".to_owned(),
                ));
            }
        }
    }
}

impl VideoDecoder for MediaCodecDecoder {
    fn decode(&mut self, _temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
        Err(CodecError::HardwareUnavailable(
            "MediaCodec decodes into the video surface, not into memory".to_owned(),
        ))
    }

    fn decode_picture(&mut self, temporal_unit: &[u8]) -> Result<DecodedPicture, CodecError> {
        if let Some(header) = SequenceHeader::find(temporal_unit)? {
            if self.codec.is_none() || self.sequence_header != header.obu() {
                self.open(&header)?;
            }
        }
        let codec = self
            .codec
            .clone()
            .ok_or(CodecError::MalformedBitstream("no sequence header yet"))?;
        self.queue(&codec, temporal_unit)?;
        Ok(DecodedPicture::Surface(self.picture(&codec)?))
    }

    fn description(&self) -> &str {
        &self.description
    }
}

/// A MediaCodec format.
struct Format {
    /// The format.
    raw: NonNull<ffi::AMediaFormat>,
}

impl Format {
    /// Returns an empty format.
    fn new() -> Result<Self, CodecError> {
        // SAFETY: the call has no preconditions.
        NonNull::new(unsafe { ffi::AMediaFormat_new() })
            .map(|raw| Self { raw })
            .ok_or_else(|| CodecError::HardwareUnavailable("cannot make a media format".to_owned()))
    }

    /// Sets `key` to `value`.
    fn set_string(&self, key: &CStr, value: &CStr) {
        // SAFETY: the format is live and both strings are C strings.
        unsafe { ffi::AMediaFormat_setString(self.raw.as_ptr(), key.as_ptr(), value.as_ptr()) };
    }

    /// Sets `key` to `value`.
    fn set_i32(&self, key: &CStr, value: i32) {
        // SAFETY: the format is live and the key a C string.
        unsafe { ffi::AMediaFormat_setInt32(self.raw.as_ptr(), key.as_ptr(), value) };
    }

    /// Sets `key` to a copy of `value`.
    fn set_buffer(&self, key: &CStr, value: &[u8]) {
        // SAFETY: the format is live, the key a C string, and the call copies
        // the bytes.
        unsafe {
            ffi::AMediaFormat_setBuffer(
                self.raw.as_ptr(),
                key.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
            );
        };
    }
}

impl Drop for Format {
    fn drop(&mut self) {
        // SAFETY: the format is live and no longer used.
        unsafe { ffi::AMediaFormat_delete(self.raw.as_ptr()) };
    }
}

/// Returns the size of the pictures `codec` hands out: its crop rectangle,
/// or its width and height.
fn output_size(codec: &Codec) -> Option<(usize, usize)> {
    // SAFETY: the codec is started; the format it returns is ours to delete.
    let format = Format {
        raw: NonNull::new(unsafe { ffi::AMediaCodec_getOutputFormat(codec.raw.as_ptr()) })?,
    };
    let (mut left, mut top, mut right, mut bottom) = (0, 0, 0, 0);
    // SAFETY: the format is live and the outputs are written by the call.
    let cropped = unsafe {
        ffi::AMediaFormat_getRect(
            format.raw.as_ptr(),
            KEY_CROP.as_ptr(),
            &mut left,
            &mut top,
            &mut right,
            &mut bottom,
        )
    };
    if cropped {
        let width = usize::try_from(right - left + 1).ok()?;
        let height = usize::try_from(bottom - top + 1).ok()?;
        return Some((width, height));
    }
    let (mut width, mut height) = (0, 0);
    // SAFETY: as above.
    let sized = unsafe {
        ffi::AMediaFormat_getInt32(format.raw.as_ptr(), KEY_WIDTH.as_ptr(), &mut width)
            && ffi::AMediaFormat_getInt32(format.raw.as_ptr(), KEY_HEIGHT.as_ptr(), &mut height)
    };
    if !sized {
        return None;
    }
    Some((usize::try_from(width).ok()?, usize::try_from(height).ok()?))
}

/// Returns the device's name for `codec`, such as `c2.qti.av1.decoder`.
fn codec_name(codec: &Codec) -> String {
    let mut name: *mut c_char = ptr::null_mut();
    // SAFETY: the codec is live; the name it writes is released below.
    let status = unsafe { ffi::AMediaCodec_getName(codec.raw.as_ptr(), &mut name) };
    if status != ffi::media_status_t::AMEDIA_OK || name.is_null() {
        return "unnamed".to_owned();
    }
    // SAFETY: the codec wrote a C string, which stays valid until released.
    let owned = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: the name is the one the codec handed out.
    unsafe { ffi::AMediaCodec_releaseName(codec.raw.as_ptr(), name) };
    owned
}

/// Returns an error saying `what` failed, unless `status` is success.
fn check(status: ffi::media_status_t, what: &str) -> Result<(), CodecError> {
    if status == ffi::media_status_t::AMEDIA_OK {
        Ok(())
    } else {
        Err(CodecError::HardwareUnavailable(format!(
            "{what} failed with status {}",
            status.0
        )))
    }
}
