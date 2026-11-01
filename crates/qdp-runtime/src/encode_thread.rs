//! Capture and encoding on a dedicated thread, so that the host's network loop
//! keeps answering feedback, retransmission requests and clock pings while a
//! frame is being encoded. Captured frames are scaled to the size of the
//! stream they are encoded for.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use qdp::message::{BlockGrid, BlockSet, StreamId};
use qdp_codec::{
    CodecPreference, EncodedFrame, EncoderSettings, KeyFrameRequest, VideoEncoder, Yuv420Frame,
    open_encoder,
};
use tokio::sync::{Notify, oneshot};
use tracing::info;

use crate::RuntimeError;
use crate::media::{CursorState, FrameSource, MonitorPlacement, WantedDisplay, changed_blocks};

/// The name of the encoder thread, as debuggers and profilers show it.
const THREAD_NAME: &str = "qdp-encoder";

/// A request to capture and encode one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EncodeJob {
    /// Whether the frame must be a key frame.
    pub(crate) request: KeyFrameRequest,
    /// Whether the frame answers a loss, so that it is flagged `RECOVERY`.
    pub(crate) recovery: bool,
    /// The stream the frame is for.
    pub(crate) stream_id: StreamId,
    /// The stream's encoder settings, whose size the captured frame is
    /// scaled to. A job with other settings than the last reopens the
    /// encoder with them.
    pub(crate) settings: EncoderSettings,
    /// The quantizer rate control chose for the frame (Section 5.5).
    pub(crate) quantizer: u8,
    /// The display the client asks the captured monitor to match, which
    /// the thread asks the source for whenever it changes.
    pub(crate) display: Option<WantedDisplay>,
}

/// One frame the encoder produced.
#[derive(Debug, Clone)]
pub(crate) struct EncodedOutput {
    /// The job that produced it.
    pub(crate) job: EncodeJob,
    /// The encoded frame.
    pub(crate) frame: EncodedFrame,
    /// The blocks that changed since the previous frame: all of them for a key
    /// frame, or when the change is not known.
    pub(crate) blocks: BlockSet,
    /// When the source frame was captured.
    pub(crate) captured_at: Instant,
    /// How long capturing the frame and scaling it to the stream's size
    /// took, before it was encoded.
    pub(crate) scale_time: Duration,
    /// The size of the captured frame, before it was scaled.
    pub(crate) capture_size: (usize, usize),
    /// The pointer when the frame was captured, if the source reports it
    /// apart from the frames.
    pub(crate) cursor: Option<CursorState>,
    /// Where the captured monitor was in the desktop, if the source knows.
    pub(crate) placement: Option<MonitorPlacement>,
}

/// A thread that owns the capture source and the encoder.
pub(crate) struct EncoderThread {
    /// Sends jobs to the thread; dropping it stops the thread.
    jobs: Option<Sender<EncodeJob>>,
    /// Receives the thread's results.
    results: Receiver<Result<EncodedOutput, RuntimeError>>,
    /// Notified whenever a result is ready.
    ready: Arc<Notify>,
    /// The thread, joined when this is dropped.
    handle: Option<JoinHandle<()>>,
}

impl EncoderThread {
    /// Starts the thread with a source, and an encoder opened as `preference`
    /// asks with `settings`. Returns the thread and the encoder's
    /// description.
    ///
    /// Encoders are opened on the thread itself, because hardware encoders
    /// hold driver state that must stay on the thread that created it.
    pub(crate) async fn spawn(
        source: Box<dyn FrameSource>,
        matched: Option<WantedDisplay>,
        settings: EncoderSettings,
        preference: CodecPreference,
    ) -> Result<(Self, String), RuntimeError> {
        let (job_sender, jobs) = mpsc::channel();
        let (result_sender, results) = mpsc::channel();
        let (opened_sender, opened) = oneshot::channel();
        let ready = Arc::new(Notify::new());
        let notify = Arc::clone(&ready);
        let handle = std::thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                let encoder = match open_encoder(settings, preference) {
                    Ok(encoder) => encoder,
                    Err(error) => {
                        let _ = opened_sender.send(Err(error));
                        return;
                    }
                };
                let _ = opened_sender.send(Ok(encoder.description().to_owned()));
                let opened = OpenEncoder { encoder, settings };
                let source = MatchingSource { source, matched };
                run(source, opened, preference, &jobs, &result_sender, &notify);
            })
            .map_err(|error| RuntimeError::Capture(error.to_string()))?;
        let thread = Self {
            jobs: Some(job_sender),
            results,
            ready,
            handle: Some(handle),
        };
        let description = opened
            .await
            .map_err(|_| RuntimeError::Capture("the encoder thread stopped".to_owned()))??;
        Ok((thread, description))
    }

    /// Asks the thread to capture and encode a frame.
    pub(crate) fn request(&self, job: EncodeJob) -> Result<(), RuntimeError> {
        self.jobs
            .as_ref()
            .and_then(|jobs| jobs.send(job).ok())
            .ok_or_else(|| RuntimeError::Capture("the encoder thread stopped".to_owned()))
    }

    /// Returns a finished result, if one is waiting.
    pub(crate) fn try_result(&self) -> Option<Result<EncodedOutput, RuntimeError>> {
        match self.results.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(RuntimeError::Capture(
                "the encoder thread stopped".to_owned(),
            ))),
        }
    }

    /// Returns the signal notified whenever a result is ready.
    pub(crate) fn ready(&self) -> &Notify {
        &self.ready
    }
}

impl Drop for EncoderThread {
    fn drop(&mut self) {
        // Closing the job channel ends the thread's loop.
        self.jobs = None;
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// An encoder and the settings it was opened with.
struct OpenEncoder {
    /// The encoder.
    encoder: Box<dyn VideoEncoder>,
    /// Its settings.
    settings: EncoderSettings,
}

/// A source, and the display it was last asked to match.
struct MatchingSource {
    /// The source.
    source: Box<dyn FrameSource>,
    /// The display it was last asked to match, whether or not it could.
    matched: Option<WantedDisplay>,
}

impl MatchingSource {
    /// Asks the source to match `wanted`, if it was not asked already.
    fn follow(&mut self, wanted: Option<WantedDisplay>) {
        if wanted == self.matched {
            return;
        }
        self.matched = wanted;
        if let Some(wanted) = wanted {
            if self.source.match_display(&wanted) {
                info!(
                    width = wanted.width,
                    height = wanted.height,
                    "the captured monitor matches the client's display"
                );
            }
        }
    }
}

/// The thread's loop: one result for each job, until the job channel closes or
/// the result channel is dropped.
fn run(
    mut source: MatchingSource,
    mut opened: OpenEncoder,
    preference: CodecPreference,
    jobs: &Receiver<EncodeJob>,
    results: &Sender<Result<EncodedOutput, RuntimeError>>,
    ready: &Notify,
) {
    let mut stream = None;
    let mut previous: Option<Arc<Yuv420Frame>> = None;
    let mut last_capture = None;
    let mut next_input_index = 0;
    while let Ok(job) = jobs.recv() {
        source.follow(job.display);
        if stream != Some(job.stream_id) {
            // A new stream starts with a key frame, which needs no earlier
            // frame to compare against.
            stream = Some(job.stream_id);
            previous = None;
        }
        let grid = BlockGrid::for_monitor(job.settings.width as u32, job.settings.height as u32);
        let captured_at = Instant::now();
        let outputs = encode_one(
            &mut *source.source,
            &mut opened,
            preference,
            job,
            &mut last_capture,
        )
        .map(|captured| {
            let input_index = next_input_index;
            next_input_index += 1;
            let changed = previous
                .as_ref()
                .map(|previous| changed_blocks(previous, &captured.frame, grid));
            previous = Some(captured.frame);
            captured
                .encoded
                .into_iter()
                .map(|frame| {
                    // The encoder returns each frame immediately; if it ever
                    // returned an earlier one, marking every block changed is
                    // always correct.
                    let blocks = match &changed {
                        Some(changed)
                            if !frame.is_key_frame && frame.input_index == input_index =>
                        {
                            changed.clone()
                        }
                        _ => BlockSet::full(grid.len()),
                    };
                    EncodedOutput {
                        job,
                        frame,
                        blocks,
                        captured_at,
                        scale_time: captured.scale_time,
                        capture_size: captured.capture_size,
                        cursor: captured.cursor.clone(),
                        placement: captured.placement,
                    }
                })
                .collect::<Vec<_>>()
        });
        let delivered = match outputs {
            Ok(outputs) => outputs
                .into_iter()
                .all(|output| results.send(Ok(output)).is_ok()),
            Err(error) => results.send(Err(error)).is_ok(),
        };
        ready.notify_one();
        if !delivered {
            return;
        }
    }
}

/// The output of [`encode_one`].
struct Encoded {
    /// The frame as encoded, after scaling.
    frame: Arc<Yuv420Frame>,
    /// The size it was captured at.
    capture_size: (usize, usize),
    /// The pointer, if the source reports it.
    cursor: Option<CursorState>,
    /// Where the captured monitor is in the desktop, if the source knows.
    placement: Option<MonitorPlacement>,
    /// How long capturing and scaling took.
    scale_time: Duration,
    /// The encoded frames.
    encoded: Vec<EncodedFrame>,
}

/// The last frame captured, and the frame it was scaled to for encoding.
type LastCapture = Option<(Arc<Yuv420Frame>, Arc<Yuv420Frame>)>;

/// Captures one frame, with the pointer if the source reports it, scales it
/// to the job's size and encodes it, first reopening the encoder if the job
/// has other settings. A capture that is `last`'s again, as a source holding
/// an unchanged screen returns, is not scaled again.
fn encode_one(
    source: &mut dyn FrameSource,
    opened: &mut OpenEncoder,
    preference: CodecPreference,
    job: EncodeJob,
    last: &mut LastCapture,
) -> Result<Encoded, RuntimeError> {
    if opened.settings != job.settings {
        let encoder = open_encoder(job.settings, preference)?;
        info!(
            encoder = encoder.description(),
            width = job.settings.width,
            height = job.settings.height,
            "encoder reopened"
        );
        *opened = OpenEncoder {
            encoder,
            settings: job.settings,
        };
    }
    opened.encoder.set_quantizer(job.quantizer);
    let capture_started_at = Instant::now();
    let captured = source.capture()?;
    let capture_size = (captured.width(), captured.height());
    let size = (job.settings.width, job.settings.height);
    let frame = match last {
        Some((last_captured, scaled))
            if Arc::ptr_eq(last_captured, &captured)
                && (scaled.width(), scaled.height()) == size =>
        {
            Arc::clone(scaled)
        }
        _ => {
            let scaled = if capture_size == size {
                Arc::clone(&captured)
            } else {
                Arc::new(captured.to_scaled(size.0, size.1))
            };
            *last = Some((captured, Arc::clone(&scaled)));
            scaled
        }
    };
    let cursor = source.cursor();
    let placement = source.monitor_placement();
    let scale_time = capture_started_at.elapsed();
    let encoded = opened.encoder.encode(&frame, job.request)?;
    Ok(Encoded {
        frame,
        capture_size,
        cursor,
        placement,
        scale_time,
        encoded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::wire::VarInt;
    use qdp_codec::SequenceHeader;

    use crate::TestPattern;

    /// The thread encodes requested frames and reports changed blocks, and
    /// a job for a stream of another size reopens the encoder and scales the
    /// captured frames to it.
    #[tokio::test]
    async fn encodes_requested_frames() {
        let (width, height) = (256, 128);
        let settings = EncoderSettings {
            width,
            height,
            frame_rate: 60,
            speed: 10,
            quantizer: 100,
            threads: 1,
        };
        let (thread, description) = EncoderThread::spawn(
            Box::new(TestPattern::new(width, height)),
            None,
            settings,
            CodecPreference::Software,
        )
        .await
        .unwrap();
        assert_eq!(description, "rav1e (software)");
        let first = StreamId(VarInt::from_u32(1));
        let half = EncoderSettings {
            width: width / 2,
            height: height / 2,
            ..settings
        };
        let job = |request, stream_id, settings: EncoderSettings| EncodeJob {
            request,
            recovery: false,
            stream_id,
            settings,
            quantizer: settings.quantizer,
            display: None,
        };
        let jobs = [
            job(KeyFrameRequest::Force, first, settings),
            job(KeyFrameRequest::Auto, first, settings),
            job(KeyFrameRequest::Force, StreamId(VarInt::from_u32(2)), half),
        ];
        let mut outputs = Vec::new();
        for job in jobs {
            thread.request(job).unwrap();
            let output = loop {
                if let Some(result) = thread.try_result() {
                    break result.unwrap();
                }
                std::thread::yield_now();
            };
            outputs.push(output);
        }
        let grid = BlockGrid::for_monitor(width as u32, height as u32);
        assert!(outputs[0].frame.is_key_frame);
        assert!(outputs[0].blocks.is_full());
        assert!(!outputs[1].frame.is_key_frame);
        let changed = outputs[1].blocks.count();
        assert!(
            changed > 0 && changed < grid.len(),
            "{changed} blocks changed"
        );
        let scaled = &outputs[2];
        assert!(scaled.frame.is_key_frame);
        assert_eq!(scaled.capture_size, (width, height));
        let header = SequenceHeader::find(&scaled.frame.temporal_unit)
            .unwrap()
            .unwrap();
        assert_eq!(
            (header.max_width, header.max_height),
            (half.width as u32, half.height as u32)
        );
    }
}
