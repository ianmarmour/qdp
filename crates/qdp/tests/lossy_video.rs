//! A simulated video stream over a lossy network: the host's `VideoSender` and
//! the client's `VideoReceiver`, exchanging fragments and feedback through a
//! network that delays and drops datagrams, in simulated time.
//!
//! The host follows the reference runtime's policy: a key frame first, and a key
//! frame flagged `RECOVERY` whenever the sender reports that one is needed.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use qdp::message::{
    BlockGrid, BlockSet, Datagram, FeedbackRecord, FrameHeader, StreamId, Timestamp, VideoLayer,
};
use qdp::time::SessionClock;
use qdp::video::{FrameKind, VideoReceiver, VideoSender};
use qdp::wire::VarInt;

/// The time between frames, for 60 frames per second.
const FRAME_INTERVAL: Duration = Duration::from_micros(16_667);

/// The one-way delay of the network.
const ONE_WAY_DELAY: Duration = Duration::from_millis(5);

/// The smoothed RTT both sides use.
const RTT: Duration = Duration::from_millis(10);

/// The step of simulated time.
const STEP: Duration = Duration::from_millis(1);

/// The largest datagram.
const DATAGRAM_SIZE: usize = 1200;

/// The temporal unit size of a key frame: about 40 fragments.
const KEY_FRAME_SIZE: usize = 48_000;

/// The temporal unit size of a delta frame: a few fragments.
const DELTA_FRAME_SIZE: usize = 3_000;

/// A small deterministic random number generator (xorshift64*).
struct Random(u64);

impl Random {
    /// Returns a number in `[0, 1)`.
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// What travels from the host to the client.
type Downlink = BTreeMap<(Instant, u64), Vec<u8>>;

/// What travels from the client to the host.
type Uplink = BTreeMap<(Instant, u64), Vec<FeedbackRecord>>;

/// The outcome of a simulated session.
#[derive(Debug)]
struct Outcome {
    /// The frames the host sent.
    sent: u64,
    /// The frames the client decoded.
    decoded: u64,
    /// The newest frame decoded.
    last_decoded: u64,
    /// The longest time without a decoded frame.
    longest_stall: Duration,
    /// The key frames sent after the first.
    recovery_key_frames: u64,
}

/// Streams for `duration` with `loss` probability per datagram in each
/// direction.
fn simulate(duration: Duration, loss: f64, seed: u64) -> Outcome {
    let start = Instant::now();
    let stream_id = StreamId(VarInt::from_u32(1));
    let mut sender = VideoSender::new(stream_id);
    let mut receiver = VideoReceiver::new(
        stream_id,
        VideoLayer::Motion,
        BlockGrid::for_monitor(1280, 720),
        SessionClock::new(start),
    );
    let mut random = Random(seed);
    let mut downlink = Downlink::new();
    let mut uplink = Uplink::new();
    let mut sequence = 0u64;
    let mut next_frame_at = start;
    let mut outcome = Outcome {
        sent: 0,
        decoded: 0,
        last_decoded: 0,
        longest_stall: Duration::ZERO,
        recovery_key_frames: 0,
    };
    let mut last_decode_at = start;

    let mut now = start;
    while now < start + duration {
        let mut send = |datagrams: Vec<Vec<u8>>, random: &mut Random, sequence: &mut u64| {
            for datagram in datagrams {
                *sequence += 1;
                if random.next() >= loss {
                    downlink.insert((now + ONE_WAY_DELAY, *sequence), datagram);
                }
            }
        };

        // The host encodes a frame on schedule.
        if now >= next_frame_at {
            let kind = if outcome.sent == 0 {
                FrameKind::Key
            } else if sender.recovery_needed().is_some() {
                outcome.recovery_key_frames += 1;
                FrameKind::KeyRecovery
            } else {
                FrameKind::Delta
            };
            let size = if matches!(kind, FrameKind::Key | FrameKind::KeyRecovery) {
                KEY_FRAME_SIZE
            } else {
                DELTA_FRAME_SIZE
            };
            let header = FrameHeader {
                capture_timestamp: Timestamp::default(),
                recovery_base: None,
                base_frame_number: None,
                blocks: BlockSet::full(BlockGrid::for_monitor(1280, 720).len()),
            };
            let sent = sender
                .send_frame(header, &vec![0x55; size], kind, DATAGRAM_SIZE, now)
                .unwrap();
            outcome.sent += 1;
            send(sent.datagrams, &mut random, &mut sequence);
            next_frame_at += FRAME_INTERVAL;
        }
        let retransmissions = sender.handle_timeout(RTT, now);
        send(retransmissions, &mut random, &mut sequence);

        // Feedback arrives at the host.
        let arrived: Vec<_> = uplink
            .range(..=(now, u64::MAX))
            .map(|(&key, _)| key)
            .collect();
        for key in arrived {
            for record in uplink.remove(&key).unwrap() {
                match record {
                    FeedbackRecord::Decoded(decoded) => {
                        sender.handle_decoded(decoded.last_decoded_frame);
                    }
                    FeedbackRecord::Loss(loss) => sender.handle_loss(&loss, RTT, now),
                    FeedbackRecord::Nack(nack) => {
                        let retransmissions = sender.handle_nack(&nack, RTT, now);
                        send(retransmissions, &mut random, &mut sequence);
                    }
                    _ => {}
                }
            }
        }

        // Fragments arrive at the client.
        let arrived: Vec<_> = downlink
            .range(..=(now, u64::MAX))
            .map(|(&key, _)| key)
            .collect();
        for key in arrived {
            let datagram = downlink.remove(&key).unwrap();
            let Ok(Datagram::VideoFragment(fragment)) = Datagram::decode(&datagram) else {
                panic!("not a video fragment");
            };
            receiver.handle_fragment(&fragment, RTT, now);
        }
        if receiver
            .poll_timeout(RTT)
            .is_some_and(|deadline| now >= deadline)
        {
            receiver.handle_timeout(RTT, now);
        }
        while let Some(frame) = receiver.poll_decodable() {
            receiver.frame_decoded(frame.frame_number, now);
            outcome.decoded += 1;
            outcome.last_decoded = frame.frame_number.0.into_inner();
            last_decode_at = now;
        }
        outcome.longest_stall = outcome
            .longest_stall
            .max(now.saturating_duration_since(last_decode_at));
        let records = receiver.poll_feedback(now);
        if !records.is_empty() {
            sequence += 1;
            if random.next() >= loss {
                uplink.insert((now + ONE_WAY_DELAY, sequence), records);
            }
        }
        now += STEP;
    }
    outcome
}

/// Over a network losing up to a tenth of the datagrams each way, the stream
/// never stalls for long and keeps decoding frames to the end.
///
/// This caught two stalls: a frame whose `NACK`s each restarted its loss timer
/// was never declared lost, and a frame none of whose fragments arrived was
/// never noticed at all.
#[test]
fn recovers_from_random_loss() {
    // The longest stall allowed at each loss rate, with a margin over the worst
    // of 100 seeds.
    let limits = [
        (0.01, Duration::from_millis(200)),
        (0.03, Duration::from_millis(300)),
        (0.10, Duration::from_secs(2)),
    ];
    for seed in 1..=20 {
        for (loss, longest_stall) in limits {
            let outcome = simulate(Duration::from_secs(20), loss, seed);
            assert!(
                outcome.longest_stall < longest_stall && outcome.last_decoded + 10 >= outcome.sent,
                "seed {seed}, loss {loss}: {outcome:?}"
            );
        }
    }
}
