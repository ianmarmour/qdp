//! End-to-end tests that stream AV1 video from a host to a client over a real
//! QUIC connection on the loopback interface.

use std::f32::consts::TAU;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use qdp::auth::scram::ScramCredentials;
use qdp::message::{CursorImage, KeyboardInfo, LayoutPlatform, PointerKind, PointerMode};
use qdp_codec::{CodecPreference, Yuv420Frame};
use qdp_quic::{ClientEndpoint, HostEndpoint, HostIdentity, HostVerification, KeyPin};
use qdp_runtime::{
    ActiveDevice, AudioInput, AudioOutput, CaptureSink, ClientAudio, ClientCredentials,
    ClientOptions, Clipboard, ClipboardDirections, CursorShape, CursorState, CursorWarp,
    DisplaySize, EncoderTuning, FrameInfo, FrameSink, FrameSource, HostAudio, HostAuthenticator,
    HostOptions, InputAction, InputHandle, InputInjector, InputQueue, NoInput, Playout,
    PointerButton, RuntimeError, SAMPLE_RATE, SharedClipboard, SinkControl, TestPattern, UserInput,
    WarpKind, input_channel, run, serve,
};

/// The width of the streamed frames.
const WIDTH: usize = 256;

/// The height of the streamed frames.
const HEIGHT: usize = 160;

/// The number of frames the client waits for before it disconnects.
const FRAMES: usize = 30;

/// The lowest acceptable luma PSNR of a decoded frame, in decibels.
const MIN_PSNR: f64 = 30.0;

/// The longest acceptable time from capture on the host to decoding on the
/// client over loopback, generous enough for slow test machines.
const MAX_LATENCY: Duration = Duration::from_millis(250);

/// The longest a test may run before it is considered hung.
const TEST_TIMEOUT: Duration = Duration::from_secs(60);

/// The token the host accepts.
const TOKEN: &[u8] = b"loopback-token";

/// The server name the client sends.
const SERVER_NAME: &str = "qdp-host";

/// Returns a loopback address with an ephemeral port.
fn loopback() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

/// The frame after which the collector sends the test input, once the devices
/// are accepted.
const INPUT_FRAME: usize = 5;

/// Collects what the client presents and stops after [`FRAMES`] frames,
/// sending the test input after [`INPUT_FRAME`] frames if it has a handle.
#[derive(Default)]
struct Collector {
    /// The frame number, luma PSNR against the pattern, and frame information
    /// of each presented frame.
    frames: Vec<(u64, f64, FrameInfo)>,
    /// Where to send the test input.
    input: Option<InputHandle>,
    /// The cursor images the client was told to draw, `None` for hidden.
    cursors: Vec<Option<CursorImage>>,
    /// The warps the client received.
    warps: Vec<CursorWarp>,
}

/// Where the cursor source's pointer is, in pixels.
const CURSOR_POSITION: (i32, i32) = (40, 30);

/// The capture after which the cursor source's pointer changes shape.
const SHAPE_CHANGE: u64 = 10;

/// Returns a solid square pointer image.
fn pointer_shape(size: u32, colour: u8) -> Arc<CursorShape> {
    Arc::new(CursorShape {
        width: size,
        height: size,
        hotspot: (0, 0),
        pixels: vec![colour; (size * size * 4) as usize],
    })
}

/// The test pattern, with a pointer reported apart from the frames that
/// changes shape after [`SHAPE_CHANGE`] captures.
struct CursorPattern {
    /// The frames.
    pattern: TestPattern,
    /// The captures so far.
    captures: u64,
    /// The pointer's shapes, before and after the change.
    shapes: [Arc<CursorShape>; 2],
}

impl FrameSource for CursorPattern {
    fn size(&self) -> (usize, usize) {
        self.pattern.size()
    }

    fn capture(&mut self) -> Result<Yuv420Frame, RuntimeError> {
        self.captures += 1;
        self.pattern.capture()
    }

    fn cursor(&mut self) -> Option<CursorState> {
        let shape = &self.shapes[usize::from(self.captures > SHAPE_CHANGE)];
        Some(CursorState {
            position: Some(CURSOR_POSITION),
            shape: Some(Arc::clone(shape)),
        })
    }
}

/// The input the input test sends, as the user would produce it.
fn test_input() -> Vec<UserInput> {
    vec![
        UserInput::Key {
            usage: 0x04,
            pressed: true,
        },
        UserInput::Key {
            usage: 0x04,
            pressed: false,
        },
        UserInput::PointerMoved { x: 10.0, y: 20.0 },
        UserInput::Button {
            button: PointerButton::Left,
            pressed: true,
        },
        UserInput::Button {
            button: PointerButton::Left,
            pressed: false,
        },
        UserInput::Scroll {
            vertical: 120,
            horizontal: 0,
        },
        UserInput::Text("hé".to_owned()),
    ]
}

/// The actions the host should perform for [`test_input`].
fn expected_actions() -> Vec<InputAction> {
    vec![
        InputAction::KeyDown(0x04),
        InputAction::KeyUp(0x04),
        InputAction::PointerMoveTo {
            x: 10 * 256,
            y: 20 * 256,
        },
        InputAction::ButtonDown(PointerButton::Left),
        InputAction::ButtonUp(PointerButton::Left),
        InputAction::Scroll {
            vertical: 120,
            horizontal: 0,
        },
        InputAction::Text("hé".to_owned()),
    ]
}

/// Records the actions the host performs, and the keyboard layout it is asked
/// to select.
#[derive(Debug, Clone, Default)]
struct Recorder(
    Arc<Mutex<Vec<InputAction>>>,
    Arc<Mutex<Option<KeyboardInfo>>>,
);

impl InputInjector for Recorder {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        self.0.lock().unwrap().push(action.clone());
        Ok(())
    }

    fn description(&self) -> &str {
        "recorder"
    }

    fn supports_text(&self) -> bool {
        true
    }

    fn set_keyboard_layout(&mut self, info: &KeyboardInfo) {
        *self.1.lock().unwrap() = Some(info.clone());
    }
}

impl FrameSink for Collector {
    fn present(&mut self, frame: &Yuv420Frame, info: FrameInfo) -> SinkControl {
        // The host sends every frame it captures, so frame number n shows pattern
        // frame n - 1.
        let number = info.frame_number.0.into_inner();
        let expected = TestPattern::frame(WIDTH, HEIGHT, number - 1);
        let psnr = frame.luma_psnr(&expected).unwrap_or(0.0);
        self.frames.push((number, psnr, info));
        if self.frames.len() == INPUT_FRAME {
            if let Some(input) = &self.input {
                for action in test_input() {
                    assert!(input.send(action));
                }
            }
        }
        if self.frames.len() >= FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }

    fn set_cursor(&mut self, cursor: Option<&CursorImage>) {
        self.cursors.push(cursor.cloned());
    }

    fn warp_cursor(&mut self, warp: CursorWarp) {
        self.warps.push(warp);
    }
}

/// Returns the test pattern source.
fn test_pattern() -> Box<dyn FrameSource> {
    Box::new(TestPattern::new(WIDTH, HEIGHT))
}

/// Starts a host serving one connection, and returns its address, its key pin
/// and the task that returns the host's summary.
fn start_host(
    authenticator: HostAuthenticator,
    source: Box<dyn FrameSource>,
    injector: Box<dyn InputInjector>,
    clipboard: Option<SharedClipboard>,
    audio: HostAudio,
) -> (
    SocketAddr,
    KeyPin,
    tokio::task::JoinHandle<Result<qdp_runtime::HostSummary, RuntimeError>>,
) {
    let identity = HostIdentity::generate_self_signed(vec![SERVER_NAME.to_owned()]).unwrap();
    let pin = identity.key_pin().unwrap();
    let endpoint = HostEndpoint::bind(loopback(), identity).unwrap();
    let address = endpoint.local_address().unwrap();
    let options = HostOptions {
        host_name: "loopback".to_owned(),
        authenticator,
        frame_rate: 60,
        encoder: EncoderTuning {
            preference: CodecPreference::Software,
            speed: 10,
            quantizer: 60,
            threads: 2,
        },
    };
    let task = tokio::spawn(async move {
        let connection = endpoint.accept().await.expect("a client")?;
        serve(connection, &options, source, injector, clipboard, audio).await
    });
    (address, pin, task)
}

/// Returns the options of the test client.
fn client_options(credentials: ClientCredentials) -> ClientOptions {
    ClientOptions {
        client_name: "qdp-streaming-test/0.1.0".to_owned(),
        credentials,
        display: DisplaySize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
            refresh_millihertz: 60_000,
        },
        decoder_threads: 2,
        decoder: CodecPreference::Software,
        keyboard_info: Some(KeyboardInfo {
            language: "en-GB".to_owned(),
            layout_platform: LayoutPlatform::XKB,
            layout: "gb".to_owned(),
        }),
    }
}

/// Connects a client with `credentials` and streams until the collector stops,
/// sending the test input if `input` is given.
async fn stream(
    address: SocketAddr,
    pin: KeyPin,
    credentials: ClientCredentials,
    input: Option<(InputHandle, InputQueue)>,
) -> Result<Collector, RuntimeError> {
    let endpoint = ClientEndpoint::bind(loopback()).unwrap();
    let connection = endpoint
        .connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![pin]),
        )
        .await?;
    let options = client_options(credentials);
    let (handle, queue) = input.unzip();
    let mut collector = Collector {
        input: handle,
        ..Collector::default()
    };
    let summary = run(
        connection,
        &options,
        &mut collector,
        queue,
        None,
        ClientAudio::default(),
    )
    .await?;
    assert_eq!(summary.frames_presented, FRAMES as u64);
    assert_eq!(summary.decode_failures, 0);
    endpoint.wait_idle().await;
    Ok(collector)
}

/// Checks the collected frames: consecutive, faithful, and timed once the clocks
/// are synchronized.
fn check_frames(collector: &Collector) {
    let numbers: Vec<u64> = collector
        .frames
        .iter()
        .map(|(number, ..)| *number)
        .collect();
    assert_eq!(numbers, (1..=FRAMES as u64).collect::<Vec<_>>());
    for (number, psnr, _) in &collector.frames {
        assert!(*psnr > MIN_PSNR, "frame {number}: {psnr:.1} dB");
    }
    let (_, _, last) = collector.frames.last().unwrap();
    let latency = last
        .latency
        .expect("the clocks are synchronized by the end");
    assert!(latency < MAX_LATENCY, "capture to decode took {latency:?}");
}

/// A client with a token streams video from a host.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_with_token() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let (address, pin, host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(NoInput),
        None,
        HostAudio::default(),
    );
    let collector = tokio::time::timeout(
        TEST_TIMEOUT,
        stream(address, pin, ClientCredentials::Token(TOKEN.to_vec()), None),
    )
    .await
    .expect("streaming finishes in time")
    .unwrap();
    check_frames(&collector);

    let summary = tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    assert!(summary.frames_sent >= FRAMES as u64);
    assert!(summary.key_frames_sent >= 1);
}

/// A client with a password streams video from a host over SCRAM-SHA-256-PLUS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_with_password() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_user(
        "ian".to_owned(),
        ScramCredentials::derive("pencil", b"streaming salt".to_vec(), 4096).unwrap(),
    );
    let (address, pin, _host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(NoInput),
        None,
        HostAudio::default(),
    );
    let credentials = ClientCredentials::Password {
        username: "ian".to_owned(),
        password: "pencil".to_owned(),
    };
    let collector = tokio::time::timeout(TEST_TIMEOUT, stream(address, pin, credentials, None))
        .await
        .expect("streaming finishes in time")
        .unwrap();
    check_frames(&collector);
}

/// A client with the wrong password is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_password_is_refused() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_user(
        "ian".to_owned(),
        ScramCredentials::derive("pencil", b"streaming salt".to_vec(), 4096).unwrap(),
    );
    let (address, pin, _host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(NoInput),
        None,
        HostAudio::default(),
    );
    let credentials = ClientCredentials::Password {
        username: "ian".to_owned(),
        password: "wrong".to_owned(),
    };
    let result = tokio::time::timeout(TEST_TIMEOUT, stream(address, pin, credentials, None))
        .await
        .expect("the attempt finishes in time");
    assert!(matches!(result, Err(RuntimeError::AuthenticationFailed(_))));
}

/// Keys, pointer, buttons, scrolling and text sent by the client are
/// performed on the host, in order.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn input_reaches_the_host() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let recorder = Recorder::default();
    let (address, pin, host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(recorder.clone()),
        None,
        HostAudio::default(),
    );
    let credentials = ClientCredentials::Token(TOKEN.to_vec());
    tokio::time::timeout(
        TEST_TIMEOUT,
        stream(address, pin, credentials, Some(input_channel())),
    )
    .await
    .expect("streaming finishes in time")
    .unwrap();
    let summary = tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    assert!(summary.input_reports >= 6);
    assert_eq!(*recorder.0.lock().unwrap(), expected_actions());
    let keyboard = recorder
        .1
        .lock()
        .unwrap()
        .clone()
        .expect("the client's keyboard");
    assert_eq!(
        (keyboard.language.as_str(), keyboard.layout.as_str()),
        ("en-GB", "gb")
    );
}

/// A pointer the host captures apart from its frames reaches the client as
/// cursor images, `CURSOR_SET`s and a warp to where the pointer is.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cursor_reaches_the_client() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let source = CursorPattern {
        pattern: TestPattern::new(WIDTH, HEIGHT),
        captures: 0,
        shapes: [pointer_shape(4, 0x11), pointer_shape(6, 0x22)],
    };
    let (address, pin, _host) = start_host(
        authenticator,
        Box::new(source),
        Box::new(NoInput),
        None,
        HostAudio::default(),
    );
    let credentials = ClientCredentials::Token(TOKEN.to_vec());
    let collector = tokio::time::timeout(TEST_TIMEOUT, stream(address, pin, credentials, None))
        .await
        .expect("streaming finishes in time")
        .unwrap();

    // The host hides the cursor until the first capture, then shows each
    // shape in turn.
    let shown: Vec<Option<(u32, u8)>> = collector
        .cursors
        .iter()
        .map(|cursor| cursor.as_ref().map(|image| (image.width, image.pixels[0])))
        .collect();
    assert_eq!(shown, [None, Some((4, 0x11)), Some((6, 0x22))]);
    assert_eq!(collector.warps.len(), 1, "{:?}", collector.warps);
    let warp = collector.warps[0];
    assert_eq!(warp.position, (40.0, 30.0));
    assert_eq!(warp.kind, WarpKind::Move);
}

/// A clipboard in memory, shared with the test.
#[derive(Debug, Clone, Default)]
struct MemoryClipboard(Arc<Mutex<Option<String>>>);

impl MemoryClipboard {
    /// Returns a clipboard holding `text`.
    fn holding(text: &str) -> Self {
        Self(Arc::new(Mutex::new(Some(text.to_owned()))))
    }

    /// Returns the clipboard's text.
    fn get(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }

    /// Replaces the clipboard's text, as a user copying would.
    fn put(&self, text: &str) {
        *self.0.lock().unwrap() = Some(text.to_owned());
    }
}

impl Clipboard for MemoryClipboard {
    fn text(&mut self) -> Result<Option<String>, String> {
        Ok(self.get())
    }

    fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.put(text);
        Ok(())
    }

    fn description(&self) -> &str {
        "memory"
    }
}

/// The text the host copies once the client's clipboard reached it.
const HOST_COPY: &str = "copied on the host";

/// The most frames the clipboard test streams before giving up.
const CLIPBOARD_FRAMES: usize = 1200;

/// Streams until the host's copy reaches the client's clipboard, copying it
/// on the host once the client's clipboard has reached the host.
struct ClipboardWatcher {
    /// The host's clipboard.
    host: MemoryClipboard,
    /// The client's clipboard.
    client: MemoryClipboard,
    /// Whether the client's text reached the host.
    reached_host: bool,
    /// The frames presented.
    frames: usize,
}

impl FrameSink for ClipboardWatcher {
    fn present(&mut self, _frame: &Yuv420Frame, _info: FrameInfo) -> SinkControl {
        self.frames += 1;
        if !self.reached_host && self.host.get().as_deref() == Some("copied on the client") {
            self.reached_host = true;
            self.host.put(HOST_COPY);
        }
        let done = self.client.get().as_deref() == Some(HOST_COPY);
        if done || self.frames >= CLIPBOARD_FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }
}

/// Text copied on the client reaches the host's clipboard at the start of the
/// session, and text copied on the host later reaches the client's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clipboard_is_shared() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let host_clipboard = MemoryClipboard::holding("the host's old text");
    let client_clipboard = MemoryClipboard::holding("copied on the client");
    let (address, pin, _host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(NoInput),
        Some(SharedClipboard {
            clipboard: Box::new(host_clipboard.clone()),
            directions: ClipboardDirections::Both,
        }),
        HostAudio::default(),
    );
    let endpoint = ClientEndpoint::bind(loopback()).unwrap();
    let connection = endpoint
        .connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![pin]),
        )
        .await
        .unwrap();
    let mut watcher = ClipboardWatcher {
        host: host_clipboard.clone(),
        client: client_clipboard.clone(),
        reached_host: false,
        frames: 0,
    };
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    let clipboard = SharedClipboard {
        clipboard: Box::new(client_clipboard.clone()),
        directions: ClipboardDirections::Both,
    };
    tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut watcher,
            None,
            Some(clipboard),
            ClientAudio::default(),
        ),
    )
    .await
    .expect("the clipboard is shared in time")
    .unwrap();
    assert!(watcher.reached_host, "the client's text reached the host");
    assert_eq!(client_clipboard.get().as_deref(), Some(HOST_COPY));
}

/// The duration of each chunk the test audio devices move.
const CHUNK: Duration = Duration::from_millis(10);

/// The samples per channel in one chunk.
const CHUNK_SAMPLES: usize = SAMPLE_RATE as usize / 100;

/// A thread a test audio device runs, stopped when the device is dropped.
struct DeviceThread {
    /// Tells the thread to stop.
    stop: Arc<AtomicBool>,
    /// The thread.
    thread: Option<JoinHandle<()>>,
}

impl DeviceThread {
    /// Runs `step` once per chunk, in real time, until stopped.
    fn start(mut step: impl FnMut() + Send + 'static) -> ActiveDevice {
        let stop = Arc::new(AtomicBool::new(false));
        let running = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let start = std::time::Instant::now();
            let mut chunks = 0;
            while !running.load(Ordering::Relaxed) {
                step();
                chunks += 1;
                if let Some(wait) =
                    (start + CHUNK * chunks).checked_duration_since(std::time::Instant::now())
                {
                    std::thread::sleep(wait);
                }
            }
        });
        ActiveDevice(Box::new(Self {
            stop,
            thread: Some(thread),
        }))
    }
}

impl Drop for DeviceThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// An input capturing a sine tone of the same frequency on every channel.
struct ToneInput {
    /// The channels.
    channels: u16,
    /// The tone's frequency in hertz.
    frequency: f32,
}

impl AudioInput for ToneInput {
    fn channels(&self) -> u16 {
        self.channels
    }

    fn start(&mut self, capture: CaptureSink) -> Result<ActiveDevice, String> {
        let (channels, frequency) = (usize::from(self.channels), self.frequency);
        let mut sample = 0_usize;
        Ok(DeviceThread::start(move || {
            let chunk: Vec<f32> = (0..CHUNK_SAMPLES)
                .flat_map(|offset| {
                    let time = (sample + offset) as f32 / SAMPLE_RATE as f32;
                    std::iter::repeat_n(0.5 * (TAU * frequency * time).sin(), channels)
                })
                .collect();
            sample += CHUNK_SAMPLES;
            capture.push(&chunk);
        }))
    }

    fn description(&self) -> &str {
        "tone"
    }
}

/// An output recording what it plays.
#[derive(Clone)]
struct RecordingOutput {
    /// The channels.
    channels: u16,
    /// The first channel of everything played.
    recorded: Arc<Mutex<Vec<f32>>>,
}

impl RecordingOutput {
    /// Returns an output of `channels` channels that has recorded nothing.
    fn new(channels: u16) -> Self {
        Self {
            channels,
            recorded: Arc::default(),
        }
    }

    /// Returns the tone's frequency in the last half second recorded, from
    /// its zero crossings, or `None` if that was silent.
    fn frequency(&self) -> Option<f32> {
        let recorded = self.recorded.lock().unwrap();
        let half_second = SAMPLE_RATE as usize / 2;
        let tail = &recorded[recorded.len().checked_sub(half_second)?..];
        if tail.iter().all(|sample| sample.abs() < 0.01) {
            return None;
        }
        let crossings = tail
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        Some(crossings as f32)
    }
}

impl AudioOutput for RecordingOutput {
    fn channels(&self) -> u16 {
        self.channels
    }

    fn start(&mut self, channels: u16, playout: Playout) -> Result<ActiveDevice, String> {
        let recorded = Arc::clone(&self.recorded);
        let channels = usize::from(channels);
        Ok(DeviceThread::start(move || {
            let mut chunk = vec![0.0; CHUNK_SAMPLES * channels];
            playout.pull(&mut chunk);
            recorded
                .lock()
                .unwrap()
                .extend(chunk.chunks_exact(channels).map(|frame| frame[0]));
        }))
    }

    fn description(&self) -> &str {
        "recorder"
    }
}

/// Streams video until the host's audio has played for a while.
struct AudioWatcher {
    /// The client's speakers.
    speakers: RecordingOutput,
    /// The frames presented.
    frames: usize,
}

impl FrameSink for AudioWatcher {
    fn present(&mut self, _frame: &Yuv420Frame, _info: FrameInfo) -> SinkControl {
        self.frames += 1;
        let played = self.speakers.recorded.lock().unwrap().len();
        if played >= SAMPLE_RATE as usize * 3 / 2 || self.frames >= CLIPBOARD_FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }
}

/// The host's desktop audio plays on the client's speakers, and the
/// client's microphone plays on the host's virtual microphone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn audio_flows_both_ways() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let host_microphone = RecordingOutput::new(1);
    let host_audio = HostAudio {
        desktop: Some(Box::new(ToneInput {
            channels: 2,
            frequency: 440.0,
        })),
        microphone: Some(Box::new(host_microphone.clone())),
    };
    let (address, pin, _host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(NoInput),
        None,
        host_audio,
    );
    let speakers = RecordingOutput::new(2);
    let client_audio = ClientAudio {
        speakers: Some(Box::new(speakers.clone())),
        microphone: Some(Box::new(ToneInput {
            channels: 1,
            frequency: 220.0,
        })),
    };
    let endpoint = ClientEndpoint::bind(loopback()).unwrap();
    let connection = endpoint
        .connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![pin]),
        )
        .await
        .unwrap();
    let mut watcher = AudioWatcher {
        speakers: speakers.clone(),
        frames: 0,
    };
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    tokio::time::timeout(
        TEST_TIMEOUT,
        run(connection, &options, &mut watcher, None, None, client_audio),
    )
    .await
    .expect("audio plays in time")
    .unwrap();

    // Half a second of a tone of f hertz crosses zero f times.
    let played = speakers.frequency().expect("the host's audio played");
    assert!((420.0..460.0).contains(&played), "played {played} Hz");
    let heard = host_microphone
        .frequency()
        .expect("the client's microphone played");
    assert!((200.0..240.0).contains(&heard), "heard {heard} Hz");
}

/// An injector that records actions and reports a pointer captured by an
/// application, as a game would.
#[derive(Debug, Clone, Default)]
struct CapturedPointer(Recorder);

impl InputInjector for CapturedPointer {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        self.0.inject(action)
    }

    fn description(&self) -> &str {
        "captured pointer"
    }

    fn pointer_mode(&mut self) -> PointerKind {
        PointerKind::Relative
    }
}

/// Moves the mouse once the host switches to relative mode, then streams a
/// little longer.
struct ModeWatcher {
    /// Where the input goes.
    input: InputHandle,
    /// The pointer modes the host announced.
    modes: Vec<PointerKind>,
    /// The frames presented since the switch.
    frames_since: Option<usize>,
    /// The frames presented.
    frames: usize,
}

impl FrameSink for ModeWatcher {
    fn present(&mut self, _frame: &Yuv420Frame, _info: FrameInfo) -> SinkControl {
        self.frames += 1;
        if let Some(frames) = &mut self.frames_since {
            *frames += 1;
        }
        if self.frames_since.is_some_and(|frames| frames > 30) || self.frames >= CLIPBOARD_FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }

    fn set_pointer_mode(&mut self, mode: &PointerMode) {
        self.modes.push(mode.mode);
        if mode.mode == PointerKind::Relative {
            assert!(self.input.send(UserInput::MouseMoved { dx: 5.0, dy: -3.0 }));
            self.frames_since = Some(0);
        }
    }
}

/// When an application captures the host's pointer, the client switches to
/// relative mode and its motion arrives as relative moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relative_mode_moves_the_mouse() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let injector = CapturedPointer::default();
    let (address, pin, _host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(injector.clone()),
        None,
        HostAudio::default(),
    );
    let endpoint = ClientEndpoint::bind(loopback()).unwrap();
    let connection = endpoint
        .connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![pin]),
        )
        .await
        .unwrap();
    let (handle, queue) = input_channel();
    let mut watcher = ModeWatcher {
        input: handle,
        modes: Vec::new(),
        frames_since: None,
        frames: 0,
    };
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut watcher,
            Some(queue),
            None,
            ClientAudio::default(),
        ),
    )
    .await
    .expect("the session finishes in time")
    .unwrap();
    assert_eq!(
        watcher.modes,
        [PointerKind::Absolute, PointerKind::Relative]
    );
    let actions = injector.0.0.lock().unwrap().clone();
    assert!(
        actions.contains(&InputAction::MouseMoveBy {
            dx: 5 * 256,
            dy: -3 * 256
        }),
        "{actions:?}"
    );
}
