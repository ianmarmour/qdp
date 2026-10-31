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

/// The network a simulated session crosses.
#[derive(Debug, Clone, Copy)]
struct Network {
    /// The probability that a datagram is lost, in each direction.
    loss: f64,
    /// How often the client's radio leaves its channel, as Wi-Fi cards do to
    /// scan or to talk to nearby devices, or zero for never.
    away_every: Duration,
    /// How long it stays away: what would arrive meanwhile arrives, in order,
    /// when it returns.
    away_for: Duration,
    /// The time each datagram takes to leave the host, as its congestion
    /// controller paces them, or zero to send a frame's at once.
    pacing: Duration,
    /// The temporal unit size of a delta frame.
    delta_frame_size: usize,
    /// The bytes per second the link carries toward the client, or zero for
    /// no limit. What it cannot carry at once waits in a buffer of
    /// [`BOTTLENECK_BUFFER`] bytes, and what overflows the buffer is lost.
    capacity: f64,
}

impl Network {
    /// A network that loses `loss` of the datagrams in each direction.
    fn lossy(loss: f64) -> Self {
        Self {
            loss,
            away_every: Duration::ZERO,
            away_for: Duration::ZERO,
            pacing: Duration::ZERO,
            delta_frame_size: DELTA_FRAME_SIZE,
            capacity: 0.0,
        }
    }

    /// Returns when something sent at `sent`, `start` being when the session
    /// began, arrives.
    fn arrival(&self, start: Instant, sent: Instant) -> Instant {
        let arrival = sent + ONE_WAY_DELAY;
        if self.away_every.is_zero() {
            return arrival;
        }
        let since = arrival.saturating_duration_since(start);
        let into_period =
            Duration::from_nanos((since.as_nanos() % self.away_every.as_nanos()) as u64);
        if into_period < self.away_for {
            arrival + (self.away_for - into_period)
        } else {
            arrival
        }
    }
}

/// The bytes a link of limited capacity buffers before it loses datagrams.
const BOTTLENECK_BUFFER: f64 = 64_000.0;

/// A link of limited capacity toward the client.
#[derive(Debug, Default)]
struct Bottleneck {
    /// When the link has carried everything it was given.
    free_at: Option<Instant>,
}

impl Bottleneck {
    /// Gives the link a datagram of `size` bytes at `at`, returning when it
    /// has crossed at `capacity` bytes per second, or `None` if the buffer
    /// is full and it is lost.
    fn carry(&mut self, size: usize, at: Instant, capacity: f64) -> Option<Instant> {
        if capacity == 0.0 {
            return Some(at);
        }
        let start = self.free_at.map_or(at, |free_at| free_at.max(at));
        let waiting = start.saturating_duration_since(at).as_secs_f64() * capacity;
        if waiting > BOTTLENECK_BUFFER {
            return None;
        }
        let done = start + Duration::from_secs_f64(size as f64 / capacity);
        self.free_at = Some(done);
        Some(done)
    }
}

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

/// Streams for `duration` across `network`.
fn simulate(duration: Duration, network: Network, seed: u64) -> Outcome {
    let loss = network.loss;
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
    let mut bottleneck = Bottleneck::default();
    // When the host's pacing lets the next datagram leave.
    let mut paced_until = start;
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
                let sent = paced_until.max(now);
                paced_until = sent + network.pacing;
                if random.next() < loss {
                    continue;
                }
                if let Some(carried) = bottleneck.carry(datagram.len(), sent, network.capacity) {
                    downlink.insert((network.arrival(start, carried), *sequence), datagram);
                }
            }
        };

        // The host encodes a frame on schedule, except while a recovery
        // frame is held back after earlier ones failed.
        let held = sender.recovery_wait().is_some_and(|due| now < due);
        if now >= next_frame_at && !held {
            let kind = if outcome.sent == 0 {
                FrameKind::Key
            } else if sender.recovery_needed(now).is_some() {
                outcome.recovery_key_frames += 1;
                FrameKind::KeyRecovery
            } else {
                FrameKind::Delta
            };
            let size = if matches!(kind, FrameKind::Key | FrameKind::KeyRecovery) {
                KEY_FRAME_SIZE
            } else {
                network.delta_frame_size
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
                uplink.insert((network.arrival(start, now), sequence), records);
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
            let outcome = simulate(Duration::from_secs(20), Network::lossy(loss), seed);
            assert!(
                outcome.longest_stall < longest_stall && outcome.last_decoded + 10 >= outcome.sent,
                "seed {seed}, loss {loss}: {outcome:?}"
            );
        }
    }
}

/// Over a link that carries less than the stream sends, recovery frames do
/// not flood it, and the client still decodes frames now and then. Without
/// rate control, which this simulation leaves out, it can do no better.
///
/// Before failed recovery frames backed off, each was repeated as soon as
/// the client reported it incomplete: over 330 recovery key frames in 20
/// seconds, and the client decoded nothing after the first frame.
#[test]
fn recovery_does_not_flood_an_overloaded_link() {
    for capacity in [0.9, 0.7] {
        // Delta frames of 10 kB at 60 frames per second: 600 kB/s.
        let network = Network {
            pacing: Duration::from_micros(50),
            delta_frame_size: 10_000,
            capacity: capacity * 600_000.0,
            ..Network::lossy(0.0)
        };
        let outcome = simulate(Duration::from_secs(20), network, 1);
        assert!(
            outcome.recovery_key_frames <= 100 && outcome.decoded >= 10,
            "capacity {capacity}: {outcome:?}"
        );
    }
}

/// A client whose radio leaves its channel for a moment loses nothing: what
/// arrives late is not mistaken for lost, so the host sends no recovery
/// frames and the client decodes every frame.
///
/// Before frames were declared lost only while the stream showed it, each
/// absence that split a frame cost a recovery frame: 16 in 20 seconds.
#[test]
fn late_fragments_are_not_lost() {
    for away_ms in [10, 20, 30, 40] {
        // Frames of about 70 fragments, paced out at about 100 Mbit/s, as a
        // 2560 × 1080 desktop playing video sends them.
        // A period that is no whole number of frames, so that absences
        // begin at every point of a frame.
        let network = Network {
            away_every: Duration::from_millis(487),
            away_for: Duration::from_millis(away_ms),
            pacing: Duration::from_micros(100),
            delta_frame_size: 80_000,
            ..Network::lossy(0.0)
        };
        let outcome = simulate(Duration::from_secs(20), network, 1);
        // The frames still on their way when the session ends excepted.
        assert!(
            outcome.recovery_key_frames <= 1 && outcome.decoded + 2 >= outcome.sent,
            "away {away_ms} ms: {outcome:?}"
        );
    }
}
