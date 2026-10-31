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
use tracing::{debug, warn};

use crate::RuntimeError;
use crate::audio_receiver::AudioReceiver;

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
/// percent: the client has no loss reports for its own stream.
const CAPTURE_EXPECTED_LOSS: u8 = 10;

/// The first Stream ID of audio streams; video uses the lower ones.
const FIRST_AUDIO_STREAM_ID: u64 = 16;

/// The names of the audio threads, as debuggers and profilers show them.
const SENDER_THREAD: &str = "qdp-audio-send";

/// The name of the audio playback thread.
const PLAYER_THREAD: &str = "qdp-audio-play";

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

/// Where a playback device takes its samples from.
#[derive(Debug, Clone)]
pub struct Playout(Arc<Mutex<AudioReceiver>>);

impl Playout {
    /// Fills `output` with the next interleaved samples at [`SAMPLE_RATE`],
    /// or silence while the stream's buffer fills.
    pub fn pull(&self, output: &mut [f32]) {
        match self.0.lock() {
            Ok(mut receiver) => receiver.pull(output),
            Err(_) => output.fill(0.0),
        }
    }
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
                        let mut stream = SendLoop {
                            stream_id,
                            format,
                            limit,
                            connection,
                            clock,
                            active,
                            input_channels: usize::from(input.channels()),
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
            match captured.recv_timeout(CAPTURE_WAIT) {
                Ok(chunk) => {
                    let channels = usize::from(self.format.channels);
                    self.pending
                        .extend(remix(&chunk, self.input_channels, channels));
                    while self.pending.len() >= self.format.frame_len() {
                        let frame: Vec<f32> =
                            self.pending.drain(..self.format.frame_len()).collect();
                        // The frame's first sample was captured before the
                        // frame and everything still waiting.
                        let queued = self.pending.len() / channels + self.format.frame_samples;
                        let age = Duration::from_secs_f64(queued as f64 / f64::from(SAMPLE_RATE));
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

/// A stream the endpoint plays: its jitter buffer, and a thread that keeps
/// the output device playing from it.
pub(crate) struct AudioPlayer {
    /// The stream.
    stream_id: StreamId,
    /// The jitter buffer the device pulls from.
    receiver: Arc<Mutex<AudioReceiver>>,
    /// Ends the thread; dropping it does too.
    stop: Option<mpsc::Sender<()>>,
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
        let playout = Playout(Arc::clone(&receiver));
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name(PLAYER_THREAD.to_owned())
            .spawn(move || {
                match output.start(format.channels, playout) {
                    Ok(device) => {
                        let _ = stopped.recv();
                        drop(device);
                    }
                    Err(error) => warn!(
                        error,
                        device = output.description(),
                        "audio playback failed"
                    ),
                }
                output
            })
            .map_err(|error| RuntimeError::Audio(error.to_string()))?;
        Ok(Self {
            stream_id,
            receiver,
            stop: Some(stop),
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
        self.stop = None;
        self.thread.take()?.join().ok()
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
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

/// Returns the format of a stream described in `AUDIO_CONFIG`, if this
/// endpoint can handle it.
fn stream_format(config: &AudioStreamConfig) -> Option<AudioFormat> {
    let sample_rate = u32::try_from(config.sample_rate.into_inner()).ok()?;
    let channels = u16::try_from(config.channel_count.into_inner()).ok()?;
    let duration = u64::from(config.frame_duration.into_inner().min(u64::from(u32::MAX)) as u32);
    let frame_samples = usize::try_from(duration * u64::from(sample_rate) / 1_000_000).ok()?;
    (sample_rate == SAMPLE_RATE && (1..=PCM_MAX_CHANNELS).contains(&channels) && frame_samples > 0)
        .then_some(AudioFormat {
            codec: config.codec,
            sample_rate,
            channels,
            frame_samples,
        })
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
    /// The client's microphone being played.
    capture: Option<AudioPlayer>,
    /// The Config ID of the last `AUDIO_CONFIG`.
    config_id: u64,
    /// The Stream ID of the last stream defined.
    last_stream_id: u64,
    /// Whether the session is active, so that packets may be sent.
    active: SessionActive,
}

impl HostAudioStreams {
    /// Returns the streams of a host with `devices`, none defined yet.
    pub(crate) fn new(devices: HostAudio) -> Self {
        Self {
            devices,
            playback: None,
            capture: None,
            config_id: 0,
            last_stream_id: FIRST_AUDIO_STREAM_ID - 1,
            active: SessionActive::default(),
        }
    }

    /// Lets the streams send packets: the session is active. Streams
    /// configured before `SESSION_START` capture, but send nothing until
    /// then (Section 4.2).
    pub(crate) fn activate(&self) {
        self.active.activate();
    }

    /// Returns the next Stream ID: any change of parameters needs a new one
    /// (Section 7.3).
    fn next_stream_id(&mut self) -> StreamId {
        self.last_stream_id += 1;
        StreamId(VarInt::new(self.last_stream_id).unwrap_or(VarInt::MAX))
    }

    /// Answers an `AUDIO_REQUEST`: stops the current streams, defines new ones
    /// the client asked for and both endpoints can carry, starts them, and
    /// returns the `AUDIO_CONFIG` that describes them.
    pub(crate) fn configure(
        &mut self,
        request: &AudioRequest,
        negotiated: &Negotiated,
        connection: &quinn::Connection,
        clock: SessionClock,
    ) -> Result<AudioConfig, RuntimeError> {
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
                    let stream_id = self.next_stream_id();
                    let kind = (AudioContent::General, PacketLimit::Path);
                    let sender = AudioSender::spawn(
                        desktop,
                        stream_id,
                        format,
                        kind,
                        connection.clone(),
                        clock,
                        self.active.clone(),
                    )?;
                    self.playback = Some(sender);
                    streams.push(stream_config(stream_id, AudioDirection::Playback, format));
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
                    let stream_id = self.next_stream_id();
                    self.capture = Some(AudioPlayer::spawn(microphone, stream_id, format)?);
                    streams.push(stream_config(stream_id, AudioDirection::Capture, format));
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
    /// The microphone the host receives, present only when the user turned it
    /// on for the session (Section 7.7).
    pub microphone: Option<Box<dyn AudioInput>>,
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

    /// Returns the client's `AUDIO_REQUEST`, or `None` if it has no audio
    /// devices.
    pub(crate) fn request(&self) -> Option<AudioRequest> {
        let playback = self.speakers.as_ref().map_or(0, |output| output.channels());
        let capture = self.microphone.as_ref().map_or(0, |input| input.channels());
        (playback > 0 || capture > 0).then(|| AudioRequest {
            playback_channels: VarInt::from(u32::from(playback)),
            capture_channels: VarInt::from(u32::from(capture)),
        })
    }
}

/// The client's audio streams.
pub(crate) struct ClientAudioStreams {
    /// The devices not in use by a stream.
    devices: ClientAudio,
    /// The host's audio being played.
    playback: Option<AudioPlayer>,
    /// The microphone being sent.
    capture: Option<AudioSender>,
}

impl ClientAudioStreams {
    /// Returns the streams of a client with `devices`, none defined yet.
    pub(crate) fn new(devices: ClientAudio) -> Self {
        Self {
            devices,
            playback: None,
            capture: None,
        }
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
                        self.playback =
                            Some(AudioPlayer::spawn(speakers, stream.stream_id, format)?);
                    }
                }
                AudioDirection::Capture if self.capture.is_none() => {
                    // A capture stream the client did not ask for is ignored,
                    // since only an enabled microphone is among the devices.
                    if let Some(microphone) = self.devices.microphone.take() {
                        let kind = (AudioContent::Speech, PacketLimit::Smallest);
                        self.capture = Some(AudioSender::spawn(
                            microphone,
                            stream.stream_id,
                            format,
                            kind,
                            connection.clone(),
                            clock,
                            SessionActive::active(),
                        )?);
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
        let foreign = AudioStreamConfig {
            sample_rate: VarInt::from_u32(44_100),
            ..config
        };
        assert_eq!(stream_format(&foreign), None);
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
