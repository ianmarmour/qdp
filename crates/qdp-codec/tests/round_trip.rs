//! Integration tests that encode frames with rav1e, carry them through the QDP
//! video sender and receiver, and decode them with rav1d.

use std::time::{Duration, Instant};

use qdp::message::{
    BlockGrid, BlockSet, DatagramPayload, FrameHeader, StreamId, Timestamp, VideoFragment,
    VideoLayer,
};
use qdp::time::SessionClock;
use qdp::video::{FrameKind, VideoReceiver, VideoSender};
use qdp::wire::{Reader, VarInt};
use qdp_codec::{
    Av1Decoder, Av1Encoder, EncodedFrame, EncoderSettings, FASTEST_SPEED, KeyFrameRequest, ObuType,
    Yuv420Frame, parse_obus,
};

/// The width of the test frames.
const WIDTH: usize = 320;

/// The height of the test frames.
const HEIGHT: usize = 192;

/// The quantizer of the test encoder: good quality, to make PSNR meaningful.
const QUANTIZER: u8 = 60;

/// The frame rate the encoder is told of.
const FRAME_RATE: u32 = 60;

/// The lowest acceptable luma PSNR of a decoded frame, in decibels.
const MIN_PSNR: f64 = 30.0;

/// The datagram size of the simulated path.
const DATAGRAM_SIZE: usize = 1200;

/// Returns frame `index` of a test sequence: a gradient with a moving bright
/// square, so consecutive frames differ.
fn test_frame(index: usize) -> Yuv420Frame {
    let mut frame = Yuv420Frame::black(WIDTH, HEIGHT);
    let square = (index * 8) % (WIDTH - 32);
    for row in 0..HEIGHT {
        for column in 0..WIDTH {
            let value = if (square..square + 32).contains(&column) && (40..72).contains(&row) {
                235
            } else {
                16 + ((row + column) % 200) as u8
            };
            frame.y_mut()[row * WIDTH + column] = value;
        }
    }
    let chroma_width = frame.chroma_width();
    for (index, sample) in frame.u_mut().iter_mut().enumerate() {
        *sample = 100 + ((index % chroma_width) % 40) as u8;
    }
    frame
}

/// Returns an encoder for the test frames.
fn encoder() -> Av1Encoder {
    Av1Encoder::new(EncoderSettings {
        width: WIDTH,
        height: HEIGHT,
        frame_rate: FRAME_RATE,
        speed: FASTEST_SPEED,
        quantizer: QUANTIZER,
        threads: 2,
    })
    .unwrap()
}

/// Every call to the encoder returns the frame it was given, with no lookahead
/// delay, so the software encoder adds no frames of latency.
#[test]
fn encoder_returns_each_frame_immediately() {
    let mut encoder = encoder();
    for index in 0..8u64 {
        let request = if index == 0 {
            KeyFrameRequest::Force
        } else {
            KeyFrameRequest::Auto
        };
        let frames = encoder
            .encode(&test_frame(index as usize), request)
            .unwrap();
        let indices: Vec<u64> = frames.iter().map(|frame| frame.input_index).collect();
        assert_eq!(indices, [index], "frame {index}");
    }
    assert!(encoder.flush().unwrap().is_empty());
}

/// Encodes `count` test frames, forcing a key frame at the indices in `keys`, and
/// returns every encoded frame in output order, including those flushed at the
/// end.
fn encode_sequence(encoder: &mut Av1Encoder, count: usize, keys: &[usize]) -> Vec<EncodedFrame> {
    let mut encoded = Vec::new();
    for index in 0..count {
        let request = if keys.contains(&index) {
            KeyFrameRequest::Force
        } else {
            KeyFrameRequest::Auto
        };
        encoded.extend(encoder.encode(&test_frame(index), request).unwrap());
    }
    encoded.extend(encoder.flush().unwrap());
    encoded
}

/// Encoded temporal units meet QDP's bitstream rules, come out in input order,
/// and decode to pictures close to the source.
#[test]
fn encode_decode() {
    let mut encoder = encoder();
    let encoded = encode_sequence(&mut encoder, 10, &[0]);
    assert_eq!(encoded.len(), 10);
    let mut decoder = Av1Decoder::new(2).unwrap();
    for (index, frame) in encoded.iter().enumerate() {
        assert_eq!(frame.input_index, index as u64);
        assert_eq!(frame.is_key_frame, index == 0, "frame {index}");

        let obus = parse_obus(&frame.temporal_unit).unwrap();
        assert!(
            obus.iter()
                .all(|obu| obu.obu_type != ObuType::TemporalDelimiter)
        );
        if frame.is_key_frame {
            assert_eq!(obus[0].obu_type, ObuType::SequenceHeader);
        }

        let decoded = decoder.decode(&frame.temporal_unit).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (WIDTH, HEIGHT));
        let psnr = decoded.luma_psnr(&test_frame(index)).unwrap();
        assert!(psnr > MIN_PSNR, "frame {index}: {psnr:.1} dB");
    }
}

/// A forced key frame mid-stream starts with a sequence header and lets a fresh
/// decoder start there.
#[test]
fn forced_key_frame_restarts_decoding() {
    let mut encoder = encoder();
    let encoded = encode_sequence(&mut encoder, 6, &[0, 3]);
    let key = encoded
        .iter()
        .find(|frame| frame.input_index == 3)
        .expect("frame 3");
    assert!(key.is_key_frame);
    let mut decoder = Av1Decoder::new(1).unwrap();
    let decoded = decoder.decode(&key.temporal_unit).unwrap();
    assert!(decoded.luma_psnr(&test_frame(3)).unwrap() > MIN_PSNR);
}

/// Frames travel from the encoder through fragmentation, reassembly and decode
/// ordering to the decoder.
#[test]
fn through_the_video_pipeline() {
    let start = Instant::now();
    let stream_id = StreamId(VarInt::from_u32(1));
    let grid = BlockGrid::for_monitor(WIDTH as u32, HEIGHT as u32);
    let mut encoder = encoder();
    let mut decoder = Av1Decoder::new(2).unwrap();
    let mut sender = VideoSender::new(stream_id);
    let mut receiver = VideoReceiver::new(
        stream_id,
        VideoLayer::Motion,
        grid,
        SessionClock::new(start),
    );
    let smoothed_rtt = Duration::from_millis(10);
    let frame_count = 6;

    for encoded in encode_sequence(&mut encoder, frame_count, &[0]) {
        let index = encoded.input_index;
        let now = start + Duration::from_millis(16 * index);
        let kind = if encoded.is_key_frame {
            FrameKind::Key
        } else {
            FrameKind::Delta
        };
        let header = FrameHeader {
            capture_timestamp: Timestamp(VarInt::from_u32(16_000 * index as u32)),
            recovery_base: None,
            base_frame_number: None,
            blocks: BlockSet::full(grid.len()),
        };
        let sent = sender
            .send_frame(header, &encoded.temporal_unit, kind, DATAGRAM_SIZE, now)
            .unwrap();
        // Deliver the fragments in reverse to exercise reassembly.
        for datagram in sent.datagrams.iter().rev() {
            assert!(datagram.len() <= DATAGRAM_SIZE);
            let fragment = VideoFragment::decode(&mut Reader::new(&datagram[1..])).unwrap();
            receiver.handle_fragment(&fragment, smoothed_rtt, now);
        }
        let frame = receiver.poll_decodable().expect("a complete frame");
        assert_eq!(frame.frame_number, sent.frame_number);
        let decoded = decoder.decode(frame.temporal_unit()).unwrap();
        receiver.frame_decoded(frame.frame_number, now);
        let source = test_frame(index as usize);
        assert!(decoded.luma_psnr(&source).unwrap() > MIN_PSNR);
    }
    assert_eq!(receiver.last_decoded().0.into_inner(), frame_count as u64);
}
