//! Audio streams (Section 7): the devices a session plays and captures
//! through, the threads that send and play each stream, and the host's
//! choice of stream parameters.

use std::any::Any;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bytes::Bytes;
use qdp::message::{
    AudioCodec, AudioCodecCapability, AudioConfig, AudioConfigId, AudioDirection, AudioPacket,
    AudioRequest, AudioStreamConfig, Capability, DatagramPayload, Directions, StreamId,
};
use qdp::session::Negotiated;
use qdp::time::SessionClock;
use qdp::wire::VarInt;
use qdp_codec::{
    AudioContent, AudioFormat, OPUS_MAX_CHANNELS, OPUS_SAMPLE_RATE, PCM_MAX_CHANNELS,
    open_audio_decoder, open_audio_encoder,
};
use tracing::{debug, info, warn};

use crate::RuntimeError;
use crate::audio_receiver::AudioReceiver;
use crate::stream_ids::StreamIds;

/// The sample rate of every stream and device: Opus requires 48 kHz, and
/// PCM allows it (Section 7.3).
pub const SAMPLE_RATE: u32 = OPUS_SAMPLE_RATE;

/// The duration of each Opus packet, and the longest PCM packet: 10 ms, the
/// shortest Opus frame that carries forward error correction (Section 7.5.1).
const FRAME_MILLISECONDS: u32 = 10;

/// The samples per channel in one millisecond.
const SAMPLES_PER_MILLISECOND: usize = (SAMPLE_RATE / 1000) as usize;

/// The microseconds in one millisecond.
const MICROSECONDS_PER_MILLISECOND: u32 = 1000;

/// The largest packet of a capture stream, so that it fits any QUIC path
/// (Section 7.4).
const CAPTURE_DATAGRAM_LIMIT: usize = 1200;

/// The bytes the fields of an `AUDIO_PACKET` take at most, before its payload.
const PACKET_HEADER_BOUND: usize = 1 + 8 + 8 + 8 + 8;

/// The captured chunks queued between a device and its sender thread.
const CAPTURE_QUEUE: usize = 64;

/// How long the sender thread waits for captured audio before checking
/// whether it should stop.
const CAPTURE_WAIT: Duration = Duration::from_millis(100);

/// The loss the microphone's forward error correction is tuned for, in
/// percent, until the connection has measured its own.
const CAPTURE_EXPECTED_LOSS: u8 = 10;

/// How often a stream with forward error correction tunes it to the loss
/// the connection observes (Section 7.5.1).
const LOSS_INTERVAL: Duration = Duration::from_secs(1);

/// The sample rate PCM streams may have besides [`SAMPLE_RATE`] (Section
/// 7.3).
const PCM_CD_RATE: u32 = 44_100;

/// The percentage of a whole.
const PERCENT: u64 = 100;

/// The names of the audio threads, as debuggers and profilers show them.
const SENDER_THREAD: &str = "qdp-audio-send";

/// The name of the audio playback thread.
const PLAYER_THREAD: &str = "qdp-audio-play";

/// How long playback waits before it starts again on the default device,
/// once its device stopped or could not start.
const PLAYBACK_RETRY: Duration = Duration::from_millis(500);

/// Keeps an audio device capturing or playing until it is dropped. It stays
/// on the thread that started the device, which some platforms require.
pub struct ActiveDevice(pub Box<dyn Any>);

impl std::fmt::Debug for ActiveDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveDevice")
            .finish_non_exhaustive()
    }
}

/// Where a capture device delivers its samples: 32-bit floats from -1 to 1,
/// interleaved, at [`SAMPLE_RATE`].
#[derive(Debug, Clone)]
pub struct CaptureSink(SyncSender<Vec<f32>>);

impl CaptureSink {
    /// Delivers captured samples. Samples that the sender cannot keep up with
    /// are dropped rather than waited for, so that capture never blocks.
    pub fn push(&self, samples: &[f32]) {
        let _ = self.0.try_send(samples.to_vec());
    }
}

/// Where a playback device takes its samples from, and tells the player
/// that it stopped.
#[derive(Debug, Clone)]
pub struct Playout {
    /// The stream's samples.
    receiver: Arc<Mutex<AudioReceiver>>,
    /// Tells the player thread that the device stopped.
    signals: mpsc::Sender<PlayerSignal>,
}

impl Playout {
    /// Fills `output` with the next interleaved samples at [`SAMPLE_RATE`],
    /// or silence while the stream's buffer fills.
    pub fn pull(&self, output: &mut [f32]) {
        match self.receiver.lock() {
            Ok(mut receiver) => receiver.pull(output),
            Err(_) => output.fill(0.0),
        }
    }

    /// Reports that the device stopped playing, as when the system's default
    /// output changes: playback starts again, on the default device then.
    pub fn stopped(&self, error: String) {
        let _ = self.signals.send(PlayerSignal::DeviceStopped(error));
    }
}

/// What the player thread is told.
#[derive(Debug)]
enum PlayerSignal {
    /// The session no longer plays the stream.
    Stop,
    /// The device stopped playing, for the reason.
    DeviceStopped(String),
}

/// Captures audio for a session: the host's desktop audio, or the client's
/// microphone.
pub trait AudioInput: Send {
    /// Returns the channels the device captures, from 1 to 8, in the order
    /// of RFC 7845's mapping family 1.
    fn channels(&self) -> u16;

    /// Starts capturing into `capture` at [`SAMPLE_RATE`] until the returned
    /// device is dropped.
    fn start(&mut self, capture: CaptureSink) -> Result<ActiveDevice, String>;

    /// Returns which device this is, for logs.
    fn description(&self) -> &str;
}

/// Plays audio for a session: the client's speakers, or a microphone the
/// host presents to its applications.
pub trait AudioOutput: Send {
    /// Returns the most channels the device plays, from 1 to 8.
    fn channels(&self) -> u16;

    /// Starts playing `channels` channels taken from `playout` at
    /// [`SAMPLE_RATE`] until the returned device is dropped.
    fn start(&mut self, channels: u16, playout: Playout) -> Result<ActiveDevice, String>;

    /// Returns which device this is, for logs.
    fn description(&self) -> &str;
}

/// Converts interleaved samples from `from` channels to `to`: fewer channels
/// keep the front left and right, or mix to mono; more repeat mono or add
/// silence.
fn remix(samples: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to {
        return samples.to_vec();
    }
    samples
        .chunks_exact(from)
        .flat_map(|frame| {
            let mono = frame.iter().sum::<f32>() / from as f32;
            (0..to).map(move |channel| match (from, to) {
                (_, 1) => mono,
                (1, _) => frame[0],
                _ => frame.get(channel).copied().unwrap_or(0.0),
            })
        })
        .collect()
}

/// How large a stream's packets may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PacketLimit {
    /// The path's current maximum datagram size: playback streams.
    Path,
    /// 1200 bytes, the smallest maximum QUIC allows: capture streams.
    Smallest,
}

/// A stream the endpoint sends: a thread that captures, encodes and sends
/// it.
pub(crate) struct AudioSender {
    /// The stream.
    stream_id: StreamId,
    /// Ends the thread; dropping it does too.
    stop: Option<mpsc::Sender<()>>,
    /// The thread, which gives the input back when it ends.
    thread: Option<JoinHandle<Box<dyn AudioInput>>>,
}

impl AudioSender {
    /// Starts sending `stream_id`, of `format`, captured from `input`.
    fn spawn(
        mut input: Box<dyn AudioInput>,
        stream_id: StreamId,
        format: AudioFormat,
        (content, limit): (AudioContent, PacketLimit),
        connection: quinn::Connection,
        clock: SessionClock,
        active: SessionActive,
    ) -> Result<Self, RuntimeError> {
        let mut encoder = open_audio_encoder(format, content)?;
        if content == AudioContent::Speech {
            encoder.set_expected_loss(CAPTURE_EXPECTED_LOSS);
        }
        let (stop, stopped) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name(SENDER_THREAD.to_owned())
            .spawn(move || {
                let (sink, captured) = mpsc::sync_channel(CAPTURE_QUEUE);
                match input.start(CaptureSink(sink)) {
                    Ok(device) => {
                        let channels = usize::from(format.channels);
                        let mut stream = SendLoop {
                            stream_id,
                            format,
                            limit,
                            loss: (content == AudioContent::Speech)
                                .then(|| LossMeter::new(&connection, Instant::now())),
                            connection,
                            clock,
                            active,
                            input_channels: usize::from(input.channels()),
                            resampler: (format.sample_rate != SAMPLE_RATE)
                                .then(|| Resampler::new(SAMPLE_RATE, format.sample_rate, channels)),
                            pending: Vec::new(),
                            sequence: 0,
                            position: 0,
                        };
                        stream.run(&mut *encoder, &captured, &stopped);
                        drop(device);
                    }
                    Err(error) => {
                        warn!(error, device = input.description(), "audio capture failed")
                    }
                }
                input
            })
            .map_err(|error| RuntimeError::Audio(error.to_string()))?;
        Ok(Self {
            stream_id,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// Returns the stream it sends.
    pub(crate) fn stream_id(&self) -> StreamId {
        self.stream_id
    }

    /// Stops sending and gives the input back.
    pub(crate) fn stop(mut self) -> Option<Box<dyn AudioInput>> {
        self.stop = None;
        self.thread.take()?.join().ok()
    }
}

impl Drop for AudioSender {
    fn drop(&mut self) {
        self.stop = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Whether a session is active, shared with its sender threads.
#[derive(Debug, Clone, Default)]
pub(crate) struct SessionActive(Arc<AtomicBool>);

impl SessionActive {
    /// Returns a flag that is already set.
    pub(crate) fn active() -> Self {
        Self(Arc::new(AtomicBool::new(true)))
    }

    /// Sets the flag.
    pub(crate) fn activate(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Returns whether the flag is set.
    fn is_active(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// The state of a sender thread.
struct SendLoop {
    /// The stream.
    stream_id: StreamId,
    /// The stream's format.
    format: AudioFormat,
    /// How large packets may be.
    limit: PacketLimit,
    /// Sends the datagrams.
    connection: quinn::Connection,
    /// The session clock that timestamps packets.
    clock: SessionClock,
    /// Whether the session is active, so that packets may be sent.
    active: SessionActive,
    /// The channels the device captures.
    input_channels: usize,
    /// Converts the devices' rate to the stream's, if they differ.
    resampler: Option<Resampler>,
    /// Measures the connection's loss, for a stream whose encoder protects
    /// packets against it.
    loss: Option<LossMeter>,
    /// Captured samples not yet sent, remixed to the stream's channels.
    pending: Vec<f32>,
    /// The Sequence Number of the last packet.
    sequence: u64,
    /// The Sample Position of the next packet.
    position: u64,
}

impl SendLoop {
    /// Sends what the device captures until told to stop.
    fn run(
        &mut self,
        encoder: &mut dyn qdp_codec::AudioEncoder,
        captured: &Receiver<Vec<f32>>,
        stopped: &Receiver<()>,
    ) {
        loop {
            match stopped.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {}
                _ => return,
            }
            if let Some(percent) = self
                .loss
                .as_mut()
                .and_then(|meter| meter.poll(&self.connection, Instant::now()))
            {
                encoder.set_expected_loss(percent);
            }
            match captured.recv_timeout(CAPTURE_WAIT) {
                Ok(chunk) => {
                    let channels = usize::from(self.format.channels);
                    let remixed = remix(&chunk, self.input_channels, channels);
                    match &mut self.resampler {
                        Some(resampler) => self.pending.extend(resampler.process(&remixed)),
                        None => self.pending.extend(remixed),
                    }
                    while self.pending.len() >= self.format.frame_len() {
                        let frame: Vec<f32> =
                            self.pending.drain(..self.format.frame_len()).collect();
                        // The frame's first sample was captured before the
                        // frame and everything still waiting.
                        let queued = self.pending.len() / channels + self.format.frame_samples;
                        let age = Duration::from_secs_f64(
                            queued as f64 / f64::from(self.format.sample_rate),
                        );
                        let captured_at =
                            Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
                        self.send(encoder, &frame, captured_at);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Encodes and sends one frame.
    fn send(
        &mut self,
        encoder: &mut dyn qdp_codec::AudioEncoder,
        frame: &[f32],
        captured_at: Instant,
    ) {
        let path = self
            .connection
            .max_datagram_size()
            .unwrap_or(CAPTURE_DATAGRAM_LIMIT);
        let datagram_limit = match self.limit {
            PacketLimit::Path => path,
            PacketLimit::Smallest => path.min(CAPTURE_DATAGRAM_LIMIT),
        };
        let max_payload = datagram_limit.saturating_sub(PACKET_HEADER_BOUND);
        let payload = match encoder.encode(frame, max_payload) {
            Ok(payload) => payload,
            Err(error) => {
                debug!(%error, "audio frame not encoded");
                return;
            }
        };
        self.sequence += 1;
        let packet = AudioPacket {
            stream_id: self.stream_id,
            sequence_number: VarInt::new(self.sequence).unwrap_or(VarInt::MAX),
            sample_position: VarInt::new(self.position).unwrap_or(VarInt::MAX),
            capture_timestamp: self.clock.timestamp(captured_at),
            payload: &payload,
        };
        self.position += self.format.frame_samples as u64;
        // Datagrams are sent only once the session is active (Section 4.2).
        if !self.active.is_active() {
            return;
        }
        if let Err(error) = self
            .connection
            .send_datagram(Bytes::from(packet.to_bytes()))
        {
            debug!(%error, "audio packet dropped");
        }
    }
}

/// Converts interleaved audio from one sample rate to another by linear
/// interpolation, which suits 44.1 and 48 kHz: they differ by under a tenth.
struct Resampler {
    /// The channels.
    channels: usize,
    /// The input samples, per channel, for each output sample.
    step: f64,
    /// Where the next output sample falls, in input samples per channel from
    /// the start of `held`.
    phase: f64,
    /// The input not yet passed, which keeps the last sample to interpolate
    /// across chunks.
    held: Vec<f32>,
}

impl Resampler {
    /// Returns a resampler of `channels` channels from `from` hertz to `to`.
    fn new(from: u32, to: u32, channels: usize) -> Self {
        Self {
            channels,
            step: f64::from(from) / f64::from(to),
            phase: 0.0,
            held: Vec::new(),
        }
    }

    /// Returns the output for the next chunk of input.
    fn process(&mut self, input: &[f32]) -> Vec<f32> {
        self.held.extend_from_slice(input);
        let channels = self.channels.max(1);
        let frames = self.held.len() / channels;
        let mut output = Vec::new();
        while self.phase + 1.0 < frames as f64 {
            let index = self.phase as usize;
            let fraction = (self.phase - index as f64) as f32;
            for channel in 0..channels {
                let current = self.held[index * channels + channel];
                let following = self.held[(index + 1) * channels + channel];
                output.push(current + (following - current) * fraction);
            }
            self.phase += self.step;
        }
        let consumed = (self.phase as usize).min(frames);
        self.held.drain(..consumed * channels);
        self.phase -= consumed as f64;
        output
    }
}

/// Measures the loss of the connection's packets, which the stream's
/// encoder protects against (Section 7.5.1).
struct LossMeter {
    /// When the counts were last read.
    measured_at: Instant,
    /// The packets lost by then.
    lost: u64,
    /// The packets sent by then.
    sent: u64,
}

impl LossMeter {
    /// Starts measuring `connection` at `now`.
    fn new(connection: &quinn::Connection, now: Instant) -> Self {
        let path = connection.stats().path;
        Self {
            measured_at: now,
            lost: path.lost_packets,
            sent: path.sent_packets,
        }
    }

    /// Returns the percentage of packets lost since the last measurement,
    /// once one is due.
    fn poll(&mut self, connection: &quinn::Connection, now: Instant) -> Option<u8> {
        if now.saturating_duration_since(self.measured_at) < LOSS_INTERVAL {
            return None;
        }
        let path = connection.stats().path;
        let percent = loss_percent(
            path.lost_packets.saturating_sub(self.lost),
            path.sent_packets.saturating_sub(self.sent),
        );
        *self = Self {
            measured_at: now,
            lost: path.lost_packets,
            sent: path.sent_packets,
        };
        Some(percent)
    }
}

/// Returns `lost` of `sent` packets as a percentage, rounded up so that
/// any loss is protected against.
fn loss_percent(lost: u64, sent: u64) -> u8 {
    if sent == 0 {
        return 0;
    }
    u8::try_from((lost * PERCENT).div_ceil(sent).min(PERCENT)).unwrap_or(u8::MAX)
}

/// A stream the endpoint plays: its jitter buffer, and a thread that keeps
/// the output device playing from it.
pub(crate) struct AudioPlayer {
    /// The stream.
    stream_id: StreamId,
    /// The jitter buffer the device pulls from.
    receiver: Arc<Mutex<AudioReceiver>>,
    /// Signals the thread.
    signals: mpsc::Sender<PlayerSignal>,
    /// The thread, which gives the output back when it ends.
    thread: Option<JoinHandle<Box<dyn AudioOutput>>>,
}

impl AudioPlayer {
    /// Starts playing `stream_id`, of `format`, through `output`.
    fn spawn(
        mut output: Box<dyn AudioOutput>,
        stream_id: StreamId,
        format: AudioFormat,
    ) -> Result<Self, RuntimeError> {
        let decoder = open_audio_decoder(format)?;
        let receiver = Arc::new(Mutex::new(AudioReceiver::new(
            format,
            decoder,
            Instant::now(),
        )));
        let (signals, signalled) = mpsc::channel();
        let playout = Playout {
            receiver: Arc::clone(&receiver),
            signals: signals.clone(),
        };
        let thread = std::thread::Builder::new()
            .name(PLAYER_THREAD.to_owned())
            .spawn(move || {
                play(output.as_mut(), format.channels, &playout, &signalled);
                output
            })
            .map_err(|error| RuntimeError::Audio(error.to_string()))?;
        Ok(Self {
            stream_id,
            receiver,
            signals,
            thread: Some(thread),
        })
    }

    /// Returns the stream it plays.
    pub(crate) fn stream_id(&self) -> StreamId {
        self.stream_id
    }

    /// Takes a packet of the stream.
    pub(crate) fn push(&self, packet: &AudioPacket<'_>, now: Instant) {
        if let Ok(mut receiver) = self.receiver.lock() {
            receiver.push(
                packet.sequence_number.into_inner(),
                packet.sample_position.into_inner(),
                packet.payload,
                now,
            );
        }
    }

    /// Stops playing and gives the output back.
    pub(crate) fn stop(mut self) -> Option<Box<dyn AudioOutput>> {
        if let Ok(receiver) = self.receiver.lock() {
            debug!(stats = ?receiver.stats(), "audio stream ended");
        }
        let _ = self.signals.send(PlayerSignal::Stop);
        self.thread.take()?.join().ok()
    }
}

impl Drop for AudioPlayer {
    /// Stops the thread without waiting for it: a device that does not
    /// answer, as Core Audio can take seconds to fail to start one, must
    /// not hold up the session's end.
    fn drop(&mut self) {
        let _ = self.signals.send(PlayerSignal::Stop);
    }
}

/// Plays `channels` channels from `playout` through `output` until told to
/// stop, starting the device again, on the default device then, whenever it
/// stops or cannot start.
fn play(
    output: &mut dyn AudioOutput,
    channels: u16,
    playout: &Playout,
    signalled: &Receiver<PlayerSignal>,
) {
    // The last failure to start, which is not logged again while it repeats.
    let mut last_failure: Option<String> = None;
    loop {
        let device = match output.start(channels, playout.clone()) {
            Ok(device) => {
                last_failure = None;
                Some(device)
            }
            Err(error) => {
                if last_failure.as_ref() != Some(&error) {
                    warn!(
                        error,
                        device = output.description(),
                        "audio playback failed; trying again"
                    );
                }
                last_failure = Some(error);
                None
            }
        };
        let signal = match device {
            Some(_) => signalled.recv().ok(),
            None => match signalled.recv_timeout(PLAYBACK_RETRY) {
                Ok(signal) => Some(signal),
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => None,
            },
        };
        drop(device);
        match signal {
            Some(PlayerSignal::DeviceStopped(error)) => {
                warn!(
                    error,
                    device = output.description(),
                    "audio playback stopped; starting it again on the system's output"
                );
                // The device may be gone a moment, and report its end more
                // than once.
                std::thread::sleep(PLAYBACK_RETRY);
                while let Ok(signal) = signalled.try_recv() {
                    if matches!(signal, PlayerSignal::Stop) {
                        return;
                    }
                }
            }
            Some(PlayerSignal::Stop) | None => return,
        }
    }
}

/// Returns the audio codec capabilities of an endpoint that can send audio
/// of `send` channels and receive audio of `receive` channels, if any.
pub(crate) fn capabilities(send: Option<u16>, receive: Option<u16>) -> Vec<Capability> {
    let directions = Directions(
        send.map_or(0, |_| Directions::SEND.0) | receive.map_or(0, |_| Directions::RECEIVE.0),
    );
    if directions.0 == 0 {
        return Vec::new();
    }
    [
        (AudioCodec::OPUS, OPUS_MAX_CHANNELS),
        (AudioCodec::PCM_L16, PCM_MAX_CHANNELS),
        (AudioCodec::PCM_L24, PCM_MAX_CHANNELS),
    ]
    .into_iter()
    .map(|(codec, max_channels)| {
        Capability::AudioCodec(AudioCodecCapability {
            codec,
            max_channels: VarInt::from(u32::from(max_channels)),
            directions,
        })
    })
    .collect()
}

/// Returns the format of a stream of `channels` channels in the first codec
/// of `codecs` that carries them in packets of at most `max_payload` bytes.
/// Opus packets are 10 ms; PCM packets are the longest whole number of
/// milliseconds, up to 10, that fits.
fn choose_format(codecs: &[AudioCodec], channels: u16, max_payload: usize) -> Option<AudioFormat> {
    let preference = [AudioCodec::OPUS, AudioCodec::PCM_L16, AudioCodec::PCM_L24];
    preference
        .into_iter()
        .filter(|codec| codecs.contains(codec))
        .find_map(|codec| {
            let ten_milliseconds = FRAME_MILLISECONDS as usize * SAMPLES_PER_MILLISECOND;
            if codec == AudioCodec::OPUS {
                return (channels <= OPUS_MAX_CHANNELS).then_some(AudioFormat {
                    codec,
                    sample_rate: SAMPLE_RATE,
                    channels,
                    frame_samples: ten_milliseconds,
                });
            }
            (1..=FRAME_MILLISECONDS as usize)
                .rev()
                .map(|milliseconds| AudioFormat {
                    codec,
                    sample_rate: SAMPLE_RATE,
                    channels,
                    frame_samples: milliseconds * SAMPLES_PER_MILLISECOND,
                })
                .find(|format| {
                    format
                        .pcm_packet_bytes()
                        .is_some_and(|bytes| bytes <= max_payload)
                })
        })
}

/// Returns a stream's description in `AUDIO_CONFIG`.
fn stream_config(
    stream_id: StreamId,
    direction: AudioDirection,
    format: AudioFormat,
) -> AudioStreamConfig {
    let frame_duration =
        format.frame_samples as u32 * MICROSECONDS_PER_MILLISECOND / SAMPLES_PER_MILLISECOND as u32;
    AudioStreamConfig {
        stream_id,
        direction,
        codec: format.codec,
        sample_rate: VarInt::from(format.sample_rate),
        channel_count: VarInt::from(u32::from(format.channels)),
        frame_duration: VarInt::from(frame_duration),
        opus_mapping: None,
    }
}

/// Returns a stream that started, or `None`, with a warning, if it could
/// not: the session goes on with fewer streams, as the host's answer to
/// every request does (Section 7.3).
fn started<T>(direction: AudioDirection, stream: Result<T, RuntimeError>) -> Option<T> {
    stream
        .inspect_err(|error| warn!(%error, ?direction, "audio stream not started"))
        .ok()
}

/// Returns the format of a stream described in `AUDIO_CONFIG`, if this
/// endpoint can handle it.
fn stream_format(config: &AudioStreamConfig) -> Option<AudioFormat> {
    let sample_rate = u32::try_from(config.sample_rate.into_inner()).ok()?;
    let channels = u16::try_from(config.channel_count.into_inner()).ok()?;
    let duration = u64::from(config.frame_duration.into_inner().min(u64::from(u32::MAX)) as u32);
    let frame_samples = usize::try_from(duration * u64::from(sample_rate) / 1_000_000).ok()?;
    let rate_supported = sample_rate == SAMPLE_RATE
        || (sample_rate == PCM_CD_RATE && config.codec != AudioCodec::OPUS);
    (rate_supported && (1..=PCM_MAX_CHANNELS).contains(&channels) && frame_samples > 0).then_some(
        AudioFormat {
            codec: config.codec,
            sample_rate,
            channels,
            frame_samples,
        },
    )
}

/// The audio devices of a host.
#[derive(Default)]
pub struct HostAudio {
    /// The desktop's audio, which clients play.
    pub desktop: Option<Box<dyn AudioInput>>,
    /// A microphone the host presents to its applications, playing the
    /// client's microphone.
    pub microphone: Option<Box<dyn AudioOutput>>,
}

impl std::fmt::Debug for HostAudio {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostAudio")
            .field(
                "desktop",
                &self.desktop.as_ref().map(|input| input.description()),
            )
            .field(
                "microphone",
                &self.microphone.as_ref().map(|output| output.description()),
            )
            .finish()
    }
}

impl HostAudio {
    /// Returns the host's audio codec capabilities.
    pub(crate) fn capabilities(&self) -> Vec<Capability> {
        capabilities(
            self.desktop.as_ref().map(|input| input.channels()),
            self.microphone.as_ref().map(|output| output.channels()),
        )
    }
}

/// The host's audio streams.
pub(crate) struct HostAudioStreams {
    /// The devices not in use by a stream.
    devices: HostAudio,
    /// The playback stream being sent.
    playback: Option<AudioSender>,
    /// The playback stream's format.
    playback_format: Option<AudioFormat>,
    /// The client's microphone being played.
    capture: Option<AudioPlayer>,
    /// The Config ID of the last `AUDIO_CONFIG`.
    config_id: u64,
    /// The client's latest `AUDIO_REQUEST`.
    request: Option<AudioRequest>,
    /// Whether the session is active, so that packets may be sent.
    active: SessionActive,
}

impl HostAudioStreams {
    /// Returns the streams of a host with `devices`, none defined yet.
    pub(crate) fn new(devices: HostAudio) -> Self {
        Self {
            devices,
            playback: None,
            playback_format: None,
            capture: None,
            config_id: 0,
            request: None,
            active: SessionActive::default(),
        }
    }

    /// Lets the streams send packets: the session is active. Streams
    /// configured before `SESSION_START` capture, but send nothing until
    /// then (Section 4.2).
    pub(crate) fn activate(&self) {
        self.active.activate();
    }

    /// Answers an `AUDIO_REQUEST`: stops the current streams, defines new ones
    /// the client asked for and both endpoints can carry, with new IDs from
    /// `stream_ids`, starts them, and returns the `AUDIO_CONFIG` that
    /// describes them.
    pub(crate) fn configure(
        &mut self,
        request: &AudioRequest,
        negotiated: &Negotiated,
        connection: &quinn::Connection,
        clock: SessionClock,
        stream_ids: &mut StreamIds,
    ) -> Result<AudioConfig, RuntimeError> {
        self.request = Some(*request);
        self.playback_format = None;
        if let Some(desktop) = self.playback.take().and_then(AudioSender::stop) {
            self.devices.desktop = Some(desktop);
        }
        if let Some(microphone) = self.capture.take().and_then(AudioPlayer::stop) {
            self.devices.microphone = Some(microphone);
        }
        let mut streams = Vec::new();
        let wanted = |channels: VarInt| u16::try_from(channels.into_inner()).unwrap_or(u16::MAX);
        let playback_channels = wanted(request.playback_channels);
        if let Some(desktop) = self.devices.desktop.take() {
            let channels = playback_channels
                .min(desktop.channels())
                .min(PCM_MAX_CHANNELS);
            let path = connection
                .max_datagram_size()
                .unwrap_or(CAPTURE_DATAGRAM_LIMIT);
            let max_payload = path.saturating_sub(PACKET_HEADER_BOUND);
            let codecs = &negotiated.playback_codecs;
            let format = (channels > 0)
                .then(|| {
                    choose_format(codecs, channels, max_payload).or_else(|| {
                        choose_format(codecs, channels.min(OPUS_MAX_CHANNELS), max_payload)
                    })
                })
                .flatten();
            match format {
                Some(format) => {
                    let stream_id = stream_ids.next();
                    let kind = (AudioContent::General, PacketLimit::Path);
                    self.playback = started(
                        AudioDirection::Playback,
                        AudioSender::spawn(
                            desktop,
                            stream_id,
                            format,
                            kind,
                            connection.clone(),
                            clock,
                            self.active.clone(),
                        ),
                    );
                    if self.playback.is_some() {
                        self.playback_format = Some(format);
                        streams.push(stream_config(stream_id, AudioDirection::Playback, format));
                    }
                }
                None => self.devices.desktop = Some(desktop),
            }
        }
        let capture_channels = wanted(request.capture_channels);
        if let Some(microphone) = self.devices.microphone.take() {
            let channels = capture_channels
                .min(microphone.channels())
                .min(OPUS_MAX_CHANNELS);
            let max_payload = CAPTURE_DATAGRAM_LIMIT - PACKET_HEADER_BOUND;
            let format = (channels > 0)
                .then(|| choose_format(&negotiated.capture_codecs, channels, max_payload))
                .flatten();
            match format {
                Some(format) => {
                    let stream_id = stream_ids.next();
                    self.capture = started(
                        AudioDirection::Capture,
                        AudioPlayer::spawn(microphone, stream_id, format),
                    );
                    if self.capture.is_some() {
                        streams.push(stream_config(stream_id, AudioDirection::Capture, format));
                    }
                }
                None => self.devices.microphone = Some(microphone),
            }
        }
        self.config_id += 1;
        Ok(AudioConfig {
            config_id: AudioConfigId(VarInt::new(self.config_id).unwrap_or(VarInt::MAX)),
            streams,
        })
    }

    /// Returns a new `AUDIO_CONFIG` if the path's maximum datagram size
    /// shrank so far that the playback stream's PCM packets no longer fit:
    /// its streams are configured again for the client's latest request
    /// (Section 7.4). Opus packets always fit, since each is limited to the
    /// space the path leaves.
    pub(crate) fn check_path(
        &mut self,
        negotiated: &Negotiated,
        connection: &quinn::Connection,
        clock: SessionClock,
        stream_ids: &mut StreamIds,
    ) -> Result<Option<AudioConfig>, RuntimeError> {
        let (Some(format), Some(request)) = (self.playback_format, self.request) else {
            return Ok(None);
        };
        let Some(packet) = format.pcm_packet_bytes() else {
            return Ok(None);
        };
        let max_payload = connection
            .max_datagram_size()
            .unwrap_or(CAPTURE_DATAGRAM_LIMIT)
            .saturating_sub(PACKET_HEADER_BOUND);
        if packet <= max_payload {
            return Ok(None);
        }
        info!(max_payload, "audio packets no longer fit the path");
        self.configure(&request, negotiated, connection, clock, stream_ids)
            .map(Some)
    }

    /// Takes a packet of the client's microphone.
    pub(crate) fn handle_packet(&self, packet: &AudioPacket<'_>, now: Instant) {
        if let Some(capture) = self
            .capture
            .as_ref()
            .filter(|capture| capture.stream_id() == packet.stream_id)
        {
            capture.push(packet, now);
        }
    }
}

/// The audio devices of a client.
#[derive(Default)]
pub struct ClientAudio {
    /// The speakers the host's audio plays on.
    pub speakers: Option<Box<dyn AudioOutput>>,
    /// The microphone, which the host receives only while it is on.
    pub microphone: Option<Box<dyn AudioInput>>,
    /// Whether the user turned the microphone on for the session, as the
    /// session starts (Section 7.7).
    pub microphone_state: Microphone,
}

/// Whether the user turned the client's microphone on for the session
/// (Section 7.7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Microphone {
    /// The microphone is off: nothing is captured.
    #[default]
    Off,
    /// The microphone is on: the host hears it.
    On,
}

impl std::fmt::Debug for ClientAudio {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClientAudio")
            .field(
                "speakers",
                &self.speakers.as_ref().map(|output| output.description()),
            )
            .field(
                "microphone",
                &self.microphone.as_ref().map(|input| input.description()),
            )
            .field("microphone_state", &self.microphone_state)
            .finish()
    }
}

impl ClientAudio {
    /// Returns the client's audio codec capabilities.
    pub(crate) fn capabilities(&self) -> Vec<Capability> {
        capabilities(
            self.microphone.as_ref().map(|input| input.channels()),
            self.speakers.as_ref().map(|output| output.channels()),
        )
    }

    /// Returns the channels of the speakers and the microphone, 0 for a
    /// device the client lacks.
    fn channels(&self) -> (u16, u16) {
        (
            self.speakers.as_ref().map_or(0, |output| output.channels()),
            self.microphone.as_ref().map_or(0, |input| input.channels()),
        )
    }

    /// Returns the client's first `AUDIO_REQUEST`, or `None` if it has no
    /// audio devices.
    pub(crate) fn request(&self) -> Option<AudioRequest> {
        let (playback, capture) = self.channels();
        (playback > 0 || capture > 0)
            .then(|| audio_request(playback, capture, self.microphone_state))
    }
}

/// Returns an `AUDIO_REQUEST` for speakers and a microphone of these
/// channels: the microphone's only while it is on (Section 7.2).
fn audio_request(playback: u16, capture: u16, microphone: Microphone) -> AudioRequest {
    let capture = match microphone {
        Microphone::On => capture,
        Microphone::Off => 0,
    };
    AudioRequest {
        playback_channels: VarInt::from(u32::from(playback)),
        capture_channels: VarInt::from(u32::from(capture)),
    }
}

/// The client's audio streams.
pub(crate) struct ClientAudioStreams {
    /// The devices not in use by a stream.
    devices: ClientAudio,
    /// The channels of the speakers and the microphone, 0 for a device the
    /// client lacks.
    channels: (u16, u16),
    /// The host's audio being played.
    playback: Option<AudioPlayer>,
    /// The microphone being sent.
    capture: Option<AudioSender>,
}

impl ClientAudioStreams {
    /// Returns the streams of a client with `devices`, none defined yet.
    pub(crate) fn new(devices: ClientAudio) -> Self {
        Self {
            channels: devices.channels(),
            devices,
            playback: None,
            capture: None,
        }
    }

    /// Turns the microphone on or off, returning the `AUDIO_REQUEST` to send
    /// if that changed anything (Section 7.2). Turned off, it stops sending
    /// at once, before the host's new `AUDIO_CONFIG` arrives (Section 7.7).
    pub(crate) fn set_microphone(&mut self, microphone: Microphone) -> Option<AudioRequest> {
        if microphone == self.devices.microphone_state || self.channels.1 == 0 {
            return None;
        }
        self.devices.microphone_state = microphone;
        if microphone == Microphone::Off {
            if let Some(device) = self.capture.take().and_then(AudioSender::stop) {
                self.devices.microphone = Some(device);
            }
        }
        info!(?microphone, "microphone");
        Some(audio_request(
            self.channels.0,
            self.channels.1,
            self.devices.microphone_state,
        ))
    }

    /// Applies an `AUDIO_CONFIG`: keeps the streams it still defines, stops the
    /// others, and starts the new ones the client can play or asked to send.
    pub(crate) fn configure(
        &mut self,
        config: &AudioConfig,
        connection: &quinn::Connection,
        clock: SessionClock,
    ) -> Result<(), RuntimeError> {
        let defines = |stream_id: StreamId, direction: AudioDirection| {
            config
                .streams
                .iter()
                .find(|stream| stream.stream_id == stream_id && stream.direction == direction)
        };
        if self
            .playback
            .as_ref()
            .is_some_and(|player| defines(player.stream_id(), AudioDirection::Playback).is_none())
        {
            if let Some(speakers) = self.playback.take().and_then(AudioPlayer::stop) {
                self.devices.speakers = Some(speakers);
            }
        }
        if self
            .capture
            .as_ref()
            .is_some_and(|sender| defines(sender.stream_id(), AudioDirection::Capture).is_none())
        {
            if let Some(microphone) = self.capture.take().and_then(AudioSender::stop) {
                self.devices.microphone = Some(microphone);
            }
        }
        for stream in &config.streams {
            let Some(format) = stream_format(stream) else {
                debug!(
                    stream = stream.stream_id.0.into_inner(),
                    "audio stream format not supported"
                );
                continue;
            };
            match stream.direction {
                AudioDirection::Playback if self.playback.is_none() => {
                    if let Some(speakers) = self.devices.speakers.take() {
                        self.playback = started(
                            AudioDirection::Playback,
                            AudioPlayer::spawn(speakers, stream.stream_id, format),
                        );
                    }
                }
                // A capture stream the client did not ask for, as one
                // defined before the user turned the microphone off, is
                // ignored (Section 7.7).
                AudioDirection::Capture
                    if self.capture.is_none()
                        && self.devices.microphone_state == Microphone::On =>
                {
                    if let Some(microphone) = self.devices.microphone.take() {
                        let kind = (AudioContent::Speech, PacketLimit::Smallest);
                        self.capture = started(
                            AudioDirection::Capture,
                            AudioSender::spawn(
                                microphone,
                                stream.stream_id,
                                format,
                                kind,
                                connection.clone(),
                                clock,
                                SessionActive::active(),
                            ),
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Takes a packet of the host's audio.
    pub(crate) fn handle_packet(&self, packet: &AudioPacket<'_>, now: Instant) {
        if let Some(playback) = self
            .playback
            .as_ref()
            .filter(|player| player.stream_id() == packet.stream_id)
        {
            playback.push(packet, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Channels are kept, mixed down or spread as the stream needs.
    #[test]
    fn remixes_channels() {
        let stereo = [0.5, -0.5, 1.0, 0.0];
        assert_eq!(remix(&stereo, 2, 2), stereo);
        assert_eq!(remix(&stereo, 2, 1), [0.0, 0.5]);
        assert_eq!(remix(&[0.25, 0.75], 1, 2), [0.25, 0.25, 0.75, 0.75]);
        assert_eq!(remix(&[0.1, 0.2, 0.3], 3, 2), [0.1, 0.2]);
        assert_eq!(remix(&[0.1, 0.2], 2, 3), [0.1, 0.2, 0.0]);
    }

    /// Opus is preferred; PCM packets are as long as fit.
    #[test]
    fn chooses_formats() {
        let all = [AudioCodec::OPUS, AudioCodec::PCM_L16];
        let opus = choose_format(&all, 2, 1200).unwrap();
        assert_eq!((opus.codec, opus.frame_samples), (AudioCodec::OPUS, 480));
        let pcm = choose_format(&[AudioCodec::PCM_L16], 2, 1200).unwrap();
        assert_eq!(
            pcm.frame_samples,
            6 * 48,
            "6 ms of stereo L16, 1152 bytes, fits 1200"
        );
        let surround = choose_format(&all, 6, 1400).unwrap();
        assert_eq!(surround.codec, AudioCodec::PCM_L16);
        assert_eq!(surround.frame_samples, 2 * 48);
        assert!(choose_format(&[AudioCodec::PCM_L24], 8, 1000).is_none());
        assert!(choose_format(&[], 2, 1200).is_none());
    }

    /// Stream descriptions round-trip through `AUDIO_CONFIG` values.
    #[test]
    fn describes_streams() {
        let format = AudioFormat {
            codec: AudioCodec::PCM_L16,
            sample_rate: SAMPLE_RATE,
            channels: 2,
            frame_samples: 240,
        };
        let config = stream_config(
            StreamId(VarInt::from_u32(16)),
            AudioDirection::Playback,
            format,
        );
        assert_eq!(config.frame_duration.into_inner(), 5000);
        assert_eq!(stream_format(&config), Some(format));
        // PCM may also run at 44.1 kHz, Opus may not.
        let cd = AudioStreamConfig {
            sample_rate: VarInt::from_u32(PCM_CD_RATE),
            ..config.clone()
        };
        let cd_format = stream_format(&cd).unwrap();
        assert_eq!(
            (cd_format.sample_rate, cd_format.frame_samples),
            (PCM_CD_RATE, 220)
        );
        let opus = AudioStreamConfig {
            codec: AudioCodec::OPUS,
            ..cd
        };
        assert_eq!(stream_format(&opus), None);
        let foreign = AudioStreamConfig {
            sample_rate: VarInt::from_u32(32_000),
            ..config
        };
        assert_eq!(stream_format(&foreign), None);
    }

    /// Resampling keeps a steady signal steady, makes as many samples as
    /// the rates ask across chunks, and interpolates between samples.
    #[test]
    fn resamples() {
        let mut resampler = Resampler::new(SAMPLE_RATE, PCM_CD_RATE, 2);
        let mut output = Vec::new();
        for _ in 0..10 {
            output.extend(resampler.process(&[0.5; 2 * 480]));
        }
        let frames = output.len() / 2;
        assert!((4405..=4410).contains(&frames), "{frames} frames");
        assert!(output.iter().all(|&sample| (sample - 0.5).abs() < 1e-6));
        let mut ramp = Resampler::new(2, 4, 1);
        assert_eq!(ramp.process(&[0.0, 1.0, 2.0]), [0.0, 0.5, 1.0, 1.5]);
    }

    /// A microphone with no sound, for the tests.
    struct SilentMicrophone;

    impl AudioInput for SilentMicrophone {
        fn channels(&self) -> u16 {
            1
        }

        fn start(&mut self, _capture: CaptureSink) -> Result<ActiveDevice, String> {
            Ok(ActiveDevice(Box::new(())))
        }

        fn description(&self) -> &str {
            "silent microphone"
        }
    }

    /// Turning the microphone on or off asks for capture channels only
    /// while it is on, and only when something changed.
    #[test]
    fn switches_the_microphone() {
        let devices = ClientAudio {
            speakers: None,
            microphone: Some(Box::new(SilentMicrophone)),
            microphone_state: Microphone::Off,
        };
        assert_eq!(devices.request().unwrap().capture_channels, VarInt::ZERO);
        let mut streams = ClientAudioStreams::new(devices);
        assert_eq!(streams.set_microphone(Microphone::Off), None);
        let on = streams.set_microphone(Microphone::On).unwrap();
        assert_eq!(on.capture_channels, VarInt::from_u32(1));
        let off = streams.set_microphone(Microphone::Off).unwrap();
        assert_eq!(off.capture_channels, VarInt::ZERO);
        let mut without = ClientAudioStreams::new(ClientAudio::default());
        assert_eq!(without.set_microphone(Microphone::On), None);
    }

    /// Loss percentages round up, and no traffic means no loss.
    #[test]
    fn measures_loss() {
        assert_eq!(loss_percent(0, 0), 0);
        assert_eq!(loss_percent(0, 500), 0);
        assert_eq!(loss_percent(1, 500), 1);
        assert_eq!(loss_percent(25, 100), 25);
        assert_eq!(loss_percent(9, 4), 100);
    }

    /// Capabilities name each codec in the directions the devices allow.
    #[test]
    fn advertises_codecs() {
        assert!(capabilities(None, None).is_empty());
        let both = capabilities(Some(2), Some(1));
        assert_eq!(both.len(), 3);
        let Capability::AudioCodec(opus) = &both[0] else {
            panic!("an audio codec");
        };
        assert_eq!(opus.codec, AudioCodec::OPUS);
        assert!(opus.directions.contains(Directions::SEND));
        assert!(opus.directions.contains(Directions::RECEIVE));
    }
}
