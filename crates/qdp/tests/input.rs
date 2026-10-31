//! Integration test that runs the client's input sender against the host's input
//! receiver over a lossy channel.

use std::time::{Duration, Instant};

use qdp::input::{DeviceClass, DeviceRest, InputReceiver, InputSender, ReportKind};
use qdp::message::{DatagramPayload, DeviceId, Input, InputAck};
use qdp::time::SessionClock;
use qdp::wire::{Reader, VarInt};

/// The smoothed RTT of the simulated path.
const RTT: Duration = Duration::from_millis(8);

/// The datagram size of the simulated path.
const DATAGRAM_SIZE: usize = 1200;

/// The simulation's time step.
const STEP: Duration = Duration::from_millis(1);

/// A deterministic pseudo-random loss pattern that drops about one datagram in
/// three, using xorshift so that losses do not line up with the repeat schedule.
struct Loss {
    /// The generator state.
    state: u64,
}

impl Loss {
    /// The generator's seed.
    const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

    /// Creates the loss pattern.
    fn new() -> Self {
        Self { state: Self::SEED }
    }

    /// Returns whether the next datagram is dropped.
    fn drops(&mut self) -> bool {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state.is_multiple_of(3)
    }
}

/// Every key transition reaches the host exactly once and in order, despite
/// heavy pseudo-random datagram loss in both directions.
#[test]
fn transitions_survive_random_loss() {
    let mut loss = Loss::new();
    simulate(|| loss.drops());
}

/// Every key transition reaches the host even when every other datagram is
/// lost, in step with the repeat interval, because retained reports ride along
/// in every datagram.
#[test]
fn transitions_survive_periodic_loss() {
    let mut count = 0u32;
    simulate(|| {
        count += 1;
        count.is_multiple_of(2)
    });
}

/// Runs the client's sender against the host's receiver for 400 ms, pushing 100
/// key transitions, while `drops` decides which datagrams the path loses.
fn simulate(mut drops: impl FnMut() -> bool) {
    let start = Instant::now();
    let keyboard = DeviceId(VarInt::from_u32(1));
    let mut sender = InputSender::new(SessionClock::new(start));
    sender.add_device(keyboard, DeviceClass::Standard);
    let mut receiver = InputReceiver::new();

    let mut applied = Vec::new();
    let mut now = start;
    for step in 0..400u32 {
        if step < 100 {
            let rest = if step % 2 == 0 {
                DeviceRest::Active
            } else {
                DeviceRest::AtRest
            };
            sender.push(
                keyboard,
                vec![step as u8],
                ReportKind::Transition,
                rest,
                now,
            );
        }
        while let Some(datagram) = sender.poll_transmit(RTT, DATAGRAM_SIZE, now) {
            if drops() {
                continue;
            }
            let input = Input::decode(&mut Reader::new(&datagram[1..])).unwrap();
            applied.extend(receiver.handle_input(&input, |_| true, RTT, now));
        }
        applied.extend(receiver.handle_timeout(RTT, now));
        if let Some(ack) = receiver.poll_ack(now) {
            if !drops() {
                let bytes = ack.to_bytes();
                let ack = InputAck::decode(&mut Reader::new(&bytes[1..])).unwrap();
                sender.handle_ack(&ack, now);
            }
        }
        now += STEP;
    }

    let data: Vec<u8> = applied.iter().map(|event| event.data[0]).collect();
    let expected: Vec<u8> = (0..100).collect();
    assert_eq!(data, expected);
    let sequences: Vec<u64> = applied
        .iter()
        .map(|event| event.sequence.0.into_inner())
        .collect();
    assert!(sequences.windows(2).all(|pair| pair[0] < pair[1]));
}
