//! End-to-end tests that stream AV1 video from a host to a client over a real
//! QUIC connection on the loopback interface.

use std::f32::consts::TAU;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use qdp::auth::scram::ScramCredentials;
use qdp::hid::ReportDescriptor;
use qdp::message::{
    BusType, CursorImage, DesktopRect, DeviceFeatures, DeviceId, FeatureOperation, FeatureStatus,
    GamepadButtons, GamepadOutput, GamepadReport, KeyboardInfo, LayoutPlatform, PassthroughInfo,
    PenFlags, PenReport, PointerKind, PointerMode, TouchpadButtons, TouchpadContact,
    TouchpadContactFlags, TouchpadInfo, TouchpadReport,
};
use qdp::wire::{SignedVarInt, VarInt};
use qdp_codec::{CodecPreference, Yuv420Frame};
use qdp_quic::{ClientEndpoint, HostEndpoint, HostVerification, KeyPin, TlsIdentity};
use qdp_runtime::{
    ActiveDevice, Admission, AudioInput, AudioOutput, CaptureSink, CapturedFrame, ClientAudio,
    ClientCredentials, ClientOptions, Clipboard, ClipboardDirections, CursorShape, CursorState,
    CursorWarp, DesktopScale, DisplaySize, DisplayTiming, EncoderTuning, FeatureRequest,
    FrameArrivals, FrameInfo, FrameSink, FrameSource, GamepadFeedback, HostAudio,
    HostAuthenticator, HostDesktop, HostOptions, InputAction, InputHandle, InputInjector,
    InputQueue, Microphone, NoInput, OpenedDesktop, PassthroughDevice, PassthroughFeedback,
    PassthroughPolicy, PenInput, PixelRect, Playout, PointerButton, PointerPosition, Presentation,
    PresentationTiming, RefreshMode, ResumptionSlot, RuntimeError, SAMPLE_RATE, SharedClipboard,
    SinkControl, TestPattern, TouchPhase, TouchPoint, UserInput, WantedDisplay, WarpKind,
    input_channel, input_classes, run, serve,
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
    /// The size the host scales the pattern to, if it does.
    scaled: Option<(usize, usize)>,
    /// The number and size of each presented frame.
    sizes: Vec<(u64, (usize, usize))>,
    /// The display to change to after the given number of frames, if any.
    resize: Option<(usize, DisplaySize)>,
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

    fn capture(&mut self) -> Result<CapturedFrame, RuntimeError> {
        self.captures += 1;
        self.pattern.capture()
    }

    fn cursor(&mut self) -> Option<CursorState> {
        let shape = &self.shapes[usize::from(self.captures > SHAPE_CHANGE)];
        Some(CursorState {
            position: PointerPosition::At(CURSOR_POSITION.0, CURSOR_POSITION.1),
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
        // A drag: the button stays held while the pointer moves.
        UserInput::Button {
            button: PointerButton::Left,
            pressed: true,
        },
        UserInput::PointerMoved { x: 20.0, y: 30.0 },
        UserInput::Button {
            button: PointerButton::Left,
            pressed: false,
        },
        UserInput::Scroll {
            vertical: 120,
            horizontal: 0,
        },
        touch(TouchPhase::Down, 30.0),
        touch(TouchPhase::Move, 31.0),
        touch(TouchPhase::Up, 31.0),
        pen(PenFlags::IN_RANGE),
        pen(PenFlags(PenFlags::IN_RANGE.0 | PenFlags::TIP_SWITCH.0)),
        UserInput::Text("hé".to_owned()),
    ]
}

/// Returns the input test's pen at (50, 60) with `flags`, tilted 12.5
/// degrees right and pressing a quarter.
fn pen(flags: PenFlags) -> UserInput {
    UserInput::Pen(PenInput {
        x: 50.0,
        y: 60.0,
        flags,
        pressure: 0.25,
        tilt_x: 12.5,
        tilt_y: 0.0,
        twist: 0.0,
    })
}

/// Returns the state the host gives its pen for [`pen`] with `flags`.
fn pen_state(flags: PenFlags) -> InputAction {
    InputAction::Pen(PenReport {
        flags,
        x: 50 * 256,
        y: 60 * 256,
        pressure: 16_383,
        tilt_x: 1250,
        tilt_y: 0,
        twist: 0,
    })
}

/// The finger the input test touches with.
const FINGER: u64 = 7;

/// Returns the input test's finger doing `phase` at `x` pixels across and 40
/// down, pressing halfway.
fn touch(phase: TouchPhase, x: f64) -> UserInput {
    UserInput::Touch {
        finger: FINGER,
        phase,
        x,
        y: 40.0,
        pressure: Some(0.5),
    }
}

/// Returns the frame the host performs for the input test's finger doing
/// `phase` at `x` pixels across.
fn touch_frame(phase: TouchPhase, x: i32) -> InputAction {
    InputAction::Touch(vec![TouchPoint {
        id: 0,
        phase,
        x: x * 256,
        y: 40 * 256,
        pressure: 32_767,
    }])
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
        InputAction::PointerMoveTo {
            x: 20 * 256,
            y: 30 * 256,
        },
        InputAction::ButtonUp(PointerButton::Left),
        InputAction::Scroll {
            vertical: 120,
            horizontal: 0,
        },
        touch_frame(TouchPhase::Down, 30),
        touch_frame(TouchPhase::Move, 31),
        touch_frame(TouchPhase::Up, 31),
        pen_state(PenFlags::IN_RANGE),
        pen_state(PenFlags(PenFlags::IN_RANGE.0 | PenFlags::TIP_SWITCH.0)),
        InputAction::Text("hé".to_owned()),
        // The session ends with the pen still touching, so the host takes it
        // out of range.
        InputAction::Pen(PenReport {
            flags: PenFlags::default(),
            x: 50 * 256,
            y: 60 * 256,
            pressure: 0,
            tilt_x: 1250,
            tilt_y: 0,
            twist: 0,
        }),
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

    fn supports_touch(&self) -> bool {
        true
    }

    fn supports_pen(&self) -> bool {
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
        let (width, height) = self.scaled.unwrap_or((WIDTH, HEIGHT));
        let expected = TestPattern::frame(WIDTH, HEIGHT, number - 1).scaled(width, height);
        let psnr = frame.luma_psnr(&expected).unwrap_or(0.0);
        self.frames.push((number, psnr, info));
        self.sizes.push((number, (frame.width(), frame.height())));
        if let (Some((after, display)), Some(input)) = (self.resize, &self.input) {
            if self.frames.len() == after {
                assert!(input.send(UserInput::Display(display)));
            }
        }
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
    start_host_with(
        authenticator,
        host_desktop(source, injector, clipboard, audio),
    )
}

/// Returns a host desktop that opens `source` and `injector`.
fn host_desktop(
    source: Box<dyn FrameSource>,
    injector: Box<dyn InputInjector>,
    clipboard: Option<SharedClipboard>,
    audio: HostAudio,
) -> HostDesktop {
    HostDesktop {
        input_classes: input_classes(&*injector, PassthroughPolicy::default()),
        clipboard,
        audio,
        open: Box::new(move || {
            Box::pin(async move {
                Ok(OpenedDesktop {
                    source,
                    injector,
                    lock: None,
                })
            })
        }),
    }
}

/// Starts a host serving `desktop` to one client, as [`start_host`] does.
fn start_host_with(
    authenticator: HostAuthenticator,
    desktop: HostDesktop,
) -> (
    SocketAddr,
    KeyPin,
    tokio::task::JoinHandle<Result<qdp_runtime::HostSummary, RuntimeError>>,
) {
    let identity = TlsIdentity::generate_self_signed(vec![SERVER_NAME.to_owned()]).unwrap();
    let pin = identity.key_pin().unwrap();
    let endpoint = HostEndpoint::bind(loopback(), identity).unwrap();
    let address = endpoint.local_address().unwrap();
    let options = host_options(authenticator);
    let task = tokio::spawn(async move {
        let incoming = endpoint.accept().await.expect("a client");
        let ticket = Admission::default()
            .admit(incoming.remote_address().ip())
            .expect("the client is admitted");
        let connection = incoming.establish().await?;
        serve(connection, ticket, &options, desktop).await
    });
    (address, pin, task)
}

/// Returns the options of a test host that accepts clients with
/// `authenticator`.
fn host_options(authenticator: HostAuthenticator) -> HostOptions {
    HostOptions {
        host_name: "loopback".to_owned(),
        authenticator,
        frame_rate: 60,
        encoder: EncoderTuning {
            preference: CodecPreference::Software,
            speed: 10,
            quantizer: 60,
            threads: 2,
        },
        passthrough: PassthroughPolicy::default(),
    }
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
            scale_percent: DisplaySize::UNSCALED,
        },
        decoder_threads: 2,
        decoder: CodecPreference::Software,
        decoder_process: None,
        keyboard_info: Some(KeyboardInfo {
            language: "en-GB".to_owned(),
            layout_platform: LayoutPlatform::XKB,
            layout: "gb".to_owned(),
        }),
        resumption: ResumptionSlot::default(),
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
    stream_with(address, pin, client_options(credentials), input, None).await
}

/// Connects a client with `options`, presenting `identity` if given, and
/// streams until the collector stops, sending the test input if `input` is
/// given.
async fn stream_with(
    address: SocketAddr,
    pin: KeyPin,
    options: ClientOptions,
    input: Option<(InputHandle, InputQueue)>,
    identity: Option<TlsIdentity>,
) -> Result<Collector, RuntimeError> {
    let display = options.display;
    let mut endpoint = ClientEndpoint::bind(loopback()).unwrap();
    if let Some(identity) = identity {
        endpoint.set_identity(identity);
    }
    let connection = endpoint
        .connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![pin]),
        )
        .await?;
    let (handle, mut queue) = input.unzip();
    let full_size = (display.width as usize, display.height as usize) >= (WIDTH, HEIGHT);
    let mut collector = Collector {
        input: handle,
        scaled: (!full_size).then_some((display.width as usize, display.height as usize)),
        ..Collector::default()
    };
    let summary = run(
        connection,
        &options,
        &mut collector,
        queue.as_mut(),
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

/// How often [`ArrivingPattern`]'s frames arrive: faster than the stream's 60
/// frames per second, so that the host must pace them.
const ARRIVAL_SPACING: Duration = Duration::from_millis(5);

/// A test pattern that says when its frames arrive, as a screen capture does,
/// from a thread of its own, until it is dropped.
struct ArrivingPattern {
    /// The frames.
    pattern: TestPattern,
    /// Whether the host watched for arrivals.
    watched: Arc<AtomicBool>,
    /// Stops the arrivals.
    stop: Arc<AtomicBool>,
}

impl FrameSource for ArrivingPattern {
    fn size(&self) -> (usize, usize) {
        self.pattern.size()
    }

    fn capture(&mut self) -> Result<CapturedFrame, RuntimeError> {
        self.pattern.capture()
    }

    fn watch_frames(&mut self, arrivals: Arc<FrameArrivals>) -> bool {
        self.watched.store(true, Ordering::Release);
        let stop = Arc::clone(&self.stop);
        std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                arrivals.arrived();
                std::thread::sleep(ARRIVAL_SPACING);
            }
        });
        true
    }
}

impl Drop for ArrivingPattern {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// A source that says when its frames arrive is captured as they do, paced
/// to the stream's frame rate, and streams as faithfully as one captured at
/// frame times.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn frames_go_out_as_they_arrive() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let watched = Arc::new(AtomicBool::new(false));
    let source = ArrivingPattern {
        pattern: TestPattern::new(WIDTH, HEIGHT),
        watched: Arc::clone(&watched),
        stop: Arc::new(AtomicBool::new(false)),
    };
    let (address, pin, host) = start_host(
        authenticator,
        Box::new(source),
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
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    assert!(watched.load(Ordering::Acquire));
}

/// How long the slow client takes to present each frame: longer than a frame
/// interval at 60 frames per second.
const SLOW_PRESENT: Duration = Duration::from_millis(40);

/// The frames the slow client presents.
const SLOW_FRAMES: usize = 60;

/// The longest the slow client's frames may take from capture to decoding,
/// past the first. Were the host to send every frame, they would queue behind
/// the decoder, each waiting about 24 ms longer than the last.
const SLOW_MAX_LATENCY: Duration = Duration::from_millis(500);

/// A client that takes [`SLOW_PRESENT`] to present each frame.
#[derive(Default)]
struct SlowSink {
    /// The latency of each frame presented, once known.
    latencies: Vec<Option<Duration>>,
}

impl FrameSink for SlowSink {
    fn present(&mut self, _frame: &Yuv420Frame, info: FrameInfo) -> SinkControl {
        std::thread::sleep(SLOW_PRESENT);
        self.latencies.push(info.latency);
        if self.latencies.len() >= SLOW_FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }
}

/// A client slower than the stream's frame rate receives frames only as fast
/// as it decodes them, so that each stays current rather than waiting longer
/// than the last behind the decoder (Section 6.11).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slow_client_stays_current() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let (address, pin, host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(NoInput),
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
    let mut sink = SlowSink::default();
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut sink,
            None,
            None,
            ClientAudio::default(),
        ),
    )
    .await
    .expect("streaming finishes in time")
    .unwrap();
    endpoint.wait_idle().await;
    // Past the first frames, no frame waits long: without the host holding
    // frames back, each would wait longer than the last, until the client
    // dropped them all and started again from a key frame.
    let latest: Vec<Duration> = sink.latencies[SLOW_FRAMES / 3..]
        .iter()
        .map(|latency| latency.expect("the clocks are synchronized by then"))
        .collect();
    assert!(
        latest.iter().all(|&latency| latency < SLOW_MAX_LATENCY),
        "the latest frames took {latest:?}"
    );
    let summary = tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    // The host sent about as many frames as the client presented, not the
    // 60 a second it captures.
    assert!(
        summary.frames_sent < 2 * SLOW_FRAMES as u64,
        "{} frames sent",
        summary.frames_sent
    );
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

/// A client with the wrong password is refused, and the host's desktop is
/// never opened for it (Section 11.2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_password_is_refused() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_user(
        "ian".to_owned(),
        ScramCredentials::derive("pencil", b"streaming salt".to_vec(), 4096).unwrap(),
    );
    let opened = Arc::new(AtomicBool::new(false));
    let desktop = HostDesktop {
        open: Box::new({
            let opened = Arc::clone(&opened);
            move || {
                opened.store(true, Ordering::SeqCst);
                Box::pin(async {
                    Ok(OpenedDesktop {
                        source: test_pattern(),
                        injector: Box::new(NoInput),
                        lock: None,
                    })
                })
            }
        }),
        ..host_desktop(
            test_pattern(),
            Box::new(NoInput),
            None,
            HostAudio::default(),
        )
    };
    let (address, pin, _host) = start_host_with(authenticator, desktop);
    let credentials = ClientCredentials::Password {
        username: "ian".to_owned(),
        password: "wrong".to_owned(),
    };
    let result = tokio::time::timeout(TEST_TIMEOUT, stream(address, pin, credentials, None))
        .await
        .expect("the attempt finishes in time");
    assert!(matches!(result, Err(RuntimeError::AuthenticationFailed(_))));
    assert!(!opened.load(Ordering::SeqCst));
}

/// Keys, pointer, buttons (a drag among them), scrolling and text sent by
/// the client are performed on the host, in order.
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

/// A client whose display is half the size of the host's monitor receives
/// video scaled to fit it, and its input lands where it points on the
/// monitor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn video_scales_to_the_display() {
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
    let display = DisplaySize {
        width: WIDTH as u32 / 2,
        height: HEIGHT as u32 / 2,
        refresh_millihertz: 60_000,
        scale_percent: DisplaySize::UNSCALED,
    };
    let options = ClientOptions {
        display,
        ..client_options(ClientCredentials::Token(TOKEN.to_vec()))
    };
    let collector = tokio::time::timeout(
        TEST_TIMEOUT,
        stream_with(address, pin, options, Some(input_channel()), None),
    )
    .await
    .expect("streaming finishes in time")
    .unwrap();
    check_frames(&collector);
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    let actions = recorder.0.lock().unwrap().clone();
    // The client pointed at (10, 20) on the half-size video.
    assert!(actions.contains(&InputAction::PointerMoveTo {
        x: 20 * 256,
        y: 40 * 256
    }));
}

/// A client whose display shrinks during the session gets a new stream at
/// the new size, starting again from Frame Number 1, without a decoding
/// failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn display_changes_during_the_session() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let (address, pin, host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(NoInput),
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
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    let half = DisplaySize {
        width: WIDTH as u32 / 2,
        height: HEIGHT as u32 / 2,
        ..options.display
    };
    let (handle, mut queue) = input_channel();
    let mut collector = Collector {
        input: Some(handle),
        resize: Some((INPUT_FRAME, half)),
        ..Collector::default()
    };
    let summary = tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut collector,
            Some(&mut queue),
            None,
            ClientAudio::default(),
        ),
    )
    .await
    .expect("streaming finishes in time")
    .unwrap();
    assert_eq!(summary.decode_failures, 0);
    endpoint.wait_idle().await;
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();

    let full = (WIDTH, HEIGHT);
    let smaller = (WIDTH / 2, HEIGHT / 2);
    let changed = collector
        .sizes
        .iter()
        .position(|(_, size)| *size == smaller)
        .expect("frames of the new size arrive");
    assert!(changed >= INPUT_FRAME);
    assert!(
        collector.sizes[..changed]
            .iter()
            .all(|(_, size)| *size == full)
    );
    assert!(
        collector.sizes[changed..]
            .iter()
            .all(|(_, size)| *size == smaller)
    );
    let numbers: Vec<u64> = collector.sizes[changed..]
        .iter()
        .map(|(number, _)| *number)
        .collect();
    assert_eq!(numbers, (1..=numbers.len() as u64).collect::<Vec<_>>());
}

/// A test pattern that takes the size of the client's display whenever the
/// host asks, as a virtual monitor does.
struct MatchingPattern {
    /// The frames, at the size last asked for.
    pattern: TestPattern,
    /// The interface scale last asked for, once asked.
    scale: Option<DesktopScale>,
    /// The displays the host asked for.
    asked: Arc<Mutex<Vec<WantedDisplay>>>,
}

impl FrameSource for MatchingPattern {
    fn size(&self) -> (usize, usize) {
        self.pattern.size()
    }

    fn capture(&mut self) -> Result<CapturedFrame, RuntimeError> {
        self.pattern.capture()
    }

    fn desktop_scale(&self) -> Option<DesktopScale> {
        self.scale
    }

    fn match_display(&mut self, display: &WantedDisplay) -> bool {
        self.asked.lock().unwrap().push(*display);
        self.pattern = TestPattern::new(display.width as usize, display.height as usize);
        self.scale = DesktopScale::from_widths(display.scale_percent as usize, 100);
        true
    }
}

/// Keeps the interface scales the host tells the injector.
#[derive(Debug, Clone, Default)]
struct ScaleRecorder(Arc<Mutex<Vec<DesktopScale>>>);

impl InputInjector for ScaleRecorder {
    fn inject(&mut self, _action: &InputAction) -> Result<(), String> {
        Ok(())
    }

    fn description(&self) -> &str {
        "scale recorder"
    }

    fn set_desktop_scale(&mut self, scale: DesktopScale) {
        self.0.lock().unwrap().push(scale);
    }
}

/// A source that can take the client's display is asked for it before the
/// first frame and again when the display changes, and the video is its
/// frames at their own size: no scaling, so not the pattern's shape. The
/// input follows the scale the source takes with each display.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_monitor_follows_the_display() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let source = MatchingPattern {
        pattern: TestPattern::new(WIDTH, HEIGHT),
        scale: None,
        asked: Arc::clone(&asked),
    };
    let scales = ScaleRecorder::default();
    let (address, pin, host) = start_host(
        authenticator,
        Box::new(source),
        Box::new(scales.clone()),
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
    // Neither display has the pattern's shape, which scaling would keep.
    let first = DisplaySize {
        width: 192,
        height: 128,
        refresh_millihertz: 60_000,
        scale_percent: 150,
    };
    let second = DisplaySize {
        width: 160,
        height: 96,
        scale_percent: 200,
        ..first
    };
    let options = ClientOptions {
        display: first,
        ..client_options(ClientCredentials::Token(TOKEN.to_vec()))
    };
    let (handle, mut queue) = input_channel();
    let mut collector = Collector {
        input: Some(handle),
        resize: Some((INPUT_FRAME, second)),
        ..Collector::default()
    };
    let summary = tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut collector,
            Some(&mut queue),
            None,
            ClientAudio::default(),
        ),
    )
    .await
    .expect("streaming finishes in time")
    .unwrap();
    assert_eq!(summary.decode_failures, 0);
    endpoint.wait_idle().await;
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();

    let wanted = |display: DisplaySize| WantedDisplay {
        width: display.width,
        height: display.height,
        refresh_millihertz: display.refresh_millihertz,
        scale_percent: display.scale_percent,
    };
    assert_eq!(*asked.lock().unwrap(), [wanted(first), wanted(second)]);
    let sizes: Vec<(usize, usize)> = collector.sizes.iter().map(|(_, size)| *size).collect();
    assert!(
        sizes[..INPUT_FRAME].iter().all(|&size| size == (192, 128)),
        "{sizes:?}"
    );
    assert_eq!(sizes.last(), Some(&(160, 96)), "{sizes:?}");
    let percents: Vec<u32> = scales
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|scale| scale.percent())
        .collect();
    assert_eq!(percents, [150, 200]);
}

/// A test pattern whose monitor shrinks to half its size after a few
/// captures, as when a local user changes the resolution.
struct ShrinkingPattern {
    /// The captures so far.
    captures: usize,
}

impl FrameSource for ShrinkingPattern {
    fn size(&self) -> (usize, usize) {
        if self.captures < INPUT_FRAME {
            (WIDTH, HEIGHT)
        } else {
            (WIDTH / 2, HEIGHT / 2)
        }
    }

    fn capture(&mut self) -> Result<CapturedFrame, RuntimeError> {
        let (width, height) = self.size();
        self.captures += 1;
        Ok(TestPattern::frame(width, height, self.captures as u64).into())
    }
}

/// When the host's monitor changes size, the host follows it on its own
/// with a new stream of the new size.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_follows_its_monitor() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let (address, pin, host) = start_host(
        authenticator,
        Box::new(ShrinkingPattern { captures: 0 }),
        Box::new(NoInput),
        None,
        HostAudio::default(),
    );
    let credentials = ClientCredentials::Token(TOKEN.to_vec());
    let collector = tokio::time::timeout(TEST_TIMEOUT, stream(address, pin, credentials, None))
        .await
        .expect("streaming finishes in time")
        .unwrap();
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    let smaller = (WIDTH / 2, HEIGHT / 2);
    let changed = collector
        .sizes
        .iter()
        .position(|(_, size)| *size == smaller)
        .expect("frames of the new size arrive");
    assert!(changed > 0);
    assert_eq!(collector.sizes[changed].0, 1);
    assert!(
        collector.sizes[changed..]
            .iter()
            .all(|(_, size)| *size == smaller)
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
        microphone_state: Microphone::On,
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

/// Turns the client's microphone on during the session and stops once the
/// host has heard it.
struct MicrophoneSwitch {
    /// Where to turn the microphone on.
    input: InputHandle,
    /// The host's virtual microphone.
    host_microphone: RecordingOutput,
    /// The frames presented.
    frames: usize,
    /// What the host had heard when the microphone was turned on.
    heard_before: Option<usize>,
}

impl FrameSink for MicrophoneSwitch {
    fn present(&mut self, _frame: &Yuv420Frame, _info: FrameInfo) -> SinkControl {
        self.frames += 1;
        let heard = self.host_microphone.recorded.lock().unwrap().len();
        if self.frames == INPUT_FRAME {
            self.heard_before = Some(heard);
            assert!(self.input.send(UserInput::Microphone(Microphone::On)));
        }
        if heard >= SAMPLE_RATE as usize / 2 || self.frames >= CLIPBOARD_FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }
}

/// A microphone that starts off sends nothing until the user turns it on
/// during the session; the host then hears it (Section 7.7).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn microphone_turns_on_during_the_session() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let host_microphone = RecordingOutput::new(1);
    let host_audio = HostAudio {
        desktop: None,
        microphone: Some(Box::new(host_microphone.clone())),
    };
    let (address, pin, _host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(NoInput),
        None,
        host_audio,
    );
    let client_audio = ClientAudio {
        speakers: None,
        microphone: Some(Box::new(ToneInput {
            channels: 1,
            frequency: 220.0,
        })),
        microphone_state: Microphone::Off,
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
    let (input, mut queue) = input_channel();
    let mut switch = MicrophoneSwitch {
        input,
        host_microphone: host_microphone.clone(),
        frames: 0,
        heard_before: None,
    };
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut switch,
            Some(&mut queue),
            None,
            client_audio,
        ),
    )
    .await
    .expect("the session ends in time")
    .unwrap();
    assert_eq!(switch.heard_before, Some(0));
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

    fn detects_pointer_capture(&self) -> bool {
        true
    }
}

/// An injector that records actions on a platform that does not tell when
/// applications capture the pointer.
#[derive(Debug, Clone, Default)]
struct UntoldCapture(Recorder);

impl InputInjector for UntoldCapture {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        self.0.inject(action)
    }

    fn description(&self) -> &str {
        "untold capture"
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

/// An injector whose applications confine the visible pointer to a part of
/// the monitor.
#[derive(Debug, Clone, Default)]
struct ConfinedPointer(Recorder);

impl InputInjector for ConfinedPointer {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        self.0.inject(action)
    }

    fn description(&self) -> &str {
        "confined pointer"
    }

    fn pointer_confinement(&mut self) -> Option<PixelRect> {
        Some(PixelRect {
            x: 10,
            y: 20,
            width: 100,
            height: 50,
        })
    }
}

/// Stops once the host has confined the pointer.
struct ConfinementWatcher {
    /// The confinements the host announced.
    confinements: Vec<Option<DesktopRect>>,
    /// The frames presented.
    frames: usize,
}

impl FrameSink for ConfinementWatcher {
    fn present(&mut self, _frame: &Yuv420Frame, _info: FrameInfo) -> SinkControl {
        self.frames += 1;
        if self.confinements.iter().any(Option::is_some) || self.frames >= CLIPBOARD_FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }

    fn set_pointer_mode(&mut self, mode: &PointerMode) {
        assert_eq!(mode.mode, PointerKind::Absolute);
        self.confinements.push(mode.confinement);
    }
}

/// When an application confines the host's visible pointer, the client is
/// told the area, in positions of the host desktop (Section 8.9).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn confinement_reaches_the_client() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let (address, pin, _host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(ConfinedPointer::default()),
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
    let mut watcher = ConfinementWatcher {
        confinements: Vec::new(),
        frames: 0,
    };
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut watcher,
            None,
            None,
            ClientAudio::default(),
        ),
    )
    .await
    .expect("the session finishes in time")
    .unwrap();
    let position = |pixels: i64| SignedVarInt::new(pixels * 256).unwrap();
    assert_eq!(
        watcher.confinements.last().copied().flatten(),
        Some(DesktopRect {
            x: position(10),
            y: position(20),
            width: VarInt::from_u32(100 * 256),
            height: VarInt::from_u32(50 * 256),
        })
    );
}

/// When an application captures the host's pointer, the client switches to
/// relative mode and its motion arrives as relative moves.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relative_mode_moves_the_mouse() {
    let injector = CapturedPointer::default();
    let actions = relative_motion(test_pattern(), Box::new(injector.clone()), &injector.0).await;
    assert!(
        actions.contains(&InputAction::MouseMoveBy {
            dx: 5 * 256,
            dy: -3 * 256
        }),
        "{actions:?}"
    );
}

/// Where the platform does not tell, a pointer an application keeps hidden
/// over the monitor, as a game drawing its own does, counts as captured.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hidden_pointer_is_captured() {
    let source = CursorPattern {
        pattern: TestPattern::new(WIDTH, HEIGHT),
        captures: 0,
        // Fully transparent after the change: the game hid the pointer.
        shapes: [pointer_shape(4, 0x11), pointer_shape(6, 0)],
    };
    let injector = UntoldCapture::default();
    let actions = relative_motion(Box::new(source), Box::new(injector.clone()), &injector.0).await;
    assert!(
        actions.contains(&InputAction::MouseMoveBy {
            dx: 5 * 256,
            dy: -3 * 256
        }),
        "{actions:?}"
    );
}

/// Streams `source` to a client until the host switches to relative mode
/// and the client's motion follows, and returns the actions `recorder`
/// recorded through `injector`.
async fn relative_motion(
    source: Box<dyn FrameSource>,
    injector: Box<dyn InputInjector>,
    recorder: &Recorder,
) -> Vec<InputAction> {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let (address, pin, _host) =
        start_host(authenticator, source, injector, None, HostAudio::default());
    let endpoint = ClientEndpoint::bind(loopback()).unwrap();
    let connection = endpoint
        .connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![pin]),
        )
        .await
        .unwrap();
    let (handle, mut queue) = input_channel();
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
            Some(&mut queue),
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
    recorder.0.lock().unwrap().clone()
}

/// The rumble the host's applications set on every gamepad they hear from.
const RUMBLE: GamepadOutput = GamepadOutput {
    low_frequency_rumble: 30_000,
    high_frequency_rumble: 10_000,
    left_trigger_rumble: 0,
    right_trigger_rumble: 0,
    led_red: 0,
    led_green: 0,
    led_blue: 0,
};

/// An injector that records actions and presents gamepads, which the host's
/// applications rumble once they hear from them.
#[derive(Debug, Clone, Default)]
struct RumblingGamepads {
    /// The actions performed.
    recorder: Recorder,
    /// The rumble not yet reported to the host.
    outputs: Vec<(DeviceId, GamepadOutput)>,
}

impl InputInjector for RumblingGamepads {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        if let InputAction::Gamepad { device_id, .. } = action {
            self.outputs.push((*device_id, RUMBLE));
        }
        self.recorder.inject(action)
    }

    fn description(&self) -> &str {
        "rumbling gamepads"
    }

    fn supports_gamepads(&self) -> bool {
        true
    }

    fn gamepad_outputs(&mut self) -> Vec<(DeviceId, GamepadOutput)> {
        std::mem::take(&mut self.outputs)
    }
}

/// The report the test's gamepad sends: A held, the left stick pushed up and
/// the right trigger fully pressed.
const GAMEPAD_STATE: GamepadReport = GamepadReport {
    buttons: GamepadButtons(1),
    left_trigger: 0,
    right_trigger: u16::MAX,
    left_stick_x: 0,
    left_stick_y: -20_000,
    right_stick_x: 0,
    right_stick_y: 0,
    gyro: [0; 3],
    accel: [0; 3],
};

/// Connects a gamepad, moves it, and streams until its rumble comes back.
struct GamepadWatcher {
    /// Where the input goes.
    input: InputHandle,
    /// The rumble the client was asked to play.
    feedback: Vec<GamepadFeedback>,
    /// The frames presented.
    frames: usize,
}

impl FrameSink for GamepadWatcher {
    fn present(&mut self, _frame: &Yuv420Frame, _info: FrameInfo) -> SinkControl {
        self.frames += 1;
        if self.frames == INPUT_FRAME {
            assert!(self.input.send(UserInput::GamepadConnected {
                slot: 0,
                features: DeviceFeatures::GAMEPAD_RUMBLE,
                name: "Test pad".to_owned(),
            }));
            assert!(self.input.send(UserInput::Gamepad {
                slot: 0,
                report: GAMEPAD_STATE,
            }));
        }
        self.feedback
            .extend(std::iter::from_fn(|| self.input.try_gamepad_feedback()));
        if !self.feedback.is_empty() || self.frames >= CLIPBOARD_FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }
}

/// A gamepad connected on the client appears on the host, and the rumble the
/// host's applications set comes back to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gamepad_rumbles() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let injector = RumblingGamepads::default();
    let (address, pin, host) = start_host(
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
    let (handle, mut queue) = input_channel();
    let mut watcher = GamepadWatcher {
        input: handle,
        feedback: Vec::new(),
        frames: 0,
    };
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut watcher,
            Some(&mut queue),
            None,
            ClientAudio::default(),
        ),
    )
    .await
    .expect("the session finishes in time")
    .unwrap();
    endpoint.wait_idle().await;
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    assert_eq!(
        watcher.feedback,
        [GamepadFeedback {
            slot: 0,
            output: RUMBLE
        }]
    );
    let actions = injector.recorder.0.lock().unwrap().clone();
    let mut held = GAMEPAD_STATE;
    // The client sets the trigger's button from its value.
    held.buttons.0 |= 1 << 7;
    let [
        InputAction::Gamepad { device_id, report },
        InputAction::Gamepad {
            report: rest,
            device_id: rest_id,
        },
    ] = actions.as_slice()
    else {
        panic!("{actions:?}");
    };
    assert_eq!(*report, held);
    assert_eq!((*rest, rest_id), (GamepadReport::default(), device_id));
}

/// A gamepad's report descriptor: 16 buttons, two 8-bit axes and a hat
/// switch in input report 1, and a 4-byte output report 2.
const PASSTHROUGH_GAMEPAD: &[u8] = &[
    0x05, 0x01, 0x09, 0x05, 0xa1, 0x01, 0x85, 0x01, 0x05, 0x09, 0x19, 0x01, 0x29, 0x10, 0x15, 0x00,
    0x25, 0x01, 0x75, 0x01, 0x95, 0x10, 0x81, 0x02, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00,
    0x26, 0xff, 0x00, 0x75, 0x08, 0x95, 0x02, 0x81, 0x02, 0x09, 0x39, 0x15, 0x00, 0x25, 0x07, 0x75,
    0x04, 0x95, 0x01, 0x81, 0x42, 0x81, 0x03, 0x85, 0x02, 0x06, 0x00, 0xff, 0x09, 0x01, 0x26, 0xff,
    0x00, 0x75, 0x08, 0x95, 0x04, 0x91, 0x02, 0xc0,
];

/// The boot keyboard's report descriptor, which the host's policy refuses.
const PASSTHROUGH_KEYBOARD: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0xc0,
];

/// The input report the client's passthrough gamepad sends: button 1 held.
const PASSTHROUGH_REPORT: &[u8] = &[0x01, 0x01, 0x00, 0x80, 0x80, 0x08];

/// The output report the host's applications send the gamepad.
const PASSTHROUGH_OUTPUT: &[u8] = &[0x02, 0xaa, 0xbb, 0xcc, 0xdd];

/// The token of the host application's feature report read.
const FEATURE_TOKEN: u64 = 9;

/// The feature report the client's gamepad returns.
const FEATURE_REPORT: &[u8] = &[0x03, 0x55];

/// What the passthrough host saw.
#[derive(Debug, Default)]
struct PassthroughLog {
    /// The names of the devices it presented.
    attached: Vec<String>,
    /// The input reports delivered.
    reports: Vec<Vec<u8>>,
    /// The answers to its feature request.
    answers: Vec<(u64, FeatureStatus, Vec<u8>)>,
}

/// An injector presenting passthrough devices, whose applications send an
/// output report and read a feature report once a device reports.
#[derive(Debug, Clone, Default)]
struct PassthroughHost {
    /// What it saw, shared with the test.
    log: Arc<Mutex<PassthroughLog>>,
    /// The output reports and feature requests waiting to be collected.
    pending: Option<(DeviceId, bool)>,
}

impl InputInjector for PassthroughHost {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        if let InputAction::PassthroughReport { device_id, report } = action {
            let mut log = self.log.lock().unwrap();
            if log.reports.is_empty() {
                self.pending = Some((*device_id, true));
            }
            log.reports.push(report.clone());
        }
        Ok(())
    }

    fn description(&self) -> &str {
        "passthrough host"
    }

    fn supports_passthrough(&self) -> bool {
        true
    }

    fn attach_passthrough(&mut self, device: &PassthroughDevice) -> Result<(), String> {
        self.log.lock().unwrap().attached.push(device.name.clone());
        Ok(())
    }

    fn passthrough_outputs(&mut self) -> Vec<(DeviceId, Vec<u8>)> {
        match &mut self.pending {
            Some((device_id, output @ true)) => {
                *output = false;
                vec![(*device_id, PASSTHROUGH_OUTPUT.to_vec())]
            }
            _ => Vec::new(),
        }
    }

    fn feature_requests(&mut self) -> Vec<FeatureRequest> {
        match self.pending.take() {
            Some((device_id, false)) => vec![FeatureRequest {
                device_id,
                token: FEATURE_TOKEN,
                operation: FeatureOperation::Get,
                report: vec![FEATURE_REPORT[0]],
            }],
            other => {
                self.pending = other;
                Vec::new()
            }
        }
    }

    fn answer_feature(&mut self, token: u64, status: FeatureStatus, report: &[u8]) {
        self.log
            .lock()
            .unwrap()
            .answers
            .push((token, status, report.to_vec()));
    }
}

/// Returns the passthrough information of a device with `descriptor`.
fn passthrough_info(descriptor: &[u8]) -> PassthroughInfo {
    let rest_reports = ReportDescriptor::parse(descriptor)
        .map(|parsed| parsed.rest_reports())
        .unwrap_or_default();
    PassthroughInfo {
        bus_type: BusType::USB,
        vendor_id: 0x1234,
        product_id: 0x5678,
        version: 0x0100,
        report_descriptor: descriptor.to_vec(),
        rest_reports,
    }
}

/// Passes a gamepad and a keyboard through, reports from the gamepad, and
/// answers the host's feature request, streaming until both directions
/// have been seen.
struct PassthroughWatcher {
    /// Where the input goes.
    input: InputHandle,
    /// The output reports the client was asked to write.
    outputs: Vec<(u32, Vec<u8>)>,
    /// The frames presented since the feature request was answered.
    frames_since_answer: Option<usize>,
    /// The frames presented.
    frames: usize,
}

impl FrameSink for PassthroughWatcher {
    fn present(&mut self, _frame: &Yuv420Frame, _info: FrameInfo) -> SinkControl {
        self.frames += 1;
        if self.frames == INPUT_FRAME {
            for (slot, name, descriptor) in [
                (1, "Passthrough pad", PASSTHROUGH_GAMEPAD),
                (2, "Passthrough keyboard", PASSTHROUGH_KEYBOARD),
            ] {
                assert!(self.input.send(UserInput::PassthroughConnected {
                    slot,
                    name: name.to_owned(),
                    info: passthrough_info(descriptor),
                }));
            }
            // Sent before the host accepts the device, the report is held
            // back until it does.
            assert!(self.input.send(UserInput::PassthroughReport {
                slot: 1,
                report: PASSTHROUGH_REPORT.to_vec(),
            }));
        }
        while let Some(feedback) = self.input.try_passthrough_feedback() {
            match feedback {
                PassthroughFeedback::Output { slot, report } => {
                    self.outputs.push((slot, report));
                }
                PassthroughFeedback::FeatureRequest {
                    request_id, report, ..
                } => {
                    assert_eq!(report, [FEATURE_REPORT[0]]);
                    assert!(self.input.send(UserInput::FeatureResponse {
                        request_id,
                        status: FeatureStatus::Ok,
                        report: FEATURE_REPORT.to_vec(),
                    }));
                    self.frames_since_answer = Some(0);
                }
            }
        }
        if let Some(frames) = &mut self.frames_since_answer {
            *frames += 1;
        }
        if self.frames_since_answer.is_some_and(|frames| frames > 30)
            || self.frames >= CLIPBOARD_FRAMES
        {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }
}

/// A game controller passed through reaches the host, whose policy refuses
/// a keyboard; the host's output report and feature request reach the
/// client, the client's answer reaches the host, and the device returns to
/// rest when the session ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn passthrough_round_trip() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let injector = PassthroughHost::default();
    let (address, pin, host) = start_host(
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
    let (handle, mut queue) = input_channel();
    let mut watcher = PassthroughWatcher {
        input: handle,
        outputs: Vec::new(),
        frames_since_answer: None,
        frames: 0,
    };
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut watcher,
            Some(&mut queue),
            None,
            ClientAudio::default(),
        ),
    )
    .await
    .expect("the session finishes in time")
    .unwrap();
    endpoint.wait_idle().await;
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    assert_eq!(watcher.outputs, [(1, PASSTHROUGH_OUTPUT.to_vec())]);
    let log = injector.log.lock().unwrap();
    assert_eq!(log.attached, ["Passthrough pad"]);
    let rest = passthrough_info(PASSTHROUGH_GAMEPAD).rest_reports;
    assert_eq!(
        log.reports,
        [PASSTHROUGH_REPORT.to_vec(), rest[0].clone()],
        "the report, then the rest report when the session ends"
    );
    assert_eq!(
        log.answers,
        [(FEATURE_TOKEN, FeatureStatus::Ok, FEATURE_REPORT.to_vec())]
    );
}

/// An injector that records actions and presents raw touchpads.
#[derive(Debug, Clone, Default)]
struct TouchpadHost(Recorder);

impl InputInjector for TouchpadHost {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        self.0.inject(action)
    }

    fn description(&self) -> &str {
        "touchpad host"
    }

    fn supports_touchpad(&self) -> bool {
        true
    }
}

/// The size of the test touchpad's surface: 100 by 60 mm.
const TOUCHPAD_SURFACE: TouchpadInfo = TouchpadInfo {
    width: VarInt::from_u32(10_000),
    height: VarInt::from_u32(6_000),
};

/// Returns the test touchpad with one finger at `x` and the primary button
/// pressed.
fn touchpad_report(x: u16) -> TouchpadReport {
    TouchpadReport {
        buttons: TouchpadButtons::PRIMARY,
        contacts: vec![TouchpadContact {
            id: 0,
            flags: TouchpadContactFlags::CONFIDENCE,
            x,
            y: 30_000,
        }],
    }
}

/// Connects a touchpad and moves a finger on it.
struct TouchpadWatcher {
    /// Where the input goes.
    input: InputHandle,
    /// The frames presented.
    frames: usize,
}

impl FrameSink for TouchpadWatcher {
    fn present(&mut self, _frame: &Yuv420Frame, _info: FrameInfo) -> SinkControl {
        self.frames += 1;
        if self.frames == INPUT_FRAME {
            assert!(self.input.send(UserInput::TouchpadConnected {
                slot: 0,
                name: "Test touchpad".to_owned(),
                surface: TOUCHPAD_SURFACE,
                clickpad: true,
            }));
            assert!(self.input.send(UserInput::Touchpad {
                slot: 0,
                report: touchpad_report(10_000),
            }));
        }
        if self.frames == INPUT_FRAME * 3 {
            assert!(self.input.send(UserInput::Touchpad {
                slot: 0,
                report: touchpad_report(12_000),
            }));
        }
        if self.frames >= FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }
}

/// A raw touchpad's contacts and clicks reach a host that presents
/// touchpads, which lifts every finger when the session ends.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn touchpad_reaches_the_host() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let injector = TouchpadHost::default();
    let (address, pin, host) = start_host(
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
    let (handle, mut queue) = input_channel();
    let mut watcher = TouchpadWatcher {
        input: handle,
        frames: 0,
    };
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut watcher,
            Some(&mut queue),
            None,
            ClientAudio::default(),
        ),
    )
    .await
    .expect("the session finishes in time")
    .unwrap();
    endpoint.wait_idle().await;
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    let reports: Vec<TouchpadReport> = injector
        .0
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|action| match action {
            InputAction::Touchpad {
                surface,
                clickpad,
                report,
                ..
            } => {
                assert_eq!((*surface, *clickpad), (TOUCHPAD_SURFACE, true));
                Some(report.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        reports,
        [
            touchpad_report(10_000),
            touchpad_report(12_000),
            TouchpadReport::default()
        ]
    );
}

/// Streams frames until told to stop by the test.
struct Endless {
    /// The frames presented.
    frames: Arc<std::sync::atomic::AtomicUsize>,
}

impl FrameSink for Endless {
    fn present(&mut self, _frame: &Yuv420Frame, _info: FrameInfo) -> SinkControl {
        self.frames.fetch_add(1, Ordering::Relaxed);
        SinkControl::Continue
    }
}

/// A sink that shows frames later, as a window does: every other frame is
/// replaced before it is shown. It reports a display's refresh timing.
#[derive(Default)]
struct LaterSink {
    /// The frames given, oldest first, not yet reported.
    given: Vec<FrameInfo>,
    /// The frames given so far.
    count: usize,
    /// The presentations reported.
    reported: Vec<Presentation>,
}

impl FrameSink for LaterSink {
    fn present(&mut self, _frame: &Yuv420Frame, info: FrameInfo) -> SinkControl {
        self.given.push(info);
        self.count += 1;
        if self.count >= FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }

    fn presentation_timing(&self) -> PresentationTiming {
        PresentationTiming::Deferred
    }

    fn take_presentations(&mut self) -> Vec<Presentation> {
        let now = Instant::now();
        let presentations: Vec<Presentation> = self
            .given
            .drain(..)
            .map(|info| Presentation {
                stream_id: info.stream_id,
                frame_number: info.frame_number,
                presented_at: (info.frame_number.0.into_inner() % 2 == 0).then_some(now),
            })
            .collect();
        self.reported.extend(&presentations);
        presentations
    }

    fn display_timing(&mut self) -> Option<DisplayTiming> {
        Some(DisplayTiming {
            refresh_period: Duration::from_nanos(16_666_667),
            vsync: Instant::now(),
            mode: RefreshMode::Fixed,
        })
    }
}

/// A sink that shows frames later reports each of them, shown or replaced,
/// and the session runs to the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deferred_presentations_are_reported() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let (address, pin, host) = start_host(
        authenticator,
        test_pattern(),
        Box::new(NoInput),
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
    let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
    let mut sink = LaterSink::default();
    let summary = tokio::time::timeout(
        TEST_TIMEOUT,
        run(
            connection,
            &options,
            &mut sink,
            None,
            None,
            ClientAudio::default(),
        ),
    )
    .await
    .expect("streaming finishes in time")
    .unwrap();
    assert_eq!(summary.frames_presented, FRAMES as u64);
    endpoint.wait_idle().await;
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host finishes in time")
        .unwrap()
        .unwrap();
    // Every frame but the last few, given after the last report, was
    // reported in order.
    let numbers: Vec<u64> = sink
        .reported
        .iter()
        .map(|presentation| presentation.frame_number.0.into_inner())
        .collect();
    assert!(numbers.len() + sink.given.len() == FRAMES);
    assert_eq!(numbers, (1..=numbers.len() as u64).collect::<Vec<_>>());
}

/// A client that authenticates replaces the desktop's current session,
/// which the host closes with `QDP_SESSION_REPLACED` (Section 2.1). The
/// replaced session locks the desktop, and the one whose client
/// disconnected does not (Section 11.4).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn newer_session_replaces_older() {
    let identity = TlsIdentity::generate_self_signed(vec![SERVER_NAME.to_owned()]).unwrap();
    let pin = identity.key_pin().unwrap();
    let endpoint = HostEndpoint::bind(loopback(), identity).unwrap();
    let address = endpoint.local_address().unwrap();
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_token(TOKEN.to_vec());
    let options = Arc::new(host_options(authenticator));
    let admission = Admission::default();
    let locks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let host_locks = Arc::clone(&locks);
    let host = tokio::spawn(async move {
        let mut sessions = Vec::new();
        for _ in 0..2 {
            let incoming = endpoint.accept().await.expect("a client");
            let ticket = admission.admit(incoming.remote_address().ip()).unwrap();
            let options = Arc::clone(&options);
            let locks = Arc::clone(&host_locks);
            sessions.push(tokio::spawn(async move {
                let connection = incoming.establish().await.unwrap();
                let desktop = HostDesktop {
                    open: Box::new(move || {
                        Box::pin(async move {
                            Ok(OpenedDesktop {
                                source: test_pattern(),
                                injector: Box::new(NoInput),
                                lock: Some(Box::new(move || {
                                    locks.fetch_add(1, Ordering::SeqCst);
                                })),
                            })
                        })
                    }),
                    ..host_desktop(
                        test_pattern(),
                        Box::new(NoInput),
                        None,
                        HostAudio::default(),
                    )
                };
                serve(connection, ticket, &options, desktop).await
            }));
        }
        // The endpoint stays open until both sessions have ended.
        for session in sessions {
            let _ = session.await;
        }
    });

    let first_frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let first = async {
        let endpoint = ClientEndpoint::bind(loopback()).unwrap();
        let connection = endpoint
            .connect(
                address,
                SERVER_NAME,
                HostVerification::PinnedKeys(vec![pin]),
            )
            .await
            .unwrap();
        let options = client_options(ClientCredentials::Token(TOKEN.to_vec()));
        let mut sink = Endless {
            frames: Arc::clone(&first_frames),
        };
        run(
            connection,
            &options,
            &mut sink,
            None,
            None,
            ClientAudio::default(),
        )
        .await
    };
    let second = async {
        while first_frames.load(Ordering::Relaxed) < INPUT_FRAME {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        stream(address, pin, ClientCredentials::Token(TOKEN.to_vec()), None).await
    };
    let (replaced, second) =
        tokio::time::timeout(TEST_TIMEOUT, async { tokio::join!(first, second) })
            .await
            .expect("both sessions finish in time");
    check_frames(&second.unwrap());
    let replaced = replaced.expect_err("the first session is replaced");
    assert!(
        format!("{replaced:?}").contains("error_code: 11"),
        "{replaced:?}"
    );
    tokio::time::timeout(TEST_TIMEOUT, host)
        .await
        .expect("the host's sessions end in time")
        .unwrap();
    assert_eq!(locks.load(Ordering::SeqCst), 1);
}

/// A client that authenticated once reconnects with the resumption token
/// the host gave it, which works only once, and is given a fresh one
/// (Section 4.4.7).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reconnects_with_a_resumption_token() {
    let identity = TlsIdentity::generate_self_signed(vec![SERVER_NAME.to_owned()]).unwrap();
    let pin = identity.key_pin().unwrap();
    let endpoint = HostEndpoint::bind(loopback(), identity).unwrap();
    let address = endpoint.local_address().unwrap();
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_user(
        "ian".to_owned(),
        ScramCredentials::derive("pencil", b"streaming salt".to_vec(), 4096).unwrap(),
    );
    let options = Arc::new(host_options(authenticator));
    let admission = Admission::default();
    let _host = tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            let ticket = admission.admit(incoming.remote_address().ip()).unwrap();
            let options = Arc::clone(&options);
            tokio::spawn(async move {
                let connection = incoming.establish().await?;
                let desktop = host_desktop(
                    test_pattern(),
                    Box::new(NoInput),
                    None,
                    HostAudio::default(),
                );
                serve(connection, ticket, &options, desktop).await
            });
        }
    });

    let resumption = ResumptionSlot::default();
    let password = ClientCredentials::Password {
        username: "ian".to_owned(),
        password: "pencil".to_owned(),
    };
    let first = ClientOptions {
        resumption: resumption.clone(),
        ..client_options(password)
    };
    tokio::time::timeout(TEST_TIMEOUT, stream_with(address, pin, first, None, None))
        .await
        .expect("the first session finishes in time")
        .unwrap();
    let token = resumption
        .take_credentials(Instant::now())
        .expect("the host gave a resumption token");

    let resumed = ClientOptions {
        resumption: resumption.clone(),
        ..client_options(token.clone())
    };
    let collector =
        tokio::time::timeout(TEST_TIMEOUT, stream_with(address, pin, resumed, None, None))
            .await
            .expect("the resumed session finishes in time")
            .unwrap();
    check_frames(&collector);
    assert!(
        resumption.is_valid(Instant::now()),
        "a fresh token was given"
    );

    let reused = tokio::time::timeout(TEST_TIMEOUT, stream(address, pin, token, None))
        .await
        .expect("the attempt finishes in time");
    assert!(matches!(reused, Err(RuntimeError::AuthenticationFailed(_))));
}

/// A client presenting a certificate whose key the host accepts streams
/// with `CLIENT_CERTIFICATE`; one presenting another key is refused
/// (Section 4.4.2).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_certificate_authenticates() {
    let accepted = TlsIdentity::generate_self_signed(vec!["client".to_owned()]).unwrap();
    let other = TlsIdentity::generate_self_signed(vec!["client".to_owned()]).unwrap();
    for (identity, admitted) in [(accepted.clone_key(), true), (other, false)] {
        let mut authenticator = HostAuthenticator::new().unwrap();
        authenticator.add_client_key(accepted.key_pin().unwrap());
        let (address, pin, _host) = start_host(
            authenticator,
            test_pattern(),
            Box::new(NoInput),
            None,
            HostAudio::default(),
        );
        let options = client_options(ClientCredentials::Certificate);
        let result = tokio::time::timeout(
            TEST_TIMEOUT,
            stream_with(address, pin, options, None, Some(identity)),
        )
        .await
        .expect("the session finishes in time");
        if admitted {
            check_frames(&result.unwrap());
        } else {
            assert!(
                matches!(result, Err(RuntimeError::AuthenticationFailed(_))),
                "{:?}",
                result.err()
            );
        }
    }
}
