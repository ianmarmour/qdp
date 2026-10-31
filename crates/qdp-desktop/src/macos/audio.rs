//! The desktop's sound on macOS, captured with ScreenCaptureKit (macOS 13 and
//! later), which records what all applications but this one play.
//!
//! macOS lets an app capture audio this way only once the user has allowed
//! Screen Recording for it.

use std::ptr::{self, NonNull};
use std::slice;
use std::sync::mpsc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved,
};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::CGMainDisplayID;
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer, CMTime,
    kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
};
use objc2_foundation::{NSArray, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType,
};
use qdp_runtime::{ActiveDevice, AudioInput, CaptureSink, SAMPLE_RATE};
use tracing::debug;

use super::capture::{TIMEOUT, completion, shareable_content};

/// The channels the desktop's sound is captured in.
const CHANNELS: u16 = 2;

/// The bits of a float sample.
const FLOAT_BITS: u32 = 32;

/// The side, in pixels, of the video ScreenCaptureKit must also produce,
/// which is discarded.
const VIDEO_SIDE: usize = 2;

/// The video frames per second ScreenCaptureKit must also produce.
const VIDEO_FRAME_RATE: i32 = 1;

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class has no
    // `Drop` implementation.
    #[unsafe(super(NSObject))]
    #[name = "QdpAudioOutput"]
    #[ivars = CaptureSink]
    struct SoundOutput;

    unsafe impl NSObjectProtocol for SoundOutput {}

    unsafe impl SCStreamOutput for SoundOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Audio {
                return;
            }
            match interleaved_samples(sample_buffer) {
                Some(samples) => self.ivars().push(&samples),
                None => debug!("unreadable audio buffer skipped"),
            }
        }
    }
);

impl SoundOutput {
    /// Returns an output delivering samples to `capture`.
    fn new(capture: CaptureSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(capture);
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Returns a sample buffer's 32-bit float samples, interleaved.
fn interleaved_samples(sample_buffer: &CMSampleBuffer) -> Option<Vec<f32>> {
    // SAFETY: the sample buffer is valid for the call.
    let description = unsafe { sample_buffer.format_description() }?;
    // SAFETY: the description is an audio one, valid while it is held.
    let format =
        unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&description).as_ref() }?;
    if format.mFormatFlags & kAudioFormatFlagIsFloat == 0 || format.mBitsPerChannel != FLOAT_BITS {
        return None;
    }
    let channels = format.mChannelsPerFrame as usize;
    let mut needed = 0;
    // SAFETY: a null list asks only for the size it needs.
    unsafe {
        sample_buffer.audio_buffer_list_with_retained_block_buffer(
            &mut needed,
            ptr::null_mut(),
            0,
            None,
            None,
            0,
            ptr::null_mut(),
        )
    };
    // The list holds a variable number of buffers; u64 storage aligns it.
    let mut storage = vec![0_u64; needed.div_ceil(size_of::<u64>()).max(1)];
    let list = storage.as_mut_ptr().cast::<AudioBufferList>();
    let mut block: *mut CMBlockBuffer = ptr::null_mut();
    // SAFETY: the storage holds `needed` bytes, aligned for the list; the
    // block buffer that backs the data is retained into `block`.
    let status = unsafe {
        sample_buffer.audio_buffer_list_with_retained_block_buffer(
            ptr::null_mut(),
            list,
            needed,
            None,
            None,
            kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
            &mut block,
        )
    };
    // SAFETY: a non-null block buffer was retained for this function.
    let _block = NonNull::new(block).map(|block| unsafe { CFRetained::from_raw(block) });
    if status != 0 {
        return None;
    }
    // SAFETY: the list was filled with `mNumberBuffers` buffers, whose data
    // the retained block buffer keeps alive.
    let buffers: &[AudioBuffer] = unsafe {
        let list = &*list;
        slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize)
    };
    let channel_data = |buffer: &AudioBuffer| -> &[f32] {
        if buffer.mData.is_null() {
            return &[];
        }
        // SAFETY: the buffer holds `mDataByteSize` bytes of float samples.
        unsafe {
            slice::from_raw_parts(
                buffer.mData.cast::<f32>(),
                buffer.mDataByteSize as usize / size_of::<f32>(),
            )
        }
    };
    if format.mFormatFlags & kAudioFormatFlagIsNonInterleaved == 0 {
        return buffers.first().map(|buffer| channel_data(buffer).to_vec());
    }
    let planes: Vec<&[f32]> = buffers.iter().take(channels).map(channel_data).collect();
    let frames = planes.iter().map(|plane| plane.len()).min().unwrap_or(0);
    Some(
        (0..frames)
            .flat_map(|frame| planes.iter().map(move |plane| plane[frame]))
            .collect(),
    )
}

/// A running capture, stopped when dropped.
struct SoundCapture {
    /// The stream.
    stream: Retained<SCStream>,
    /// The output receiving the audio, kept alive with the stream.
    _output: Retained<SoundOutput>,
}

impl Drop for SoundCapture {
    fn drop(&mut self) {
        // SAFETY: stopping a running stream has no other requirement.
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
    }
}

/// Captures the desktop's sound with ScreenCaptureKit.
#[derive(Debug, Default)]
pub struct MacDesktopAudio;

impl AudioInput for MacDesktopAudio {
    fn channels(&self) -> u16 {
        CHANNELS
    }

    fn start(&mut self, capture: CaptureSink) -> Result<ActiveDevice, String> {
        let content = shareable_content()?;
        let main = CGMainDisplayID();
        // SAFETY: the content's displays are valid while it is.
        let displays = unsafe { content.displays() };
        let display = displays
            .iter()
            .find(|display| unsafe { display.displayID() } == main)
            .or_else(|| displays.firstObject())
            .ok_or("no display can be captured")?;
        // SAFETY: the filter captures the display, excluding no windows.
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            )
        };
        // SAFETY: the configuration's setters take plain values.
        let configuration = unsafe {
            let configuration = SCStreamConfiguration::new();
            configuration.setWidth(VIDEO_SIDE);
            configuration.setHeight(VIDEO_SIDE);
            configuration.setMinimumFrameInterval(CMTime::new(1, VIDEO_FRAME_RATE));
            configuration.setCapturesAudio(true);
            configuration.setSampleRate(SAMPLE_RATE as isize);
            configuration.setChannelCount(CHANNELS as isize);
            configuration.setExcludesCurrentProcessAudio(true);
            configuration
        };
        let output = SoundOutput::new(capture);
        // SAFETY: the filter and configuration are valid.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &configuration,
                None,
            )
        };
        // SAFETY: the output implements SCStreamOutput and lives as long as
        // the stream's use of it; a null queue uses the stream's own.
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Audio,
                None,
            )
        }
        .map_err(|error| error.localizedDescription().to_string())?;
        let (started, started_receiver) = mpsc::channel();
        // SAFETY: the handler is called once with the start's outcome.
        unsafe { stream.startCaptureWithCompletionHandler(Some(&completion(started))) };
        started_receiver
            .recv_timeout(TIMEOUT)
            .map_err(|_| "the audio capture did not start".to_owned())?
            .map_err(|error| format!("cannot capture the desktop's sound: {error}"))?;
        Ok(ActiveDevice(Box::new(SoundCapture {
            stream,
            _output: output,
        })))
    }

    fn description(&self) -> &str {
        "ScreenCaptureKit desktop audio"
    }
}
