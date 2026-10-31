//! Capture and encoding on a dedicated thread, so that the host's network loop
//! keeps answering feedback, retransmission requests and clock pings while a
//! frame is being encoded.

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::Instant;

use qdp::message::{BlockGrid, BlockSet};
use qdp_codec::{Av1Encoder, EncodedFrame, KeyFrameRequest, Yuv420Frame};
use tokio::sync::Notify;

use crate::RuntimeError;
use crate::media::{FrameSource, changed_blocks};

/// The name of the encoder thread, as debuggers and profilers show it.
const THREAD_NAME: &str = "qdp-encoder";

/// A request to capture and encode one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EncodeJob {
    /// Whether the frame must be a key frame.
    pub(crate) request: KeyFrameRequest,
    /// Whether the frame answers a loss, so that it is flagged `RECOVERY`.
    pub(crate) recovery: bool,
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
    /// Starts the thread with a source and an encoder for frames divided into
    /// `grid`.
    pub(crate) fn spawn(
        source: Box<dyn FrameSource>,
        encoder: Av1Encoder,
        grid: BlockGrid,
    ) -> Result<Self, RuntimeError> {
        let (job_sender, jobs) = mpsc::channel();
        let (result_sender, results) = mpsc::channel();
        let ready = Arc::new(Notify::new());
        let notify = Arc::clone(&ready);
        let handle = std::thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || run(source, encoder, grid, &jobs, &result_sender, &notify))
            .map_err(|error| RuntimeError::Capture(error.to_string()))?;
        Ok(Self {
            jobs: Some(job_sender),
            results,
            ready,
            handle: Some(handle),
        })
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

/// The thread's loop: one result for each job, until the job channel closes or
/// the result channel is dropped.
fn run(
    mut source: Box<dyn FrameSource>,
    mut encoder: Av1Encoder,
    grid: BlockGrid,
    jobs: &Receiver<EncodeJob>,
    results: &Sender<Result<EncodedOutput, RuntimeError>>,
    ready: &Notify,
) {
    let mut previous: Option<Yuv420Frame> = None;
    let mut next_input_index = 0;
    while let Ok(job) = jobs.recv() {
        let captured_at = Instant::now();
        let outputs = encode_one(&mut *source, &mut encoder, job).map(|(frame, encoded)| {
            let input_index = next_input_index;
            next_input_index += 1;
            let changed = previous
                .as_ref()
                .map(|previous| changed_blocks(previous, &frame, grid));
            previous = Some(frame);
            encoded
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

/// Captures one frame and encodes it.
fn encode_one(
    source: &mut dyn FrameSource,
    encoder: &mut Av1Encoder,
    job: EncodeJob,
) -> Result<(Yuv420Frame, Vec<EncodedFrame>), RuntimeError> {
    let frame = source.capture()?;
    let encoded = encoder.encode(&frame, job.request)?;
    Ok((frame, encoded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TestPattern;
    use qdp_codec::EncoderSettings;

    /// The thread encodes requested frames and reports changed blocks.
    #[test]
    fn encodes_requested_frames() {
        let (width, height) = (256, 128);
        let encoder = Av1Encoder::new(EncoderSettings {
            width,
            height,
            speed: 10,
            quantizer: 100,
            threads: 1,
        })
        .unwrap();
        let grid = BlockGrid::for_monitor(width as u32, height as u32);
        let thread =
            EncoderThread::spawn(Box::new(TestPattern::new(width, height)), encoder, grid).unwrap();
        let jobs = [
            EncodeJob {
                request: KeyFrameRequest::Force,
                recovery: false,
            },
            EncodeJob {
                request: KeyFrameRequest::Auto,
                recovery: false,
            },
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
        assert!(outputs[0].frame.is_key_frame);
        assert!(outputs[0].blocks.is_full());
        assert!(!outputs[1].frame.is_key_frame);
        let changed = outputs[1].blocks.count();
        assert!(
            changed > 0 && changed < grid.len(),
            "{changed} blocks changed"
        );
    }
}
