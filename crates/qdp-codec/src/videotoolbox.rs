//! Hardware AV1 decoding with VideoToolbox, on Macs whose GPU decodes AV1
//! (Apple M3 and later).
//!
//! VideoToolbox is a C interface, so this module uses `unsafe`: each block
//! states the contract it relies on, from Apple's documentation.

#![allow(unsafe_code)]

use std::ffi::c_void;
use std::ptr::{self, NonNull};

use objc2_core_foundation::{CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString};
use objc2_core_media::{
    CMBlockBuffer, CMFormatDescription, CMSampleBuffer, CMTime, CMVideoFormatDescriptionCreate,
    kCMBlockBufferAssureMemoryNowFlag,
    kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms, kCMVideoCodecType_AV1,
};
use objc2_core_video::{
    CVImageBuffer, CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeightOfPlane,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelBufferPixelFormatTypeKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, kCVReturnSuccess,
};
use objc2_video_toolbox::{
    VTDecodeFrameFlags, VTDecodeInfoFlags, VTDecompressionOutputCallbackRecord,
    VTDecompressionSession, VTIsHardwareDecodeSupported,
    VTRegisterSupplementalVideoDecoderIfAvailable,
    kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder,
};

use crate::{CodecError, SequenceHeader, VideoDecoder, Yuv420Frame};

/// How the decoder describes itself.
const DESCRIPTION: &str = "VideoToolbox (hardware)";

/// The sample description extension that carries the AV1 configuration record.
const AV1C_ATOM: &str = "av1C";

/// The status of a call that succeeded.
const NO_ERROR: i32 = 0;

/// The luma plane of a bi-planar pixel buffer.
const LUMA_PLANE: usize = 0;

/// The interleaved chroma plane of a bi-planar pixel buffer.
const CHROMA_PLANE: usize = 1;

/// Returns an error for a failed VideoToolbox or Core Media call.
fn check(status: i32, what: &'static str) -> Result<(), CodecError> {
    if status == NO_ERROR {
        Ok(())
    } else {
        Err(CodecError::Decoder { what, code: status })
    }
}

/// A hardware AV1 decoder using VideoToolbox.
///
/// The decompression session is created from the first sequence header and
/// recreated whenever the sequence header changes.
pub struct VideoToolboxDecoder {
    /// The session for the current sequence header, once one has arrived.
    session: Option<Session>,
}

// SAFETY: Core Foundation objects are reference counted atomically, and a
// VideoToolbox decompression session may be called from any thread, one call
// at a time, which `&mut self` on every method guarantees.
unsafe impl Send for VideoToolboxDecoder {}

impl std::fmt::Debug for VideoToolboxDecoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VideoToolboxDecoder")
            .finish_non_exhaustive()
    }
}

impl VideoToolboxDecoder {
    /// Returns a decoder if this Mac decodes AV1 in hardware.
    pub fn new() -> Result<Self, CodecError> {
        // SAFETY: both functions take a codec type and have no other contract.
        let supported = unsafe {
            VTRegisterSupplementalVideoDecoderIfAvailable(kCMVideoCodecType_AV1);
            VTIsHardwareDecodeSupported(kCMVideoCodecType_AV1)
        };
        if !supported {
            return Err(CodecError::HardwareUnavailable(
                "this Mac has no hardware AV1 decoder".to_owned(),
            ));
        }
        Ok(Self { session: None })
    }
}

impl VideoDecoder for VideoToolboxDecoder {
    fn decode(&mut self, temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
        if let Some(header) = SequenceHeader::find(temporal_unit)? {
            let current = self
                .session
                .as_ref()
                .is_some_and(|session| session.sequence_header == header.obu());
            if !current {
                self.session = None;
                self.session = Some(Session::new(&header)?);
            }
        }
        let session = self
            .session
            .as_ref()
            .ok_or(CodecError::MalformedBitstream("no sequence header yet"))?;
        session.decode(temporal_unit)
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }
}

/// A decompression session for one sequence header.
struct Session {
    /// The sequence header OBU the session was created for.
    sequence_header: Vec<u8>,
    /// The stream's format, attached to every sample.
    format: CFRetained<CMFormatDescription>,
    /// The session.
    decompression: CFRetained<VTDecompressionSession>,
}

impl Session {
    /// Creates a session for the stream `header` describes.
    fn new(header: &SequenceHeader) -> Result<Self, CodecError> {
        if header.bit_depth != 8
            || header.monochrome
            || !header.subsampling_x
            || !header.subsampling_y
        {
            return Err(CodecError::UnsupportedFormat);
        }
        let unavailable = |what: &str, status: i32| {
            CodecError::HardwareUnavailable(format!("{what} failed with status {status}"))
        };

        let record = CFData::from_bytes(&header.codec_configuration_record());
        let atom = CFString::from_static_str(AV1C_ATOM);
        let atoms = CFDictionary::from_slices(&[&*atom], &[&*record]);
        // SAFETY: the extension key is an immutable string constant.
        let atoms_key = unsafe { kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms };
        let atoms_value: &CFDictionary = (*atoms).as_ref();
        let extensions = CFDictionary::from_slices(&[atoms_key], &[atoms_value]);
        let mut format: *const CMFormatDescription = ptr::null();
        // SAFETY: the extensions are a dictionary of Core Foundation objects,
        // copied by the call, and `format` is valid for writing.
        let status = unsafe {
            CMVideoFormatDescriptionCreate(
                None,
                kCMVideoCodecType_AV1,
                header.max_width as i32,
                header.max_height as i32,
                Some((*extensions).as_ref()),
                NonNull::from(&mut format),
            )
        };
        if status != NO_ERROR {
            return Err(unavailable("CMVideoFormatDescriptionCreate", status));
        }
        let format = NonNull::new(format.cast_mut())
            .ok_or_else(|| unavailable("CMVideoFormatDescriptionCreate", status))?;
        // SAFETY: the format description was created above with a +1 count.
        let format = unsafe { CFRetained::from_raw(format) };

        // SAFETY: the specification key is an immutable string constant.
        let require_hardware =
            unsafe { kVTVideoDecoderSpecification_RequireHardwareAcceleratedVideoDecoder };
        let specification = CFDictionary::from_slices(&[require_hardware], &[CFBoolean::new(true)]);
        let pixel_format =
            CFNumber::new_i32(kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange as i32);
        // SAFETY: the attribute key is an immutable string constant.
        let pixel_format_key = unsafe { kCVPixelBufferPixelFormatTypeKey };
        let attributes = CFDictionary::from_slices(&[pixel_format_key], &[&*pixel_format]);
        let callback = VTDecompressionOutputCallbackRecord {
            decompressionOutputCallback: Some(output_callback),
            decompressionOutputRefCon: ptr::null_mut(),
        };
        let mut decompression: *mut VTDecompressionSession = ptr::null_mut();
        // SAFETY: the dictionaries hold Core Foundation objects, the callback
        // record is copied by the call, and `decompression` is valid for
        // writing.
        let status = unsafe {
            VTDecompressionSession::create(
                None,
                &format,
                Some((*specification).as_ref()),
                Some((*attributes).as_ref()),
                &callback,
                NonNull::from(&mut decompression),
            )
        };
        if status != NO_ERROR {
            return Err(unavailable("VTDecompressionSessionCreate", status));
        }
        let decompression = NonNull::new(decompression)
            .ok_or_else(|| unavailable("VTDecompressionSessionCreate", status))?;
        Ok(Self {
            sequence_header: header.obu().to_vec(),
            format,
            // SAFETY: the session was created above with a +1 count.
            decompression: unsafe { CFRetained::from_raw(decompression) },
        })
    }

    /// Decodes one temporal unit and returns its picture.
    fn decode(&self, temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
        let length = temporal_unit.len();
        let source = NonNull::new(temporal_unit.as_ptr().cast_mut())
            .filter(|_| length > 0)
            .ok_or(CodecError::MalformedBitstream("empty temporal unit"))?;
        let mut block: *mut CMBlockBuffer = ptr::null_mut();
        // SAFETY: a null memory block asks Core Media to allocate `length`
        // bytes, and `block` is valid for writing.
        let status = unsafe {
            CMBlockBuffer::create_with_memory_block(
                None,
                ptr::null_mut(),
                length,
                None,
                ptr::null(),
                0,
                length,
                kCMBlockBufferAssureMemoryNowFlag,
                NonNull::from(&mut block),
            )
        };
        check(status, "CMBlockBufferCreateWithMemoryBlock")?;
        let block = NonNull::new(block).ok_or(CodecError::Decoder {
            what: "CMBlockBufferCreateWithMemoryBlock",
            code: status,
        })?;
        // SAFETY: the block buffer was created above with a +1 count.
        let block = unsafe { CFRetained::from_raw(block) };
        // SAFETY: `source` points to `length` readable bytes, and the block
        // buffer holds `length` bytes.
        let status = unsafe {
            CMBlockBuffer::replace_data_bytes(source.cast::<c_void>(), &block, 0, length)
        };
        check(status, "CMBlockBufferReplaceDataBytes")?;

        let sizes = [length];
        let mut sample: *mut CMSampleBuffer = ptr::null_mut();
        // SAFETY: one sample of `length` bytes, without timing, in the block
        // buffer; `sample` is valid for writing.
        let status = unsafe {
            CMSampleBuffer::create_ready(
                None,
                Some(&block),
                Some(&self.format),
                1,
                0,
                ptr::null(),
                1,
                sizes.as_ptr(),
                NonNull::from(&mut sample),
            )
        };
        check(status, "CMSampleBufferCreateReady")?;
        let sample = NonNull::new(sample).ok_or(CodecError::Decoder {
            what: "CMSampleBufferCreateReady",
            code: status,
        })?;
        // SAFETY: the sample buffer was created above with a +1 count.
        let sample = unsafe { CFRetained::from_raw(sample) };

        let mut decoded = DecodedImage {
            status: NO_ERROR,
            image: None,
        };
        let mut info = VTDecodeInfoFlags(0);
        // SAFETY: `decoded` outlives the call and the wait below, after which
        // the callback no longer runs for this frame.
        let status = unsafe {
            self.decompression.decode_frame(
                &sample,
                VTDecodeFrameFlags(0),
                (&raw mut decoded).cast::<c_void>(),
                &mut info,
            )
        };
        // SAFETY: the session is valid; waiting delivers any pending frames.
        unsafe { self.decompression.wait_for_asynchronous_frames() };
        check(status, "VTDecompressionSessionDecodeFrame")?;
        check(decoded.status, "VTDecompressionOutputCallback")?;
        let image = decoded.image.ok_or(CodecError::NoPicture)?;
        copy_pixel_buffer(&image)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the session is valid and no decode is in progress.
        unsafe { self.decompression.invalidate() };
    }
}

/// What the output callback received for one frame.
struct DecodedImage {
    /// The decode status.
    status: i32,
    /// The decoded picture, if decoding succeeded.
    image: Option<CFRetained<CVPixelBuffer>>,
}

/// Receives a decoded frame from VideoToolbox.
///
/// # Safety
///
/// `frame_context` must be the [`DecodedImage`] passed to the decode call.
unsafe extern "C-unwind" fn output_callback(
    _session_context: *mut c_void,
    frame_context: *mut c_void,
    status: i32,
    _flags: VTDecodeInfoFlags,
    image: *mut CVImageBuffer,
    _presentation_time: CMTime,
    _duration: CMTime,
) {
    // SAFETY: `frame_context` is the decode call's `DecodedImage`, which no
    // one else uses while the decode is in progress.
    let decoded = unsafe { &mut *frame_context.cast::<DecodedImage>() };
    decoded.status = status;
    // SAFETY: a non-null image buffer is valid for the callback's duration;
    // retaining it keeps it valid afterwards.
    decoded.image = NonNull::new(image).map(|image| unsafe { CFRetained::retain(image) });
}

/// Copies a decoded NV12 pixel buffer into a planar frame.
fn copy_pixel_buffer(image: &CVPixelBuffer) -> Result<Yuv420Frame, CodecError> {
    if CVPixelBufferGetPixelFormatType(image) != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange {
        return Err(CodecError::UnsupportedFormat);
    }
    // SAFETY: the pixel buffer is valid; it is unlocked below.
    let status = unsafe { CVPixelBufferLockBaseAddress(image, CVPixelBufferLockFlags::ReadOnly) };
    if status != kCVReturnSuccess {
        return Err(CodecError::Decoder {
            what: "CVPixelBufferLockBaseAddress",
            code: status,
        });
    }
    let frame = read_planes(image);
    // SAFETY: the pixel buffer was locked above with the same flags.
    unsafe { CVPixelBufferUnlockBaseAddress(image, CVPixelBufferLockFlags::ReadOnly) };
    frame
}

/// Reads the planes of a locked NV12 pixel buffer.
fn read_planes(image: &CVPixelBuffer) -> Result<Yuv420Frame, CodecError> {
    let width = CVPixelBufferGetWidthOfPlane(image, LUMA_PLANE);
    let height = CVPixelBufferGetHeightOfPlane(image, LUMA_PLANE);
    let chroma_width = CVPixelBufferGetWidthOfPlane(image, CHROMA_PLANE);
    let chroma_height = CVPixelBufferGetHeightOfPlane(image, CHROMA_PLANE);
    let luma_stride = CVPixelBufferGetBytesPerRowOfPlane(image, LUMA_PLANE);
    let chroma_stride = CVPixelBufferGetBytesPerRowOfPlane(image, CHROMA_PLANE);
    let luma = CVPixelBufferGetBaseAddressOfPlane(image, LUMA_PLANE).cast::<u8>();
    let chroma = CVPixelBufferGetBaseAddressOfPlane(image, CHROMA_PLANE).cast::<u8>();
    if width == 0
        || height == 0
        || luma.is_null()
        || chroma.is_null()
        || chroma_width != width.div_ceil(2)
        || chroma_height != height.div_ceil(2)
        || luma_stride < width
        || chroma_stride < 2 * chroma_width
    {
        return Err(CodecError::UnsupportedFormat);
    }
    // SAFETY: the buffer is locked, and each plane holds its height in rows of
    // its stride, the last row at least as long as the plane's width.
    let (luma, chroma) = unsafe {
        (
            std::slice::from_raw_parts(luma, luma_stride * (height - 1) + width),
            std::slice::from_raw_parts(
                chroma,
                chroma_stride * (chroma_height - 1) + 2 * chroma_width,
            ),
        )
    };
    Yuv420Frame::from_nv12(width, height, (luma, luma_stride), (chroma, chroma_stride))
        .ok_or(CodecError::UnsupportedFormat)
}
