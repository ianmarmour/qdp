//! Compares decoding in the client's own process with decoding in a separate
//! process, as [`ProcessDecoder`] does (Section 11.5). It encodes video-like
//! frames at several sizes, decodes them both ways, and reports the frames
//! decoded per second and how long each frame took, from handing over its
//! temporal unit to holding its picture.
//!
//! Run it optimized, as the client runs:
//!
//! ```text
//! cargo run --release -p qdp-codec --example decoder_process_benchmark
//! ```

use std::time::{Duration, Instant};

use qdp_codec::{
    Av1Decoder, Av1Encoder, CodecPreference, DecoderCommand, EncoderSettings, FASTEST_SPEED,
    KeyFrameRequest, ProcessDecoder, VideoDecoder, Yuv420Frame, serve_decoder_on_stdio,
};

/// The argument with which the benchmark runs as the decoder process.
const CHILD_ARGUMENT: &str = "--decoder-process";

/// The picture sizes measured.
const SIZES: [(usize, usize); 3] = [(1280, 720), (1920, 1080), (2560, 1440)];

/// The frames encoded at each size, a second at 60 frames per second.
const FRAMES: usize = 60;

/// How many times each way decodes the frames; the rounds alternate, so
/// that both ways meet the same conditions.
const ROUNDS: usize = 3;

/// The quantizer the host uses by default.
const QUANTIZER: u8 = 100;

fn main() {
    if std::env::args().nth(1).as_deref() == Some(CHILD_ARGUMENT) {
        if let Err(error) = serve_decoder_on_stdio() {
            eprintln!("decoder process: {error}");
        }
        return;
    }
    let command = DecoderCommand {
        program: std::env::current_exe().expect("the benchmark knows its own path"),
        args: vec![CHILD_ARGUMENT.into()],
    };
    println!(
        "{FRAMES} frames per size, {ROUNDS} rounds each way; per frame: median and 95th percentile\n"
    );
    for (width, height) in SIZES {
        let frames = encode(width, height);
        let kilobytes = frames.iter().map(Vec::len).sum::<usize>() / frames.len() / 1024;
        let mut in_process = Vec::new();
        let mut separate = Vec::new();
        let mut startups = Vec::new();
        for _ in 0..ROUNDS {
            let decoder = Av1Decoder::new(0).expect("rav1d opens");
            in_process.push(decode(Box::new(decoder), &frames));
            let started = Instant::now();
            let decoder = ProcessDecoder::spawn(command.clone(), 0, CodecPreference::Software)
                .expect("the decoder process starts");
            startups.push(started.elapsed());
            separate.push(decode(Box::new(decoder), &frames));
        }
        println!("{width}×{height}, {kilobytes} KB per frame:");
        let in_process = summarize("  in the client's process ", &in_process);
        let separate = summarize("  in a separate process   ", &separate);
        println!(
            "  difference: {:+.1}% frames per second, {:+.2} ms per frame (median); \
             process start {:.1} ms\n",
            (separate.0 / in_process.0 - 1.0) * 100.0,
            (separate.1 - in_process.1).as_secs_f64() * 1000.0,
            median(&mut startups).as_secs_f64() * 1000.0,
        );
    }
}

/// Encodes [`FRAMES`] frames of `width × height` video-like pictures, the
/// first a key frame, as a host streams them.
fn encode(width: usize, height: usize) -> Vec<Vec<u8>> {
    let mut encoder = Av1Encoder::new(EncoderSettings {
        width,
        height,
        frame_rate: 60,
        speed: FASTEST_SPEED,
        quantizer: QUANTIZER,
        threads: 0,
    })
    .expect("rav1e opens");
    let mut encoded = Vec::new();
    for index in 0..FRAMES {
        let request = if index == 0 {
            KeyFrameRequest::Force
        } else {
            KeyFrameRequest::Auto
        };
        encoded.extend(
            encoder
                .encode(&picture(width, height, index), request)
                .expect("rav1e encodes"),
        );
    }
    encoded.extend(encoder.flush().expect("rav1e flushes"));
    encoded
        .into_iter()
        .map(|frame| frame.temporal_unit)
        .collect()
}

/// Returns picture `index`: a still desktop around a video, whose shapes
/// move and whose grain changes every frame, as filmed video's does.
fn picture(width: usize, height: usize, index: usize) -> Yuv420Frame {
    let mut picture = Yuv420Frame::black(width, height);
    let (left, right) = (width / 8, width * 7 / 8);
    let (top, bottom) = (height / 8, height * 7 / 8);
    let time = index as f32 / 10.0;
    let seed = (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    for (row, line) in picture.y_mut().chunks_exact_mut(width).enumerate() {
        for (column, pixel) in line.iter_mut().enumerate() {
            let in_video = (left..right).contains(&column) && (top..bottom).contains(&row);
            let value = if in_video {
                let shapes = (column as f32 / 40.0 + time).sin() * (row as f32 / 30.0 - time).cos();
                let mut hash = seed ^ ((row * width + column) as u64).wrapping_mul(0xbf58_476d);
                hash ^= hash >> 29;
                120 + (shapes * 70.0) as i32 + (hash % 16) as i32 - 8
            } else {
                200 - (row / 8) as i32
            };
            *pixel = value.clamp(16, 235) as u8;
        }
    }
    picture
}

/// Decodes `frames` in order, returning how long each took.
fn decode(mut decoder: Box<dyn VideoDecoder>, frames: &[Vec<u8>]) -> Vec<Duration> {
    frames
        .iter()
        .map(|frame| {
            let started = Instant::now();
            decoder.decode(frame).expect("the frame decodes");
            started.elapsed()
        })
        .collect()
}

/// Prints one way's results, labelled `label`, and returns its frames per
/// second and median time per frame.
fn summarize(label: &str, rounds: &[Vec<Duration>]) -> (f64, Duration) {
    let mut totals: Vec<Duration> = rounds.iter().map(|round| round.iter().sum()).collect();
    let frames_per_second = FRAMES as f64 / median(&mut totals).as_secs_f64();
    let mut each: Vec<Duration> = rounds.iter().flatten().copied().collect();
    let typical = median(&mut each);
    let slow = percentile(&mut each, 0.95);
    println!(
        "{label}{frames_per_second:7.1} frames/s   {:6.2} ms   {:6.2} ms",
        typical.as_secs_f64() * 1000.0,
        slow.as_secs_f64() * 1000.0
    );
    (frames_per_second, typical)
}

/// Returns the median of `values`.
fn median(values: &mut [Duration]) -> Duration {
    percentile(values, 0.5)
}

/// Returns the value below which `share` of `values` lie.
fn percentile(values: &mut [Duration], share: f64) -> Duration {
    values.sort_unstable();
    let index = ((values.len() - 1) as f64 * share).round() as usize;
    values[index]
}
