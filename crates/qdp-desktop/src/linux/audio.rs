//! Audio on Linux through PipeWire: the desktop's sound, captured from the
//! default output's monitor; a virtual microphone that applications record
//! the client's microphone from; and a client's speakers and microphone.

use std::path::PathBuf;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use pipewire as pw;
use pw::spa;
use qdp_runtime::{ActiveDevice, AudioInput, AudioOutput, CaptureSink, Playout, SAMPLE_RATE};
use tracing::{debug, warn};

/// The bytes of one 32-bit float sample.
const SAMPLE_BYTES: usize = 4;

/// How long to wait for a stream to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The samples per channel to play when PipeWire does not say how many it
/// wants: 10 ms.
const DEFAULT_PLAYBACK_FRAMES: usize = SAMPLE_RATE as usize / 100;

/// The name of the threads that run PipeWire audio streams.
const THREAD_NAME: &str = "qdp-pipewire-audio";

/// The SPA positions of each channel count, in the channel order of RFC 7845's
/// mapping family 1 (Section 7.3).
fn positions(channels: u16) -> Vec<u32> {
    use spa::sys::{
        SPA_AUDIO_CHANNEL_FC, SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR, SPA_AUDIO_CHANNEL_LFE,
        SPA_AUDIO_CHANNEL_MONO, SPA_AUDIO_CHANNEL_RC, SPA_AUDIO_CHANNEL_RL, SPA_AUDIO_CHANNEL_RR,
        SPA_AUDIO_CHANNEL_SL, SPA_AUDIO_CHANNEL_SR,
    };
    let (fl, fc, fr, lfe) = (
        SPA_AUDIO_CHANNEL_FL,
        SPA_AUDIO_CHANNEL_FC,
        SPA_AUDIO_CHANNEL_FR,
        SPA_AUDIO_CHANNEL_LFE,
    );
    let (sl, sr, rl, rr, rc) = (
        SPA_AUDIO_CHANNEL_SL,
        SPA_AUDIO_CHANNEL_SR,
        SPA_AUDIO_CHANNEL_RL,
        SPA_AUDIO_CHANNEL_RR,
        SPA_AUDIO_CHANNEL_RC,
    );
    match channels {
        1 => vec![SPA_AUDIO_CHANNEL_MONO],
        2 => vec![fl, fr],
        3 => vec![fl, fc, fr],
        4 => vec![fl, fr, rl, rr],
        5 => vec![fl, fc, fr, rl, rr],
        6 => vec![fl, fc, fr, rl, rr, lfe],
        7 => vec![fl, fc, fr, sl, sr, rc, lfe],
        _ => vec![fl, fc, fr, sl, sr, rl, rr, lfe],
    }
}

/// What a PipeWire audio stream is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Purpose {
    /// The host's desktop sound: the default output's monitor.
    Desktop,
    /// The client's microphone.
    Microphone,
    /// The client's speakers.
    Speakers,
    /// A source the host's applications record the client's microphone from.
    VirtualMicrophone,
}

impl Purpose {
    /// Returns the stream's properties.
    fn properties(self) -> pw::properties::PropertiesBox {
        let mut properties = pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
        };
        let entries: &[(&str, &str)] = match self {
            Self::Desktop => &[
                (*pw::keys::MEDIA_CATEGORY, "Capture"),
                (*pw::keys::MEDIA_ROLE, "Music"),
                (*pw::keys::STREAM_CAPTURE_SINK, "true"),
                (*pw::keys::NODE_NAME, "qdp-desktop-audio"),
            ],
            Self::Microphone => &[
                (*pw::keys::MEDIA_CATEGORY, "Capture"),
                (*pw::keys::MEDIA_ROLE, "Communication"),
                (*pw::keys::NODE_NAME, "qdp-microphone-capture"),
            ],
            Self::Speakers => &[
                (*pw::keys::MEDIA_CATEGORY, "Playback"),
                (*pw::keys::MEDIA_ROLE, "Music"),
                (*pw::keys::NODE_NAME, "qdp-speakers"),
            ],
            Self::VirtualMicrophone => &[
                (*pw::keys::MEDIA_CLASS, "Audio/Source"),
                (*pw::keys::NODE_NAME, "qdp-microphone"),
                (*pw::keys::NODE_DESCRIPTION, "QDP microphone"),
            ],
        };
        for (key, value) in entries {
            properties.insert(*key, *value);
        }
        properties
    }

    /// Returns the stream's direction: capture streams take data in.
    fn direction(self) -> spa::utils::Direction {
        match self {
            Self::Desktop | Self::Microphone => spa::utils::Direction::Input,
            Self::Speakers | Self::VirtualMicrophone => spa::utils::Direction::Output,
        }
    }

    /// Returns how the stream connects: the virtual microphone waits for
    /// applications to link to it, and the others link to the default
    /// device.
    fn flags(self) -> pw::stream::StreamFlags {
        let flags = pw::stream::StreamFlags::MAP_BUFFERS | pw::stream::StreamFlags::RT_PROCESS;
        match self {
            Self::VirtualMicrophone => flags,
            _ => flags | pw::stream::StreamFlags::AUTOCONNECT,
        }
    }
}

/// Where a stream's samples go or come from.
enum Endpoint {
    /// Captured samples go here.
    Capture(CaptureSink),
    /// Played samples come from here.
    Playback(Playout),
}

/// A PipeWire audio stream's thread, stopped when dropped.
struct StreamThread {
    /// Stops the thread's main loop.
    stop: Option<pw::channel::Sender<()>>,
    /// The thread.
    thread: Option<JoinHandle<()>>,
}

impl Drop for StreamThread {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts a stream for `purpose` of `channels` channels on a thread of its
/// own, through the PipeWire at socket `remote` or the session's own, and
/// waits until it is connected.
fn start(
    purpose: Purpose,
    channels: u16,
    endpoint: Endpoint,
    remote: Option<PathBuf>,
) -> Result<ActiveDevice, String> {
    let (stop, stop_receiver) = pw::channel::channel();
    let (connected, connected_receiver) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name(THREAD_NAME.to_owned())
        .spawn(move || {
            if let Err(error) = run(
                purpose,
                channels,
                endpoint,
                remote,
                stop_receiver,
                &connected,
            ) {
                let _ = connected.send(Err(error));
            }
        })
        .map_err(|error| error.to_string())?;
    let device = StreamThread {
        stop: Some(stop),
        thread: Some(thread),
    };
    connected_receiver
        .recv_timeout(CONNECT_TIMEOUT)
        .map_err(|_| "PipeWire did not connect the audio stream".to_owned())??;
    Ok(ActiveDevice(Box::new(device)))
}

/// Returns the format parameter: 32-bit float samples at [`SAMPLE_RATE`].
fn format_param(channels: u16) -> Result<Vec<u8>, String> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(SAMPLE_RATE);
    info.set_channels(u32::from(channels));
    let mut position = [0; spa::param::audio::MAX_CHANNELS];
    for (slot, channel) in position.iter_mut().zip(positions(channels)) {
        *slot = channel;
    }
    info.set_position(position);
    let object = spa::pod::Object {
        type_: spa::sys::SPA_TYPE_OBJECT_Format,
        id: spa::sys::SPA_PARAM_EnumFormat,
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|(bytes, _)| bytes.into_inner())
    .map_err(|error| format!("cannot describe the audio format: {error:?}"))
}

/// Runs a stream until `stop` receives, reporting on `connected` once it is
/// connected.
fn run(
    purpose: Purpose,
    channels: u16,
    endpoint: Endpoint,
    remote: Option<PathBuf>,
    stop: pw::channel::Receiver<()>,
    connected: &mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    pw::init();
    let main_loop = pw::main_loop::MainLoopRc::new(None).map_err(|error| error.to_string())?;
    let context =
        pw::context::ContextRc::new(&main_loop, None).map_err(|error| error.to_string())?;
    let remote = remote.map(|socket| {
        pw::properties::properties! {
            *pw::keys::REMOTE_NAME => socket.to_string_lossy().into_owned(),
        }
    });
    let core = context
        .connect_rc(remote)
        .map_err(|error| format!("cannot connect to PipeWire: {error}"))?;
    let stream = pw::stream::StreamBox::new(&core, "qdp-audio", purpose.properties())
        .map_err(|error| error.to_string())?;
    let stride = usize::from(channels) * SAMPLE_BYTES;
    let _listener = stream
        .add_local_listener_with_user_data(endpoint)
        .process(move |stream, endpoint| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let requested = usize::try_from(buffer.requested()).unwrap_or(0);
            let Some(data) = buffer.datas_mut().first_mut() else {
                return;
            };
            match endpoint {
                Endpoint::Capture(capture) => {
                    let chunk = data.chunk();
                    let (offset, size) = (chunk.offset() as usize, chunk.size() as usize);
                    let Some(bytes) = data
                        .data()
                        .and_then(|bytes| bytes.get(offset..offset + size))
                    else {
                        return;
                    };
                    let samples: Vec<f32> = bytes
                        .chunks_exact(SAMPLE_BYTES)
                        .map(|sample| {
                            f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]])
                        })
                        .collect();
                    capture.push(&samples);
                }
                Endpoint::Playback(playout) => {
                    let Some(bytes) = data.data() else {
                        return;
                    };
                    let room = bytes.len() / stride;
                    let wanted = if requested > 0 {
                        requested
                    } else {
                        DEFAULT_PLAYBACK_FRAMES
                    };
                    let frames = wanted.min(room);
                    let mut samples = vec![0.0_f32; frames * usize::from(channels)];
                    playout.pull(&mut samples);
                    for (target, sample) in bytes.chunks_exact_mut(SAMPLE_BYTES).zip(&samples) {
                        target.copy_from_slice(&sample.to_le_bytes());
                    }
                    let chunk = data.chunk_mut();
                    *chunk.offset_mut() = 0;
                    *chunk.stride_mut() = stride as i32;
                    *chunk.size_mut() = (frames * stride) as u32;
                }
            }
        })
        .register()
        .map_err(|error| error.to_string())?;

    let format = format_param(channels)?;
    let pod = spa::pod::Pod::from_bytes(&format)
        .ok_or_else(|| "cannot describe the audio format".to_owned())?;
    stream
        .connect(purpose.direction(), None, purpose.flags(), &mut [pod])
        .map_err(|error| format!("cannot connect the audio stream: {error}"))?;
    let _ = connected.send(Ok(()));
    debug!(?purpose, channels, "audio stream connected");

    let quit_loop = main_loop.clone();
    let _stop = stop.attach(main_loop.loop_(), move |()| quit_loop.quit());
    main_loop.run();
    Ok(())
}

/// Captures audio through PipeWire: the desktop's sound or a microphone.
#[derive(Debug)]
pub struct PipeWireInput {
    /// What the stream captures.
    purpose: Purpose,
    /// The channels it captures.
    channels: u16,
    /// Its name, for logs.
    description: &'static str,
    /// The socket of the PipeWire it captures from, or `None` for the
    /// session's own.
    remote: Option<PathBuf>,
}

impl PipeWireInput {
    /// Returns an input capturing the desktop's sound in stereo: what plays
    /// on the default output.
    pub fn desktop() -> Self {
        Self {
            purpose: Purpose::Desktop,
            channels: 2,
            description: "PipeWire desktop audio",
            remote: None,
        }
    }

    /// Returns an input capturing the default microphone in mono.
    pub fn microphone() -> Self {
        Self {
            purpose: Purpose::Microphone,
            channels: 1,
            description: "PipeWire microphone",
            remote: None,
        }
    }

    /// Returns the input capturing through the PipeWire at `socket`, such
    /// as another user's session's, rather than this session's.
    pub fn through(self, socket: PathBuf) -> Self {
        Self {
            remote: Some(socket),
            ..self
        }
    }
}

impl AudioInput for PipeWireInput {
    fn channels(&self) -> u16 {
        self.channels
    }

    fn start(&mut self, capture: CaptureSink) -> Result<ActiveDevice, String> {
        start(
            self.purpose,
            self.channels,
            Endpoint::Capture(capture),
            self.remote.clone(),
        )
    }

    fn description(&self) -> &str {
        self.description
    }
}

/// Plays audio through PipeWire: on the default output, or as a virtual
/// microphone.
#[derive(Debug)]
pub struct PipeWireOutput {
    /// What the stream plays to.
    purpose: Purpose,
    /// The most channels it plays.
    channels: u16,
    /// Its name, for logs.
    description: &'static str,
    /// The socket of the PipeWire it plays through, or `None` for the
    /// session's own.
    remote: Option<PathBuf>,
}

impl PipeWireOutput {
    /// Returns an output playing on the default output device, in up to 8
    /// channels.
    pub fn speakers() -> Self {
        Self {
            purpose: Purpose::Speakers,
            channels: 2,
            description: "PipeWire speakers",
            remote: None,
        }
    }

    /// Returns a mono source named "QDP microphone" that the host's
    /// applications record from.
    pub fn virtual_microphone() -> Self {
        Self {
            purpose: Purpose::VirtualMicrophone,
            channels: 1,
            description: "PipeWire virtual microphone",
            remote: None,
        }
    }

    /// Returns the output playing through the PipeWire at `socket`, such
    /// as another user's session's, rather than this session's.
    pub fn through(self, socket: PathBuf) -> Self {
        Self {
            remote: Some(socket),
            ..self
        }
    }
}

impl AudioOutput for PipeWireOutput {
    fn channels(&self) -> u16 {
        self.channels
    }

    fn start(&mut self, channels: u16, playout: Playout) -> Result<ActiveDevice, String> {
        if channels > self.channels {
            warn!(channels, "more audio channels than the device plays");
        }
        start(
            self.purpose,
            channels,
            Endpoint::Playback(playout),
            self.remote.clone(),
        )
    }

    fn description(&self) -> &str {
        self.description
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every channel count has one position per channel, mono and stereo as
    /// RFC 7845 orders them.
    #[test]
    fn positions_channels() {
        for channels in 1..=8 {
            assert_eq!(positions(channels).len(), usize::from(channels));
        }
        assert_eq!(positions(1), [spa::sys::SPA_AUDIO_CHANNEL_MONO]);
        assert_eq!(
            positions(2),
            [
                spa::sys::SPA_AUDIO_CHANNEL_FL,
                spa::sys::SPA_AUDIO_CHANNEL_FR
            ]
        );
        assert_eq!(positions(6)[5], spa::sys::SPA_AUDIO_CHANNEL_LFE);
    }

    /// The format parameter serializes.
    #[test]
    fn describes_the_format() {
        let bytes = format_param(2).unwrap();
        assert!(spa::pod::Pod::from_bytes(&bytes).is_some());
    }
}
