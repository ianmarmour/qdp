//! The receiving end of one audio stream (Section 7.6): an adaptive jitter
//! buffer that puts packets back in order, conceals lost ones, and resamples
//! gently to follow the sender's clock.

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use qdp_codec::{AudioDecoder, AudioFormat};
use tracing::debug;

use crate::audio::SAMPLE_RATE;

/// The smallest target depth, in seconds (Section 7.6).
const MIN_TARGET_SECONDS: f64 = 0.005;

/// The largest target depth, in seconds, for paths with heavy jitter.
const MAX_TARGET_SECONDS: f64 = 0.08;

/// The jitter estimate's weight for each new sample, as RFC 3550's 1/16.
const JITTER_GAIN: f64 = 1.0 / 16.0;

/// How many jitter estimates the target depth covers.
const JITTER_MARGIN: f64 = 3.0;

/// The largest playout rate change for clock drift: 0.2 %, far beyond real
/// clock differences and too small to hear.
const MAX_SKEW: f64 = 0.002;

/// The depth, as a multiple of the target, above which excess audio is
/// dropped at once rather than played faster.
const FLUSH_FACTOR: usize = 4;

/// The most packets held for reordering.
const MAX_PENDING: usize = 256;

/// The longest silence, in seconds, inserted for a gap in Sample Position.
const MAX_GAP_SECONDS: u64 = 1;

/// A packet waiting to be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pending {
    /// Its first sample's index since the stream began.
    position: u64,
    /// Its encoded frame.
    payload: Vec<u8>,
}

/// What a receiver has seen so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ReceiverStats {
    /// Packets decoded.
    pub(crate) decoded: u64,
    /// Lost packets replaced by concealment or forward error correction.
    pub(crate) concealed: u64,
    /// Packets that arrived after their time, or twice.
    pub(crate) late: u64,
    /// Times playout ran dry and started buffering again.
    pub(crate) underruns: u64,
}

/// The jitter buffer and decoder of one audio stream.
pub(crate) struct AudioReceiver {
    /// The decoder.
    decoder: Box<dyn AudioDecoder>,
    /// The stream's format.
    format: AudioFormat,
    /// Packets waiting, by Sequence Number.
    pending: BTreeMap<u64, Pending>,
    /// The Sequence Number decoded next.
    next_sequence: Option<u64>,
    /// The Sample Position the next decoded sample has.
    next_position: Option<u64>,
    /// Decoded samples waiting for playout, interleaved.
    decoded: VecDeque<f32>,
    /// Whether playout is running, rather than filling up to the target.
    playing: bool,
    /// The depth to keep, in samples per channel.
    target: usize,
    /// The most samples per channel the device has taken in one pull.
    largest_pull: usize,
    /// The jitter estimate, in seconds.
    jitter: f64,
    /// The previous packet's transit time, in seconds, against `epoch`.
    last_transit: Option<f64>,
    /// When the receiver started, which arrival times count from.
    epoch: Instant,
    /// The playout position between two decoded samples, as a fraction.
    phase: f64,
    /// What the receiver has seen.
    stats: ReceiverStats,
}

impl std::fmt::Debug for AudioReceiver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AudioReceiver")
            .field("format", &self.format)
            .field("target", &self.target)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl AudioReceiver {
    /// Returns a receiver for a stream of `format`, decoding with `decoder`.
    pub(crate) fn new(format: AudioFormat, decoder: Box<dyn AudioDecoder>, now: Instant) -> Self {
        let mut receiver = Self {
            decoder,
            format,
            pending: BTreeMap::new(),
            next_sequence: None,
            next_position: None,
            decoded: VecDeque::new(),
            playing: false,
            target: 0,
            largest_pull: 0,
            jitter: 0.0,
            last_transit: None,
            epoch: now,
            phase: 0.0,
            stats: ReceiverStats::default(),
        };
        receiver.update_target();
        receiver
    }

    /// Returns what the receiver has seen.
    pub(crate) fn stats(&self) -> ReceiverStats {
        self.stats
    }

    /// Returns the depth the receiver keeps, in samples per channel.
    #[cfg(test)]
    pub(crate) fn target(&self) -> usize {
        self.target
    }

    /// Returns the channels of the stream.
    fn channels(&self) -> usize {
        usize::from(self.format.channels)
    }

    /// Returns the audio waiting, decoded or not, in samples per channel.
    pub(crate) fn depth(&self) -> usize {
        self.decoded.len() / self.channels() + self.pending.len() * self.format.frame_samples
    }

    /// Takes a packet that arrived at `now`.
    pub(crate) fn push(&mut self, sequence: u64, position: u64, payload: &[u8], now: Instant) {
        if self.next_sequence.is_some_and(|next| sequence < next)
            || self.pending.contains_key(&sequence)
        {
            self.stats.late += 1;
            return;
        }
        let arrival = now.saturating_duration_since(self.epoch).as_secs_f64();
        let transit = arrival - position as f64 / f64::from(self.format.sample_rate);
        if let Some(last) = self.last_transit {
            self.jitter += ((transit - last).abs() - self.jitter) * JITTER_GAIN;
            self.update_target();
        }
        self.last_transit = Some(transit);
        self.pending.insert(
            sequence,
            Pending {
                position,
                payload: payload.to_vec(),
            },
        );
        while self.pending.len() > MAX_PENDING {
            self.pending.pop_first();
        }
    }

    /// Sets the target depth from the jitter estimate and the device's
    /// pulls.
    ///
    /// The depth is measured as a pull starts, and the pull takes its
    /// samples at once, so the target holds what the device takes beyond a
    /// frame on top of the margin for the network's jitter: a device that
    /// takes 40 ms a time, as Android's does outside its low-latency mode,
    /// would otherwise empty the buffer with each pull.
    fn update_target(&mut self) {
        let rate = f64::from(self.format.sample_rate);
        let frame = self.format.frame_samples as f64;
        let wanted = frame + JITTER_MARGIN * self.jitter * rate;
        let smallest = (MIN_TARGET_SECONDS * rate).max(frame * 1.5);
        let pull_beyond_frame = self.largest_pull.saturating_sub(self.format.frame_samples);
        self.target =
            wanted.clamp(smallest, MAX_TARGET_SECONDS * rate) as usize + pull_beyond_frame;
    }

    /// Fills `output` with interleaved samples to play, or silence while the
    /// buffer fills.
    pub(crate) fn pull(&mut self, output: &mut [f32]) {
        let channels = self.channels();
        let frames = output.len() / channels;
        if frames > self.largest_pull {
            self.largest_pull = frames;
            self.update_target();
        }
        if !self.playing {
            if self.depth() < self.target {
                output.fill(0.0);
                return;
            }
            self.playing = true;
        }
        self.flush_excess();
        // The stream's samples are played at the devices' rate.
        let ratio =
            self.playout_ratio() * f64::from(self.format.sample_rate) / f64::from(SAMPLE_RATE);
        for frame in 0..frames {
            let index = self.phase as usize;
            if !self.ensure_decoded(index + 2) {
                output[frame * channels..].fill(0.0);
                self.playing = false;
                self.stats.underruns += 1;
                debug!(?self.stats, "audio buffer ran dry");
                break;
            }
            let fraction = (self.phase - index as f64) as f32;
            for channel in 0..channels {
                let current = self.decoded[index * channels + channel];
                let following = self.decoded[(index + 1) * channels + channel];
                output[frame * channels + channel] = current + (following - current) * fraction;
            }
            self.phase += ratio;
        }
        let consumed = (self.phase as usize).min(self.decoded.len() / channels);
        self.decoded.drain(..consumed * channels);
        self.phase -= consumed as f64;
    }

    /// Returns how fast to play: a little faster above the target depth and
    /// a little slower below it, to follow the sender's clock.
    fn playout_ratio(&self) -> f64 {
        let error = (self.depth() as f64 - self.target as f64) / self.target.max(1) as f64;
        1.0 + error.clamp(-1.0, 1.0) * MAX_SKEW
    }

    /// Drops audio far beyond the target, as after a burst of late packets,
    /// keeping the newest.
    fn flush_excess(&mut self) {
        while self.depth() > self.target * FLUSH_FACTOR && !self.pending.is_empty() {
            let channels = self.channels();
            if self.decoded.len() >= self.format.frame_len() {
                self.decoded.drain(..self.format.frame_len());
            } else if let Some((sequence, packet)) = self.pending.pop_first() {
                self.decoded.clear();
                self.next_sequence = Some(sequence + 1);
                self.next_position = Some(packet.position + self.format.frame_samples as u64);
            }
            debug_assert_eq!(self.decoded.len() % channels, 0);
        }
    }

    /// Decodes until `frames` samples per channel are waiting, returning
    /// whether there were packets enough.
    fn ensure_decoded(&mut self, frames: usize) -> bool {
        while self.decoded.len() / self.channels() < frames {
            if !self.decode_next() {
                return false;
            }
        }
        true
    }

    /// Decodes the next packet, or conceals it if it is lost, returning
    /// whether anything was decoded.
    fn decode_next(&mut self) -> bool {
        let Some(next) = self
            .next_sequence
            .or_else(|| self.pending.keys().next().copied())
        else {
            return false;
        };
        let mut samples = Vec::with_capacity(self.format.frame_len());
        if let Some(packet) = self.pending.remove(&next) {
            self.insert_gap(packet.position);
            if let Err(error) = self.decoder.decode(&packet.payload, &mut samples) {
                debug!(%error, "undecodable audio packet concealed");
                samples.clear();
                let _ = self.decoder.conceal(None, &mut samples);
                self.stats.concealed += 1;
            } else {
                self.stats.decoded += 1;
            }
            self.next_position = Some(packet.position + self.format.frame_samples as u64);
        } else if self.pending.keys().any(|&sequence| sequence > next) {
            // The packet is lost: recover it from the next one's forward error
            // correction where it has any, and conceal it otherwise.
            let following = self
                .pending
                .get(&(next + 1))
                .map(|packet| packet.payload.as_slice());
            let _ = self.decoder.conceal(following, &mut samples);
            self.stats.concealed += 1;
            self.next_position = self
                .next_position
                .map(|position| position + self.format.frame_samples as u64);
        } else {
            return false;
        }
        samples.resize(self.format.frame_len(), 0.0);
        self.decoded.extend(samples);
        self.next_sequence = Some(next + 1);
        true
    }

    /// Inserts silence for the time the sender sent nothing before a packet
    /// at `position` (Section 7.4).
    fn insert_gap(&mut self, position: u64) {
        let Some(expected) = self.next_position else {
            return;
        };
        let gap = position.saturating_sub(expected);
        let longest = MAX_GAP_SECONDS * u64::from(self.format.sample_rate);
        let silence = gap.min(longest) as usize * self.channels();
        self.decoded.extend(std::iter::repeat_n(0.0, silence));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use qdp::message::AudioCodec;
    use qdp_codec::open_audio_decoder;

    use super::*;

    /// Mono L16 with 48-sample (1 ms) frames, so that tests can count samples.
    const FORMAT: AudioFormat = AudioFormat {
        codec: AudioCodec::PCM_L16,
        sample_rate: 48_000,
        channels: 1,
        frame_samples: 48,
    };

    /// Returns a packet whose samples all hold `value`.
    fn packet(value: i16) -> Vec<u8> {
        value.to_be_bytes().repeat(FORMAT.frame_samples)
    }

    /// Returns a receiver for [`FORMAT`].
    fn receiver(now: Instant) -> AudioReceiver {
        AudioReceiver::new(FORMAT, open_audio_decoder(FORMAT).unwrap(), now)
    }

    /// Returns the value of a decoded sample, as the packet held it.
    fn value(sample: f32) -> i16 {
        (sample * 32_767.0).round() as i16
    }

    /// Packets are played in order once the target depth is buffered,
    /// whatever order they arrive in.
    #[test]
    fn reorders_packets() {
        let now = Instant::now();
        let mut receiver = receiver(now);
        let target_packets = receiver.target().div_ceil(FORMAT.frame_samples);
        let mut output = vec![0.0; FORMAT.frame_samples];
        receiver.pull(&mut output);
        assert!(
            output.iter().all(|&sample| sample == 0.0),
            "silent while filling"
        );
        for sequence in (1..=target_packets as u64 + 2).rev() {
            let position = (sequence - 1) * FORMAT.frame_samples as u64;
            receiver.push(sequence, position, &packet(sequence as i16), now);
        }
        receiver.pull(&mut output);
        assert_eq!(value(output[0]), 1);
        receiver.pull(&mut output);
        assert_eq!(value(output[1]), 2);
        assert_eq!(receiver.stats().decoded, 3);
    }

    /// A lost packet is concealed, and late and duplicate packets are
    /// dropped.
    #[test]
    fn conceals_lost_packets() {
        let now = Instant::now();
        let mut receiver = receiver(now);
        for sequence in [1_u64, 2, 4, 5, 6, 7, 8, 9, 10] {
            let position = (sequence - 1) * FORMAT.frame_samples as u64;
            receiver.push(sequence, position, &packet(100), now);
        }
        let mut output = vec![0.0; FORMAT.frame_samples * 4];
        receiver.pull(&mut output);
        let lost = &output[2 * FORMAT.frame_samples..3 * FORMAT.frame_samples];
        assert!(lost.contains(&0.0), "PCM conceals with silence");
        assert_eq!(receiver.stats().concealed, 1);
        receiver.push(3, 2 * FORMAT.frame_samples as u64, &packet(100), now);
        receiver.push(5, 4 * FORMAT.frame_samples as u64, &packet(100), now);
        assert_eq!(receiver.stats().late, 2);
    }

    /// Playout stops when the buffer runs dry and starts again at the
    /// target depth.
    #[test]
    fn recovers_from_underruns() {
        let now = Instant::now();
        let mut receiver = receiver(now);
        let target_packets = receiver.target().div_ceil(FORMAT.frame_samples) as u64;
        for sequence in 1..=target_packets {
            receiver.push(sequence, (sequence - 1) * 48, &packet(7), now);
        }
        // Playout starts, then a pull takes more than is left.
        let mut output = vec![0.0; FORMAT.frame_samples];
        receiver.pull(&mut output);
        let mut output = vec![0.0; receiver.target() * 2];
        receiver.pull(&mut output);
        assert_eq!(receiver.stats().underruns, 1);
        let mut output = vec![1.0; FORMAT.frame_samples];
        receiver.pull(&mut output);
        assert!(
            output.iter().all(|&sample| sample == 0.0),
            "buffering again"
        );
    }

    /// A gap in Sample Position plays as silence.
    #[test]
    fn plays_gaps_as_silence() {
        let now = Instant::now();
        let mut receiver = receiver(now);
        receiver.push(1, 0, &packet(9), now);
        receiver.push(2, 10 * 48, &packet(9), now);
        for sequence in 3..20 {
            receiver.push(sequence, (sequence + 8) * 48, &packet(9), now);
        }
        let mut output = vec![0.0; 48 * 4];
        receiver.pull(&mut output);
        assert_eq!(value(output[10]), 9);
        assert_eq!(output[100], 0.0);
    }

    /// Steady packets that arrive faster than playout keep the depth near
    /// the target instead of growing: playout speeds up slightly.
    #[test]
    fn follows_a_faster_sender() {
        let start = Instant::now();
        let mut receiver = receiver(start);
        let mut output = vec![0.0; FORMAT.frame_samples];
        let mut sequence = 0;
        // The sender's clock runs 0.1 % fast: 1001 packets per 1000 pulls.
        for pull in 0..20_000_u64 {
            let now = start + Duration::from_millis(pull);
            while sequence * 1000 < (pull + 20) * 1001 {
                sequence += 1;
                receiver.push(sequence, (sequence - 1) * 48, &packet(1), now);
            }
            receiver.pull(&mut output);
        }
        let depth = receiver.depth();
        assert!(
            depth <= receiver.target() * 2,
            "depth {depth} against target {}",
            receiver.target()
        );
        assert_eq!(receiver.stats().underruns, 0);
    }

    /// A device that takes several frames a pull, as Android's does, plays
    /// steady packets without running dry: the target holds a pull's worth.
    #[test]
    fn keeps_a_large_pull_buffered() {
        let start = Instant::now();
        let mut receiver = receiver(start);
        // Packets of 1 ms arrive steadily; the device takes 20 ms a pull,
        // four times the smallest target.
        let frame = FORMAT.frame_samples;
        let pull_frames = 20;
        let mut output = vec![0.0; frame * pull_frames];
        let mut sequence = 0_u64;
        for packet_time in 1..=4_000_u64 {
            let now = start + Duration::from_millis(packet_time);
            sequence += 1;
            receiver.push(sequence, (sequence - 1) * frame as u64, &packet(1), now);
            if packet_time % pull_frames as u64 == 0 {
                receiver.pull(&mut output);
            }
        }
        assert!(receiver.target() >= frame * pull_frames);
        assert_eq!(receiver.stats().underruns, 0, "{:?}", receiver.stats());
    }

    /// Jittery arrivals raise the target depth.
    #[test]
    fn adapts_to_jitter() {
        let start = Instant::now();
        let mut steady = receiver(start);
        let mut jittery = receiver(start);
        for sequence in 1..200_u64 {
            let on_time = start + Duration::from_millis(sequence);
            steady.push(sequence, (sequence - 1) * 48, &packet(1), on_time);
            let late = on_time + Duration::from_millis((sequence % 3) * 8);
            jittery.push(sequence, (sequence - 1) * 48, &packet(1), late);
        }
        assert!(jittery.target() > steady.target());
        assert!(steady.target() as f64 >= MIN_TARGET_SECONDS * 48_000.0);
    }
}
