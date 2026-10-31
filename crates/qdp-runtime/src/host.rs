//! The host's media pipeline: capture, encode, send, and answer feedback.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qdp::input::{InputEvent, InputReceiver};
use qdp::message::{
    Av1Profiles, Capability, ControlMessage, CursorId, CursorSet, Datagram, DatagramPayload,
    DeviceAttach, DeviceFeatures, DeviceId, DeviceKind, DeviceStatus, DeviceStatusCode,
    FeatureStatus, FeedbackRecord, FrameHeader, HostHello, ImageSerial, InputClasses,
    KeyboardOutput, MinorVersion, NO_LEVEL_LIMIT, PointerKind, PointerMode, StreamHeader,
    VideoCodec, VideoCodecCapability, VideoCodecFlags,
};
use qdp::registry::ErrorCode;
use qdp::session::{AuthResponse, HostConfig, HostEvent};
use qdp::time::{SessionClock, answer_ping};
use qdp::video::{FrameKind, VideoSender};
use qdp::wire::VarInt;
use qdp_codec::{CodecPreference, KeyFrameRequest};
use qdp_quic::{DriverInput, HostDriver, QdpConnection, StreamPolicy, TransportError};
use tracing::{debug, info, warn};

use crate::admission::AdmissionTicket;
use crate::audio::{HostAudio, HostAudioStreams};
use crate::auth::HostAuthExchange;
use crate::clipboard::{
    self, ClipboardDirections, ClipboardLink, ClipboardOutput, ClipboardWorker, SharedClipboard,
    Side,
};
use crate::encode_thread::{EncodeJob, EncodedOutput, EncoderThread};
use crate::host_cursor::{CursorUpdate, HostCursor};
use crate::host_display::{HostDisplay, StreamChange, VideoLimits};
use crate::host_input::{InputAction, InputInjector, Positions, StandardInput};
use crate::host_output::{DeviceOutputs, OutputReport};
use crate::host_passthrough::{
    self, FeatureRequest, PassthroughDevice, PassthroughLink, PassthroughPolicy,
};
use crate::layout::VideoLayout;
use crate::media::{CursorState, FrameSource};
use crate::rate_control::{QueueDetector, RateControl};
use crate::send_stats::{SendStats, queue_delay};
use crate::stream_ids::StreamIds;
use crate::{HostAuthenticator, RuntimeError, close_on_error, is_normal_close};

/// The bit depth of the streams this host sends.
pub(crate) const BIT_DEPTH: u8 = 8;

/// The interface scale the host reports, in percent, for its monitor and the
/// cursor images drawn for it.
pub(crate) const SCALE_PERCENT: u32 = 100;

/// The largest frame the host's AV1 capability advertises.
const MAX_FRAME_DIMENSION: u32 = 8192;

/// How long the host waits for an `INPUT` datagram while the client holds
/// something before releasing it (Section 8.7, rule 8).
const INPUT_SILENCE: Duration = Duration::from_secs(1);

/// The Reason sent with every failed authentication attempt.
const FAILURE_REASON: &str = "authentication failed";

/// How the host encodes video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderTuning {
    /// Whether to encode in hardware or software.
    pub preference: CodecPreference,
    /// rav1e's speed preset, from 0 to 10.
    pub speed: u8,
    /// The quantizer, from 0 (best) to 255.
    pub quantizer: u8,
    /// The number of encoder threads, or 0 to choose automatically.
    pub threads: usize,
}

/// The configuration of a host.
#[derive(Debug, Clone)]
pub struct HostOptions {
    /// The name the host sends in `HOST_HELLO`.
    pub host_name: String,
    /// Which clients the host accepts.
    pub authenticator: HostAuthenticator,
    /// The frames per second the host captures.
    pub frame_rate: u32,
    /// How the host encodes video.
    pub encoder: EncoderTuning,
    /// Which passthrough devices the host accepts.
    pub passthrough: PassthroughPolicy,
}

/// What happened during one session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostSummary {
    /// The frames sent.
    pub frames_sent: u64,
    /// The key frames among them.
    pub key_frames_sent: u64,
    /// The fragments sent again after a `NACK` or a missing `DECODED`.
    pub retransmissions: u64,
    /// The input reports received and applied.
    pub input_reports: u64,
}

/// Returns the input classes the host can inject with `injector`, accepting
/// passthrough devices unless `passthrough` is off.
fn input_classes(injector: &dyn InputInjector, passthrough: PassthroughPolicy) -> InputClasses {
    let mut classes = InputClasses::KEYBOARD
        .union(InputClasses::POINTER)
        .union(InputClasses::MOUSE);
    if injector.supports_text() {
        classes = classes.union(InputClasses::TEXT);
    }
    if injector.supports_touch() {
        classes = classes.union(InputClasses::TOUCHSCREEN);
    }
    if injector.supports_pen() {
        classes = classes.union(InputClasses::PEN);
    }
    if injector.supports_touchpad() {
        classes = classes.union(InputClasses::TOUCHPAD);
    }
    if injector.supports_gamepads() {
        classes = classes.union(InputClasses::GAMEPAD);
    }
    if injector.supports_passthrough() && passthrough != PassthroughPolicy::Off {
        classes = classes.union(InputClasses::PASSTHROUGH);
    }
    classes
}

/// Returns the capabilities the host advertises: the input `classes` it
/// injects, clipboard sharing in `clipboard`'s directions if it shares one,
/// and the audio its devices handle.
fn capabilities(
    classes: InputClasses,
    clipboard: Option<ClipboardDirections>,
    audio: &HostAudio,
) -> Vec<Capability> {
    let mut capabilities = vec![
        Capability::VideoCodec(VideoCodecCapability {
            codec: VideoCodec::AV1,
            profiles: Av1Profiles::MAIN,
            max_bit_depth: BIT_DEPTH,
            max_width: VarInt::from_u32(MAX_FRAME_DIMENSION),
            max_height: VarInt::from_u32(MAX_FRAME_DIMENSION),
            max_pixel_rate: VarInt::ZERO,
            flags: VideoCodecFlags::default(),
            max_level: NO_LEVEL_LIMIT,
        }),
        Capability::Input(classes),
    ];
    if let Some(directions) = clipboard {
        capabilities.push(clipboard::capability(directions));
    }
    capabilities.extend(audio.capabilities());
    capabilities
}

/// One host session in the active phase.
struct ActiveHost {
    /// The connection and session.
    driver: HostDriver,
    /// Captures and encodes frames on its own thread.
    encoder: EncoderThread,
    /// When the frame the encoder is working on was requested, if it is
    /// working on one.
    pending_since: Option<Instant>,
    /// The number of frames requested from the encoder so far.
    frames_requested: u64,
    /// The video stream's sender.
    sender: VideoSender,
    /// The input reports' receiver.
    input: InputReceiver,
    /// The host's session clock.
    clock: SessionClock,
    /// The devices the host accepted, and their last applied state.
    devices: StandardInput,
    /// Performs the client's input on the host's desktop.
    injector: Box<dyn InputInjector>,
    /// The display configuration and the motion stream it defines.
    display: HostDisplay,
    /// Hands out the Stream IDs of new video and audio streams.
    stream_ids: StreamIds,
    /// How the host encodes, whatever the stream's size.
    tuning: EncoderTuning,
    /// The quantizer, chosen from how frames cross the path.
    rate: RateControl,
    /// Finds queues on the path from the frames' arrival times.
    queue: QueueDetector,
    /// The session's statistics.
    summary: HostSummary,
    /// The statistics logged every second.
    stats: SendStats,
    /// The output reports sent for the devices.
    outputs: DeviceOutputs,
    /// The cursor the client draws.
    cursor: HostCursor,
    /// Clipboard sharing, if the session negotiated it.
    clipboard: Option<ClipboardLink>,
    /// The audio streams.
    audio: HostAudioStreams,
    /// The pointer mode the client was last told.
    pointer_mode: PointerKind,
    /// When the last `INPUT` datagram arrived.
    last_input_at: Instant,
    /// Which passthrough devices the host accepts.
    passthrough_policy: PassthroughPolicy,
    /// The passthrough devices' output streams and feature requests.
    passthrough: PassthroughLink,
}

/// Runs the host's side of one session on an accepted connection until the
/// client disconnects, streaming frames from `source`, which is captured and
/// encoded on a thread of its own, performing the client's input with
/// `injector`, sharing `clipboard`, if given, with the client, and streaming
/// audio through `audio`'s devices. `ticket` is the connection's admission,
/// which records its authentication attempts and, once it authenticates,
/// makes its session the desktop's one.
pub async fn serve(
    connection: QdpConnection,
    ticket: AdmissionTicket,
    options: &HostOptions,
    source: Box<dyn FrameSource>,
    injector: Box<dyn InputInjector>,
    clipboard: Option<SharedClipboard>,
    audio: HostAudio,
) -> Result<HostSummary, RuntimeError> {
    let quic = connection.quic().clone();
    let result = serve_session(
        connection, ticket, options, source, injector, clipboard, audio,
    )
    .await;
    close_on_error(&quic, result)
}

/// Runs the host's side of one session, as [`serve`] describes.
async fn serve_session(
    connection: QdpConnection,
    mut ticket: AdmissionTicket,
    options: &HostOptions,
    source: Box<dyn FrameSource>,
    mut injector: Box<dyn InputInjector>,
    clipboard: Option<SharedClipboard>,
    audio: HostAudio,
) -> Result<HostSummary, RuntimeError> {
    if options.authenticator.methods().is_empty() {
        return Err(RuntimeError::Config(
            "the host accepts no authentication method",
        ));
    }
    let binding = connection.channel_binding()?;
    // The clipboard is read from the start, so that its content is known
    // when the session becomes active.
    let directions = clipboard.as_ref().map(|shared| shared.directions);
    let worker = clipboard
        .map(|shared| ClipboardWorker::spawn(shared.clipboard))
        .transpose()
        .map_err(RuntimeError::Clipboard)?;
    let config = HostConfig {
        host_hello: HostHello {
            minor_version: MinorVersion::CURRENT,
            host_name: options.host_name.clone(),
            auth_methods: options.authenticator.methods(),
            capabilities: capabilities(
                input_classes(&*injector, options.passthrough),
                directions,
                &audio,
            ),
        },
        failure_reason: FAILURE_REASON.to_owned(),
    };
    let mut driver = HostDriver::host(connection, config);
    // Streams are read from the start, so that one the client opens before
    // SESSION_START is a connection error (Section 3.5.2); once the session
    // is active, the rules hold the clipboard data the host accepts.
    let stream_rules: Arc<Mutex<StreamRules>> = Arc::default();
    let rules = Arc::clone(&stream_rules);
    driver.accept_streams(move |header| {
        match *rules
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
        {
            StreamRules::BeforeSession => StreamPolicy::Forbid(ErrorCode::STREAM_CREATION_ERROR),
            StreamRules::Active { clipboard_limit } => {
                client_stream_policy(header, clipboard_limit)
            }
        }
    });
    let mut auth = HostAuthExchange::new(&options.authenticator, binding);
    let mut devices = StandardInput::default();
    let mut display_request = None;
    let mut audio_request = None;

    loop {
        let event = match driver.next_event().await {
            Ok(event) => event,
            Err(error) if is_normal_close(&error) => return Ok(HostSummary::default()),
            Err(error) => return Err(error.into()),
        };
        let now = Instant::now();
        match event {
            HostEvent::ClientHello(hello) => info!(client = %hello.client_name, "client hello"),
            HostEvent::AuthStart { method, auth_data } => {
                let response = auth.start(method, &auth_data);
                record_attempt(&mut ticket, &response, driver.connection());
                driver.session().respond_auth(response, now)?;
            }
            HostEvent::AuthContinue { auth_data } => {
                let response = auth.continue_exchange(&auth_data);
                record_attempt(&mut ticket, &response, driver.connection());
                driver.session().respond_auth(response, now)?;
            }
            HostEvent::Message(ControlMessage::DeviceAttach(attach)) => {
                answer_attach(
                    &mut driver,
                    &attach,
                    &mut devices,
                    &mut *injector,
                    options.passthrough,
                )?;
            }
            HostEvent::Message(ControlMessage::KeyboardInfo(keyboard)) => {
                info!(
                    language = keyboard.language,
                    layout = keyboard.layout,
                    "client keyboard"
                );
                injector.set_keyboard_layout(&keyboard);
            }
            HostEvent::Message(ControlMessage::DisplayRequest(request)) => {
                display_request = Some(request);
            }
            HostEvent::Message(ControlMessage::AudioRequest(request)) => {
                audio_request = Some(request);
            }
            HostEvent::Message(ControlMessage::ClientReady(_)) => break,
            HostEvent::Message(_) => {}
        }
    }

    let limits = VideoLimits {
        frame_rate: options.frame_rate,
        client: driver
            .session()
            .negotiated()
            .map(|negotiated| negotiated.client_video),
    };
    let mut stream_ids = StreamIds::default();
    let display = HostDisplay::new(source.size(), display_request, limits, &mut stream_ids)?;
    let clock = SessionClock::new(Instant::now());
    let mut audio = HostAudioStreams::new(audio);
    let quic = driver.connection().quic().clone();
    let session = driver.session();
    session.send(&ControlMessage::DisplayConfig(display.config().clone()))?;
    if let (Some(request), Some(negotiated)) = (audio_request, session.negotiated()) {
        let config = audio.configure(&request, negotiated, &quic, clock, &mut stream_ids)?;
        session.send(&ControlMessage::AudioConfig(config))?;
    }
    session.send(&ControlMessage::PointerMode(PointerMode {
        mode: PointerKind::Absolute,
        confinement: None,
    }))?;
    session.send(&ControlMessage::CursorSet(CursorSet {
        cursor_id: CursorId::HIDDEN,
        image_serial: ImageSerial::default(),
    }))?;
    session.start()?;
    let clipboard = match (worker, session.negotiated()) {
        (Some(worker), Some(negotiated)) => ClipboardLink::new(Side::Host, negotiated, worker),
        _ => None,
    };
    *stream_rules
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = StreamRules::Active {
        clipboard_limit: clipboard.as_ref().and_then(ClipboardLink::receive_limit),
    };
    audio.activate();
    driver.flush().await?;
    // Datagrams the client sent before SESSION_START are dropped (Section
    // 3.7.3).
    let early = driver.connection().quic().clone();
    while let Ok(Ok(_)) = tokio::time::timeout(Duration::ZERO, early.read_datagram()).await {}
    let plan = *display.plan();
    info!(
        capture_width = plan.layout.capture.0,
        capture_height = plan.layout.capture.1,
        width = plan.layout.video.0,
        height = plan.layout.video.1,
        refresh_millihertz = plan.refresh_millihertz,
        input = injector.description(),
        "session active"
    );

    let settings = plan.encoder_settings(&options.encoder);
    let (encoder, description) =
        EncoderThread::spawn(source, settings, options.encoder.preference).await?;
    info!(encoder = description, "encoding");
    let stats = SendStats::new(Instant::now(), driver.connection().path_stats());
    let mut active = ActiveHost {
        driver,
        stats,
        encoder,
        pending_since: None,
        frames_requested: 0,
        sender: VideoSender::new(display.stream_id()),
        input: InputReceiver::new(),
        clock,
        devices,
        injector,
        display,
        stream_ids,
        tuning: options.encoder,
        rate: RateControl::new(options.encoder.quantizer),
        queue: QueueDetector::default(),
        summary: HostSummary::default(),
        outputs: DeviceOutputs::default(),
        cursor: HostCursor::default(),
        clipboard,
        audio,
        pointer_mode: PointerKind::Absolute,
        last_input_at: Instant::now(),
        passthrough_policy: options.passthrough,
        passthrough: PassthroughLink::default(),
    };
    active.update_outputs(Instant::now())?;
    if let Some(link) = &mut active.clipboard {
        let outputs = link.start();
        active.apply_clipboard(outputs)?;
    }
    let result = active.run().await;
    // Nothing the client held stays held on the host.
    let actions = active.devices.release_all();
    active.inject(&actions, active.display.layout());
    result
}

/// Records an authentication attempt's outcome in the connection's
/// admission.
fn record_attempt(
    ticket: &mut AdmissionTicket,
    response: &AuthResponse,
    connection: &QdpConnection,
) {
    match response {
        AuthResponse::Accept(_) => ticket.authenticated(connection),
        AuthResponse::Reject => ticket.failed(),
        AuthResponse::Continue(_) => {}
    }
}

/// Which streams the host takes from the client.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum StreamRules {
    /// None: the session is not active yet.
    #[default]
    BeforeSession,
    /// Those of an active session, with the clipboard data the host accepts.
    Active {
        /// The most clipboard data the host accepts, if it accepts any.
        clipboard_limit: Option<usize>,
    },
}

/// Returns what the host does with a stream the client opened (Section
/// 3.5.2), given the clipboard data the host accepts.
fn client_stream_policy(header: &StreamHeader, clipboard_limit: Option<usize>) -> StreamPolicy {
    match header {
        StreamHeader::CursorImage | StreamHeader::HidOutput { .. } => {
            StreamPolicy::Forbid(ErrorCode::STREAM_CREATION_ERROR)
        }
        StreamHeader::ClipboardData { .. } => clipboard::data_stream_policy(clipboard_limit),
        StreamHeader::Unknown(_) => StreamPolicy::Refuse(ErrorCode::STREAM_TYPE_UNKNOWN),
    }
}

/// Answers a `DEVICE_ATTACH`, accepting the standard devices this host
/// supports whose input class the session negotiated (Section 4.6.5), and
/// the passthrough devices `policy` allows, which `injector` presents
/// (Section 8.2.5). Returns whether the device was accepted.
fn answer_attach(
    driver: &mut HostDriver,
    attach: &DeviceAttach,
    devices: &mut StandardInput,
    injector: &mut dyn InputInjector,
    policy: PassthroughPolicy,
) -> Result<bool, RuntimeError> {
    let negotiated = attach.kind.input_class().is_some_and(|class| {
        driver
            .session()
            .negotiated()
            .is_some_and(|negotiated| negotiated.supports_input(class))
    });
    let outcome = match (&attach.passthrough, negotiated) {
        (_, false) => Err((DeviceStatusCode::Unsupported, String::new())),
        (Some(info), true) if attach.kind == DeviceKind::PASSTHROUGH => {
            host_passthrough::check(policy, info).and_then(|()| {
                let device = PassthroughDevice {
                    device_id: attach.device_id,
                    name: attach.name.clone(),
                    info: info.clone(),
                };
                injector
                    .attach_passthrough(&device)
                    .map_err(|error| (DeviceStatusCode::Failed, error))?;
                devices.attach_passthrough(attach.device_id, &info.rest_reports);
                Ok(())
            })
        }
        (_, true) if attach.kind == DeviceKind::TOUCHPAD => match attach.touchpad {
            Some(surface) => {
                let clickpad = attach.features.contains(DeviceFeatures::TOUCHPAD_CLICKPAD);
                devices.attach_touchpad(attach.device_id, surface, clickpad);
                Ok(())
            }
            None => Err((
                DeviceStatusCode::Failed,
                "the touchpad's size is missing".to_owned(),
            )),
        },
        (_, true) => {
            if devices.attach(attach.device_id, attach.kind) {
                Ok(())
            } else {
                Err((DeviceStatusCode::Unsupported, String::new()))
            }
        }
    };
    let (status, reason) = match &outcome {
        Ok(()) => (DeviceStatusCode::Accepted, String::new()),
        Err((status, reason)) => {
            debug!(
                device = attach.device_id.0.into_inner(),
                reason, "device refused"
            );
            (*status, reason.clone())
        }
    };
    driver
        .session()
        .send(&ControlMessage::DeviceStatus(DeviceStatus {
            device_id: attach.device_id,
            status,
            reason,
        }))?;
    Ok(outcome.is_ok())
}

impl ActiveHost {
    /// Streams until the client disconnects.
    async fn run(&mut self) -> Result<HostSummary, RuntimeError> {
        let mut next_frame_at = Instant::now();
        loop {
            let frame_interval = self.display.plan().frame_interval();
            let smoothed_rtt = self.driver.connection().smoothed_rtt();
            let deadline = [
                self.pending_since.is_none().then_some(next_frame_at),
                self.sender.poll_timeout(smoothed_rtt),
                self.input.poll_timeout(smoothed_rtt),
                Some(self.stats.next_log_at()),
                self.clipboard.as_ref().map(ClipboardLink::poll_timeout),
                self.outputs.poll_timeout(),
                self.input_silence_deadline(),
                self.passthrough.poll_timeout(),
            ]
            .into_iter()
            .flatten()
            .min();
            let input = self
                .driver
                .next_input(deadline, Some(self.encoder.ready()))
                .await;
            match input {
                Ok(DriverInput::Event(event)) => self.handle_event(event)?,
                Ok(DriverInput::Datagram(datagram)) => self.handle_datagram(&datagram)?,
                Ok(DriverInput::Stream(stream)) => self.handle_stream(stream)?,
                // No stream is followed yet.
                Ok(DriverInput::StreamChunk(_)) => {}
                Ok(DriverInput::Deadline | DriverInput::Wake) => {}
                Err(error) if is_normal_close(&error) => return Ok(self.summary),
                Err(error) => return Err(error.into()),
            }

            let now = Instant::now();
            while let Some(result) = self.encoder.try_result() {
                self.pending_since = None;
                self.send_encoded(result?, now)?;
            }
            for datagram in self.sender.handle_timeout(smoothed_rtt, now) {
                self.summary.retransmissions += 1;
                self.stats.retransmitted();
                self.send(datagram)?;
            }
            let events = self.input.handle_timeout(smoothed_rtt, now);
            self.apply_input(&events);
            if self
                .input_silence_deadline()
                .is_some_and(|deadline| now >= deadline)
            {
                warn!("no input for a second while keys or buttons were held; releasing them");
                let actions = self.devices.release_all();
                self.inject(&actions, self.display.layout());
            }
            if let Some(link) = &mut self.clipboard {
                let outputs = link.service(now);
                self.apply_clipboard(outputs)?;
            }
            if let Some(ack) = self.input.poll_ack(now) {
                self.send(ack.to_bytes())?;
            }
            if self.outputs.poll_timeout().is_some_and(|due| due <= now)
                || self
                    .passthrough
                    .poll_timeout()
                    .is_some_and(|due| due <= now)
            {
                self.update_outputs(now)?;
            }
            // A recovery frame goes out as soon as possible: between frame
            // times, and even when the network is behind (Section 6.8.4).
            let recovery = self.sender.recovery_needed().is_some();
            if self.pending_since.is_none() && (now >= next_frame_at || recovery) {
                self.follow_pointer_mode()?;
                self.update_outputs(now)?;
                if recovery || !self.network_behind(frame_interval) {
                    self.request_frame(now)?;
                } else {
                    self.stats.frame_skipped();
                    self.rate.frame_skipped(now);
                }
                next_frame_at = if now >= next_frame_at {
                    next_frame_at + frame_interval
                } else {
                    now + frame_interval
                };
                if next_frame_at < now {
                    // Encoding fell behind: skip ahead rather than send a burst.
                    next_frame_at = now + frame_interval;
                }
            }
            let connection = self.driver.connection();
            self.stats.log_if_due(
                now,
                connection.path_stats(),
                connection.queued_datagram_bytes(),
                self.pending_since
                    .map(|since| now.saturating_duration_since(since)),
                self.rate.quantizer(),
            );
        }
    }

    /// Returns when the host releases what the client holds if no `INPUT`
    /// datagram arrives first, or `None` while nothing is held.
    fn input_silence_deadline(&self) -> Option<Instant> {
        self.devices
            .is_holding()
            .then_some(self.last_input_at + INPUT_SILENCE)
    }

    /// Applies input reports through the injector. Positions the client
    /// computed before the latest warp are ignored (Section 8.7).
    fn apply_input(&mut self, events: &[InputEvent]) {
        self.summary.input_reports += events.len() as u64;
        for event in events {
            // Positions computed with a layout the host no longer keeps are
            // ignored (Section 8.7, rule 6).
            let layout = self.display.layout_for(event.display_config_id);
            let positions = if event.warp_id < self.cursor.latest_warp() || layout.is_none() {
                Positions::Ignore
            } else {
                Positions::Apply
            };
            let actions = self.devices.apply(event, positions, self.pointer_mode);
            self.inject(&actions, layout.unwrap_or(self.display.layout()));
        }
    }

    /// Performs input actions, whose positions are in the host desktop of
    /// `layout`, logging the ones that fail.
    fn inject(&mut self, actions: &[InputAction], layout: VideoLayout) {
        let now = Instant::now();
        for action in actions {
            let action = layout.action_in_capture(action.clone());
            self.cursor.input(&action, now);
            if let Err(error) = self.injector.inject(&action) {
                warn!(?action, error, "input not applied");
            }
        }
    }

    /// Tells the client when the host's applications capture or release the
    /// pointer (Section 8.9). On returning to absolute mode the host warps the
    /// client's cursor to where the pointer is.
    fn follow_pointer_mode(&mut self) -> Result<(), RuntimeError> {
        let mode = self.injector.pointer_mode();
        if mode == self.pointer_mode {
            return Ok(());
        }
        self.pointer_mode = mode;
        info!(?mode, "pointer mode");
        let session = self.driver.session();
        session.send(&ControlMessage::PointerMode(PointerMode {
            mode,
            confinement: None,
        }))?;
        if mode == PointerKind::Absolute {
            let warp = self
                .display
                .layout()
                .warp_in_video(self.cursor.warp_to_current());
            session.send(&ControlMessage::CursorWarp(warp))?;
        }
        Ok(())
    }

    /// Carries out what clipboard sharing asks for.
    fn apply_clipboard(&mut self, outputs: Vec<ClipboardOutput>) -> Result<(), RuntimeError> {
        for output in outputs {
            match output {
                ClipboardOutput::Message(message) => self.driver.session().send(&message)?,
                ClipboardOutput::Data { request_id, data } => {
                    let header = StreamHeader::ClipboardData { request_id }.to_bytes();
                    self.driver
                        .connection()
                        .send_stream([header, data].concat());
                }
                ClipboardOutput::Publish(text) => {
                    if let Some(link) = &self.clipboard {
                        link.publish(text);
                    }
                }
            }
        }
        Ok(())
    }

    /// Acts on a stream from the client.
    fn handle_stream(&mut self, stream: qdp_quic::ReceivedStream) -> Result<(), RuntimeError> {
        let (StreamHeader::ClipboardData { request_id }, Some(link)) =
            (stream.header, &mut self.clipboard)
        else {
            return Ok(());
        };
        let outputs = link.handle_data(request_id, stream.data);
        self.apply_clipboard(outputs)
    }

    /// Tells the client about the pointer a capture saw: new images on
    /// streams of their own, and `CURSOR_SET` and `CURSOR_WARP` messages.
    fn send_cursor(&mut self, state: &CursorState, now: Instant) -> Result<(), RuntimeError> {
        let layout = self.display.layout();
        for update in self.cursor.update(state, now) {
            match update {
                CursorUpdate::Image(mut image) => {
                    image.scale = layout.cursor_scale(image.scale);
                    let stream = [StreamHeader::CursorImage.to_bytes(), image.to_bytes()].concat();
                    self.driver.connection().send_stream(stream);
                }
                CursorUpdate::Set(set) => {
                    self.driver
                        .session()
                        .send(&ControlMessage::CursorSet(set))?;
                }
                CursorUpdate::Warp(warp) => {
                    self.driver
                        .session()
                        .send(&ControlMessage::CursorWarp(layout.warp_in_video(warp)))?;
                }
            }
        }
        Ok(())
    }

    /// Sends the output reports that changed, the host's keyboard LEDs
    /// (Section 8.4.1) and its applications' gamepad rumble and LEDs
    /// (Section 8.4.7), and repeats those that are due (Section 8.8.1).
    fn update_outputs(&mut self, now: Instant) -> Result<(), RuntimeError> {
        let leds = OutputReport::Keyboard(KeyboardOutput {
            leds: self.injector.keyboard_leds(),
        });
        let mut changes: Vec<(DeviceId, OutputReport)> = self
            .devices
            .keyboards()
            .into_iter()
            .map(|keyboard| (keyboard, leds))
            .collect();
        changes.extend(
            self.injector
                .gamepad_outputs()
                .into_iter()
                .filter(|(device_id, _)| self.devices.contains(*device_id))
                .map(|(device_id, output)| (device_id, OutputReport::Gamepad(output))),
        );
        let mut datagrams: Vec<Vec<u8>> = changes
            .into_iter()
            .filter_map(|(device_id, report)| self.outputs.update(device_id, report, now))
            .collect();
        datagrams.extend(self.outputs.repeats(now));
        for datagram in datagrams {
            self.send(datagram)?;
        }
        self.update_passthrough(now)
    }

    /// Sends the output reports the host's applications sent passthrough
    /// devices, relays their feature report requests, and fails those the
    /// client has not answered in time (Sections 8.8.2 and 8.8.3).
    fn update_passthrough(&mut self, now: Instant) -> Result<(), RuntimeError> {
        let outputs: Vec<(DeviceId, Vec<u8>)> = self
            .injector
            .passthrough_outputs()
            .into_iter()
            .filter(|(device_id, _)| self.devices.contains(*device_id))
            .collect();
        self.passthrough
            .send_outputs(self.driver.connection(), outputs);
        let (requests, unknown): (Vec<FeatureRequest>, Vec<FeatureRequest>) = self
            .injector
            .feature_requests()
            .into_iter()
            .partition(|request| self.devices.contains(request.device_id));
        for request in unknown {
            self.injector
                .answer_feature(request.token, FeatureStatus::Failed, &[]);
        }
        for request in self.passthrough.requests(requests, now) {
            self.driver
                .session()
                .send(&ControlMessage::HidFeatureRequest(request))?;
        }
        for token in self.passthrough.expired(now) {
            self.injector
                .answer_feature(token, FeatureStatus::Failed, &[]);
        }
        Ok(())
    }

    /// Returns whether the datagrams waiting to be sent would take longer than
    /// a frame interval to leave, so that the next frame should be skipped
    /// rather than wait behind them.
    fn network_behind(&self, frame_interval: Duration) -> bool {
        let connection = self.driver.connection();
        queue_delay(connection.queued_datagram_bytes(), &connection.path_stats()) > frame_interval
    }

    /// Sends a datagram, dropping it if it no longer fits the path.
    fn send(&self, datagram: Vec<u8>) -> Result<(), RuntimeError> {
        match self.driver.connection().send_datagram(datagram) {
            Ok(()) => Ok(()),
            Err(TransportError::SendDatagram(quinn_error)) => {
                debug!(%quinn_error, "datagram dropped");
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Asks the encoder thread for the next frame: a key frame for the first one
    /// and whenever the client needs to recover.
    fn request_frame(&mut self, now: Instant) -> Result<(), RuntimeError> {
        let recovery = self.sender.recovery_needed().is_some();
        let request = if self.frames_requested == 0 || recovery {
            KeyFrameRequest::Force
        } else {
            KeyFrameRequest::Auto
        };
        self.encoder.request(EncodeJob {
            request,
            recovery,
            stream_id: self.display.stream_id(),
            settings: self.display.plan().encoder_settings(&self.tuning),
            quantizer: self.rate.quantizer(),
        })?;
        self.pending_since = Some(now);
        self.frames_requested += 1;
        Ok(())
    }

    /// Sends the display's new configuration, starting the stream that
    /// replaces the motion stream if `change` says one does: its first frame
    /// is a key frame, and the cursor images are sent again with the new
    /// layout's Scale.
    fn reconfigure(&mut self, change: StreamChange) -> Result<(), RuntimeError> {
        if change == StreamChange::Replaced {
            self.sender = VideoSender::new(self.display.stream_id());
            self.frames_requested = 0;
            self.cursor.forget_images();
        }
        self.driver.session().send(&ControlMessage::DisplayConfig(
            self.display.config().clone(),
        ))?;
        Ok(())
    }

    /// Fragments and sends one encoded frame, and the pointer captured with
    /// it.
    fn send_encoded(&mut self, output: EncodedOutput, now: Instant) -> Result<(), RuntimeError> {
        if let Some(cursor) = &output.cursor {
            self.send_cursor(cursor, now)?;
        }
        if output.job.stream_id != self.display.stream_id() {
            // Encoded for a stream a new configuration replaced.
            return Ok(());
        }
        if output.capture_size != self.display.layout().capture {
            // The monitor changed size: its frames need a new configuration,
            // and this one is dropped.
            let change = self
                .display
                .capture_resized(output.capture_size, &mut self.stream_ids)?;
            return self.reconfigure(change);
        }
        let kind = match (output.frame.is_key_frame, output.job.recovery) {
            (true, true) => FrameKind::KeyRecovery,
            (true, false) => FrameKind::Key,
            (false, _) => FrameKind::Delta,
        };
        let header = FrameHeader {
            capture_timestamp: self.clock.timestamp(output.captured_at),
            recovery_base: None,
            base_frame_number: None,
            blocks: output.blocks,
        };
        let max_datagram_size = self.driver.connection().max_datagram_size();
        let sent = self.sender.send_frame(
            header,
            &output.frame.temporal_unit,
            kind,
            max_datagram_size,
            now,
        )?;
        self.summary.frames_sent += 1;
        if output.frame.is_key_frame {
            self.summary.key_frames_sent += 1;
        }
        self.stats.frame_sent(
            output.frame.temporal_unit.len(),
            output.frame.is_key_frame,
            now.saturating_duration_since(output.captured_at),
        );
        self.rate.frame_sent(
            output.frame.temporal_unit.len(),
            output.frame.is_key_frame,
            &self.driver.connection().path_stats(),
            self.display.plan().frame_interval(),
            now,
        );
        for datagram in sent.datagrams {
            self.send(datagram)?;
        }
        if !output.frame.is_key_frame {
            self.queue.frame_sent(
                self.display.stream_id(),
                sent.frame_number,
                self.clock.timestamp(Instant::now()),
            );
        }
        Ok(())
    }

    /// Acts on a control message from the client.
    fn handle_event(&mut self, event: HostEvent) -> Result<(), RuntimeError> {
        match event {
            HostEvent::Message(ControlMessage::KeyframeRequest(request)) => {
                if request.stream_id == self.display.stream_id() {
                    self.sender.handle_keyframe_request();
                }
            }
            HostEvent::Message(ControlMessage::DisplayRequest(request)) => {
                let change = self.display.handle_request(request, &mut self.stream_ids)?;
                self.reconfigure(change)?;
            }
            HostEvent::Message(ControlMessage::DeviceAttach(attach)) => {
                if answer_attach(
                    &mut self.driver,
                    &attach,
                    &mut self.devices,
                    &mut *self.injector,
                    self.passthrough_policy,
                )? {
                    self.update_outputs(Instant::now())?;
                }
            }
            HostEvent::Message(ControlMessage::DeviceDetach(detach)) => {
                let actions = self.devices.detach(detach.device_id);
                self.inject(&actions, self.display.layout());
                self.outputs.remove(detach.device_id);
                for token in self.passthrough.remove(detach.device_id) {
                    self.injector
                        .answer_feature(token, FeatureStatus::Failed, &[]);
                }
            }
            HostEvent::Message(ControlMessage::HidFeatureResponse(response)) => {
                if let Some((token, status, report)) = self.passthrough.response(response) {
                    self.injector.answer_feature(token, status, &report);
                }
            }
            HostEvent::Message(ControlMessage::KeyboardInfo(keyboard)) => {
                info!(
                    language = keyboard.language,
                    layout = keyboard.layout,
                    "client keyboard"
                );
                self.injector.set_keyboard_layout(&keyboard);
            }
            HostEvent::Message(ControlMessage::AudioRequest(request)) => {
                // The host answers every request (Section 7.3).
                let quic = self.driver.connection().quic().clone();
                let session = self.driver.session();
                if let Some(negotiated) = session.negotiated() {
                    let config = self.audio.configure(
                        &request,
                        negotiated,
                        &quic,
                        self.clock,
                        &mut self.stream_ids,
                    )?;
                    session.send(&ControlMessage::AudioConfig(config))?;
                }
            }
            HostEvent::Message(message) => {
                if let Some(link) = &mut self.clipboard {
                    let outputs = link.handle_message(&message, Instant::now());
                    self.apply_clipboard(outputs)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Acts on a datagram from the client.
    fn handle_datagram(&mut self, bytes: &[u8]) -> Result<(), RuntimeError> {
        // A datagram shorter than its fields is a connection error (Section
        // 1.6); unknown types decode as such and are ignored.
        let datagram = Datagram::decode(bytes).map_err(|error| {
            RuntimeError::ProtocolViolation(format!("malformed datagram: {error}"))
        })?;
        let now = Instant::now();
        let smoothed_rtt = self.driver.connection().smoothed_rtt();
        match datagram {
            Datagram::VideoFeedback(feedback) => {
                for record in &feedback.records {
                    self.handle_feedback(record, smoothed_rtt, now)?;
                }
            }
            Datagram::TimePing(ping) => {
                let pong = answer_ping(&ping, &self.clock, now, Instant::now());
                self.send(pong.to_bytes())?;
            }
            Datagram::AudioPacket(packet) => self.audio.handle_packet(&packet, now),
            Datagram::Input(input) => {
                self.last_input_at = now;
                let devices = &self.devices;
                let events = self.input.handle_input(
                    &input,
                    |device| devices.contains(device),
                    smoothed_rtt,
                    now,
                );
                self.apply_input(&events);
            }
            _ => {}
        }
        Ok(())
    }

    /// Acts on one feedback record.
    fn handle_feedback(
        &mut self,
        record: &FeedbackRecord,
        smoothed_rtt: Duration,
        now: Instant,
    ) -> Result<(), RuntimeError> {
        match record {
            FeedbackRecord::Decoded(decoded) if decoded.stream_id == self.display.stream_id() => {
                self.sender.handle_decoded(decoded.last_decoded_frame);
            }
            FeedbackRecord::Nack(nack) if nack.stream_id == self.display.stream_id() => {
                for datagram in self.sender.handle_nack(nack, smoothed_rtt, now) {
                    self.summary.retransmissions += 1;
                    self.send(datagram)?;
                }
            }
            FeedbackRecord::Loss(loss) if loss.stream_id == self.display.stream_id() => {
                self.sender.handle_loss(loss, smoothed_rtt, now);
            }
            FeedbackRecord::FrameTiming(timing) => {
                if let Some(delay) = self.queue.frame_timing(timing, now) {
                    self.rate.queue_measured(delay, now);
                }
            }
            _ => {}
        }
        Ok(())
    }
}
