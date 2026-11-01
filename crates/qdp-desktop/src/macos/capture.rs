//! Screen capture on macOS with ScreenCaptureKit (macOS 12.3 and later).
//!
//! macOS lets an app capture the screen only once the user has allowed it
//! under System Settings, Privacy & Security, Screen Recording.

use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
use objc2_core_graphics::{CGDisplayCopyDisplayMode, CGDisplayMode, CGMainDisplayID};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow,
    CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
    kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamOutput, SCStreamOutputType,
};
use qdp_codec::{Rgb32Order, Yuv420Frame};
use qdp_runtime::{CursorState, FrameArrivals, FrameSource, RuntimeError};
use tracing::{debug, info};

use super::cursor::SystemCursor;
use crate::latest_frame::LatestFrame;

/// How long to wait for ScreenCaptureKit's answers and the first frame.
pub(super) const TIMEOUT: Duration = Duration::from_secs(10);

/// The frames per second asked for.
const FRAME_RATE: i32 = 60;

/// The frames ScreenCaptureKit keeps in flight.
const QUEUE_DEPTH: isize = 3;

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class has no
    // `Drop` implementation.
    #[unsafe(super(NSObject))]
    #[name = "QdpScreenOutput"]
    #[ivars = Arc<LatestFrame>]
    struct ScreenOutput;

    unsafe impl NSObjectProtocol for ScreenOutput {}

    unsafe impl SCStreamDelegate for ScreenOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            self.ivars()
                .fail(format!("the screen capture stopped: {}", error.localizedDescription()));
        }
    }


    unsafe impl SCStreamOutput for ScreenOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output_sample_buffer(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Screen {
                return;
            }
            // Frames without an image report that the screen is unchanged.
            // SAFETY: the sample buffer is valid for the call.
            let Some(image) = (unsafe { sample_buffer.image_buffer() }) else {
                return;
            };
            match convert(&image) {
                Some(frame) => self.ivars().store(frame),
                None => debug!("unreadable screen frame skipped"),
            }
        }
    }
);

impl ScreenOutput {
    /// Returns an output storing frames in `latest`.
    fn new(latest: Arc<LatestFrame>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(latest);
        // SAFETY: `init` is NSObject's designated initializer.
        unsafe { msg_send![super(this), init] }
    }
}

/// Converts a BGRA pixel buffer to a frame.
fn convert(image: &CVPixelBuffer) -> Option<Yuv420Frame> {
    // SAFETY: the pixel buffer is valid; it is unlocked below.
    if unsafe { CVPixelBufferLockBaseAddress(image, CVPixelBufferLockFlags::ReadOnly) }
        != kCVReturnSuccess
    {
        return None;
    }
    let width = CVPixelBufferGetWidth(image);
    let height = CVPixelBufferGetHeight(image);
    let stride = CVPixelBufferGetBytesPerRow(image);
    let base = CVPixelBufferGetBaseAddress(image).cast::<u8>();
    let frame = (!base.is_null() && height > 0).then(|| {
        // SAFETY: the locked buffer holds `height` rows of `stride` bytes.
        let data = unsafe { std::slice::from_raw_parts(base, stride * height) };
        Yuv420Frame::from_rgb32(width, height, stride, Rgb32Order::Bgr, data)
    });
    // SAFETY: the buffer was locked above with the same flags.
    unsafe { CVPixelBufferUnlockBaseAddress(image, CVPixelBufferLockFlags::ReadOnly) };
    frame.flatten()
}

/// Returns a completion handler that sends an error or success.
pub(super) fn completion(
    sender: mpsc::Sender<Result<(), String>>,
) -> RcBlock<dyn Fn(*mut NSError)> {
    RcBlock::new(move |error: *mut NSError| {
        // SAFETY: a non-null error is a valid NSError for the call.
        let result = match unsafe { error.as_ref() } {
            Some(error) => Err(error.localizedDescription().to_string()),
            None => Ok(()),
        };
        let _ = sender.send(result);
    })
}

/// Captures the main display.
pub struct MacScreenCapture {
    /// The display's size in pixels.
    size: (usize, usize),
    /// The newest frame.
    latest: Arc<LatestFrame>,
    /// The stream.
    stream: Retained<SCStream>,
    /// The output receiving frames and errors, kept alive with the stream.
    _output: Retained<ScreenOutput>,
    /// Reads the pointer, which the frames then leave out, if macOS reports
    /// it.
    cursor: SystemCursor,
}

// SAFETY: ScreenCaptureKit's stream may be controlled from any thread, and
// the output only stores frames in a thread-safe slot.
unsafe impl Send for MacScreenCapture {}

impl std::fmt::Debug for MacScreenCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MacScreenCapture")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

impl MacScreenCapture {
    /// Starts capturing the main display at its full pixel resolution.
    /// Blocks until the first frame arrives.
    pub fn open() -> Result<Self, String> {
        let content = shareable_content()?;
        let main = CGMainDisplayID();
        // SAFETY: the content's displays are valid while it is.
        let displays = unsafe { content.displays() };
        let display = displays
            .iter()
            .find(|display| unsafe { display.displayID() } == main)
            .or_else(|| displays.firstObject())
            .ok_or("no display can be captured")?;
        let mode = CGDisplayCopyDisplayMode(main);
        let width = CGDisplayMode::pixel_width(mode.as_deref());
        let height = CGDisplayMode::pixel_height(mode.as_deref());
        if width == 0 || height == 0 {
            return Err("the main display has no size".to_owned());
        }

        // SAFETY: the filter captures the display, excluding no windows.
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            )
        };
        // The client draws the pointer where macOS reports it; the frames
        // never show it (Section 9.1).
        let cursor = SystemCursor::new(width, height);
        // SAFETY: the configuration's setters take plain values.
        let configuration = unsafe {
            let configuration = SCStreamConfiguration::new();
            configuration.setWidth(width);
            configuration.setHeight(height);
            configuration.setPixelFormat(kCVPixelFormatType_32BGRA);
            configuration.setMinimumFrameInterval(CMTime::new(1, FRAME_RATE));
            configuration.setShowsCursor(false);
            configuration.setQueueDepth(QUEUE_DEPTH);
            configuration
        };
        let latest = Arc::new(LatestFrame::default());
        let output = ScreenOutput::new(Arc::clone(&latest));
        // SAFETY: the filter and configuration are valid, and the delegate
        // lives as long as the stream.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &configuration,
                Some(ProtocolObject::from_ref(&*output)),
            )
        };
        // SAFETY: the output implements SCStreamOutput and outlives the
        // stream's use of it; a null queue uses the stream's own.
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Screen,
                None,
            )
        }
        .map_err(|error| error.localizedDescription().to_string())?;
        let (started, started_receiver) = mpsc::channel();
        // SAFETY: the handler is called once with the start's outcome.
        unsafe { stream.startCaptureWithCompletionHandler(Some(&completion(started))) };
        started_receiver
            .recv_timeout(TIMEOUT)
            .map_err(|_| "the screen capture did not start".to_owned())?
            .map_err(|error| format!("cannot capture the screen: {error}"))?;

        let capture = Self {
            size: (width, height),
            latest,
            stream,
            _output: output,
            cursor,
        };
        capture.latest.wait(TIMEOUT)?;
        info!(width, height, "capturing the screen");
        Ok(capture)
    }
}

/// Returns the displays and windows that can be captured.
pub(super) fn shareable_content() -> Result<Retained<SCShareableContent>, String> {
    let (sender, receiver) = mpsc::channel();
    let handler = RcBlock::new(
        move |content: *mut SCShareableContent, error: *mut NSError| {
            let result = match NonNull::new(content) {
                // SAFETY: a non-null content is a valid object, retained here.
                Some(content) => unsafe { Retained::retain(content.as_ptr()) }
                    .ok_or_else(|| "no shareable content".to_owned()),
                // SAFETY: a non-null error is a valid NSError for the call.
                None => Err(unsafe { error.as_ref() }.map_or_else(
                    || "no shareable content".to_owned(),
                    |error| error.localizedDescription().to_string(),
                )),
            };
            let _ = sender.send(result);
        },
    );
    // SAFETY: the handler is called once with the content or an error.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    receiver.recv_timeout(TIMEOUT).map_err(|_| {
        "macOS did not list the displays; allow Screen Recording for this app".to_owned()
    })?
}

impl FrameSource for MacScreenCapture {
    fn size(&self) -> (usize, usize) {
        self.size
    }

    fn capture(&mut self) -> Result<Arc<Yuv420Frame>, RuntimeError> {
        let frame = self.latest.wait(TIMEOUT).map_err(RuntimeError::Capture)?;
        // A display that changes size sends frames of its new size, which
        // the host follows with a new stream.
        self.size = (frame.width(), frame.height());
        Ok(frame)
    }

    fn cursor(&mut self) -> Option<CursorState> {
        Some(self.cursor.state())
    }

    /// ScreenCaptureKit delivers a frame each time the display changes, so
    /// each is told as it arrives.
    fn watch_frames(&mut self, arrivals: Arc<FrameArrivals>) -> bool {
        self.latest.watch(arrivals)
    }
}

impl Drop for MacScreenCapture {
    fn drop(&mut self) {
        // SAFETY: stopping a running stream has no other requirement.
        unsafe { self.stream.stopCaptureWithCompletionHandler(None) };
    }
}
