//! Decoding on a thread of its own for each video stream, so that the
//! client's network loop keeps receiving and timing fragments, and sending
//! feedback and input, while a frame decodes (Sections 6.7 and 6.8).

use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use qdp::video::DecodableFrame;
use qdp_codec::{CodecError, DecodedPicture, VideoDecoder};
use tokio::sync::Notify;

use crate::RuntimeError;

/// The name of decoder threads, as debuggers and profilers show it.
const THREAD_NAME: &str = "qdp-decoder";

/// A decoded frame, or why it could not be decoded.
#[derive(Debug)]
pub(crate) struct Decoded {
    /// The frame passed to the decoder.
    pub(crate) frame: DecodableFrame,
    /// The picture, where the decoder left it, or the decoder's error.
    pub(crate) picture: Result<DecodedPicture, CodecError>,
    /// When decoding finished.
    pub(crate) decoded_at: Instant,
    /// How long the decoder took.
    pub(crate) decode_time: Duration,
}

/// A thread that owns one stream's decoder.
#[derive(Debug)]
pub(crate) struct DecodeThread {
    /// Sends frames to the thread; dropping it ends the thread.
    frames: Option<Sender<DecodableFrame>>,
    /// Receives the thread's results.
    results: Receiver<Decoded>,
    /// The thread, which ends on its own once its frames stop.
    _handle: JoinHandle<()>,
}

impl DecodeThread {
    /// Starts a thread decoding with `decoder`, which notifies `wake` when a
    /// result is ready.
    pub(crate) fn spawn(
        mut decoder: Box<dyn VideoDecoder>,
        wake: Arc<Notify>,
    ) -> Result<Self, RuntimeError> {
        let (frame_sender, frames) = mpsc::channel::<DecodableFrame>();
        let (result_sender, results) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name(THREAD_NAME.to_owned())
            .spawn(move || {
                while let Ok(frame) = frames.recv() {
                    let started_at = Instant::now();
                    let picture = decoder.decode_picture(frame.temporal_unit());
                    let decoded_at = Instant::now();
                    let decoded = Decoded {
                        frame,
                        picture,
                        decoded_at,
                        decode_time: decoded_at.saturating_duration_since(started_at),
                    };
                    if result_sender.send(decoded).is_err() {
                        return;
                    }
                    wake.notify_one();
                }
            })
            .map_err(|error| RuntimeError::Decode(error.to_string()))?;
        Ok(Self {
            frames: Some(frame_sender),
            results,
            _handle: handle,
        })
    }

    /// Passes a frame to the decoder.
    pub(crate) fn decode(&self, frame: DecodableFrame) -> Result<(), RuntimeError> {
        self.frames
            .as_ref()
            .and_then(|frames| frames.send(frame).ok())
            .ok_or_else(|| RuntimeError::Decode("the decoder thread stopped".to_owned()))
    }

    /// Returns a finished result, if one is waiting.
    pub(crate) fn try_result(&self) -> Result<Option<Decoded>, RuntimeError> {
        match self.results.try_recv() {
            Ok(decoded) => Ok(Some(decoded)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(RuntimeError::Decode(
                "the decoder thread stopped".to_owned(),
            )),
        }
    }
}

impl Drop for DecodeThread {
    fn drop(&mut self) {
        // Closing the channel ends the thread once its frame, if any, is
        // decoded; a replaced stream need not wait for it.
        self.frames = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use qdp::message::{
        BlockGrid, BlockSet, DatagramPayload, FrameHeader, StreamId, Timestamp, VideoFragment,
        VideoLayer,
    };
    use qdp::time::SessionClock;
    use qdp::video::{FrameKind, VideoReceiver, VideoSender};
    use qdp::wire::{Reader, VarInt};
    use qdp_codec::{
        Av1Decoder, Av1Encoder, EncoderSettings, FASTEST_SPEED, KeyFrameRequest, Yuv420Frame,
    };

    /// The test frame's side, in pixels.
    const SIDE: usize = 64;

    /// How long the test waits for the decoder.
    const WAIT: Duration = Duration::from_secs(10);

    /// Returns a key frame of a black picture, as the client receives it.
    fn key_frame() -> DecodableFrame {
        let mut encoder = Av1Encoder::new(EncoderSettings {
            width: SIDE,
            height: SIDE,
            frame_rate: 60,
            speed: FASTEST_SPEED,
            quantizer: 100,
            threads: 1,
        })
        .unwrap();
        let black = Yuv420Frame::black(SIDE, SIDE);
        let mut encoded = encoder.encode(&black, KeyFrameRequest::Force).unwrap();
        encoded.extend(encoder.flush().unwrap());
        let stream_id = StreamId(VarInt::from_u32(1));
        let grid = BlockGrid::for_monitor(SIDE as u32, SIDE as u32);
        let header = FrameHeader {
            capture_timestamp: Timestamp::default(),
            recovery_base: None,
            base_frame_number: None,
            blocks: BlockSet::full(grid.len()),
        };
        let now = Instant::now();
        let sent = VideoSender::new(stream_id)
            .send_frame(header, &encoded[0].temporal_unit, FrameKind::Key, 1200, now)
            .unwrap();
        let mut receiver =
            VideoReceiver::new(stream_id, VideoLayer::Motion, grid, SessionClock::new(now));
        for datagram in &sent.datagrams {
            let fragment = VideoFragment::decode(&mut Reader::new(&datagram[1..])).unwrap();
            receiver.handle_fragment(&fragment, Duration::from_millis(1), now);
        }
        receiver.poll_decodable().unwrap()
    }

    /// The thread decodes a frame and says so.
    #[tokio::test]
    async fn decodes_frames() {
        let wake = Arc::new(Notify::new());
        let decoder = Box::new(Av1Decoder::new(1).unwrap());
        let thread = DecodeThread::spawn(decoder, Arc::clone(&wake)).unwrap();
        assert!(thread.try_result().unwrap().is_none());
        thread.decode(key_frame()).unwrap();
        tokio::time::timeout(WAIT, wake.notified()).await.unwrap();
        let decoded = thread.try_result().unwrap().unwrap();
        let picture = decoded.picture.unwrap();
        assert_eq!(picture.size(), (SIDE, SIDE));
        assert_eq!(decoded.frame.frame_number.0.into_inner(), 1);
    }
}
