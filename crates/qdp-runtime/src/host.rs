//! The host's media pipeline: capture, encode, send, and answer feedback.

use std::time::{Duration, Instant};

use qdp::input::{InputEvent, InputReceiver};
use qdp::message::{
    Av1Profiles, BlockGrid, Capability, Chroma, ClientMonitorId, ControlMessage, CursorId,
    CursorSet, Datagram, DatagramPayload, DeviceAttach, DeviceId, DeviceStatus, DeviceStatusCode,
    DisplayConfig, DisplayConfigId, DisplayRequestId, FeedbackRecord, FrameHeader, HostHello,
    HostMonitor, HostMonitorFlags, ImageSerial, InputClasses, KeyboardOutput, MinorVersion,
    MonitorId, NO_LEVEL_LIMIT, PointerKind, PointerMode, StreamHeader, StreamId, VideoCodec,
    VideoCodecCapability, VideoCodecFlags, VideoLayer, VideoStreamConfig, VideoStreamFlags,
};
use qdp::registry::ErrorCode;
use qdp::session::{HostConfig, HostEvent};
use qdp::time::{SessionClock, answer_ping};
use qdp::video::{FrameKind, VideoSender};
use qdp::wire::{SignedVarInt, VarInt};
use qdp_codec::{CodecPreference, EncoderSettings, KeyFrameRequest};
use qdp_quic::{DriverInput, HostDriver, QdpConnection, StreamPolicy, TransportError};
use tracing::{debug, info, warn};

use crate::audio::{HostAudio, HostAudioStreams};
use crate::auth::HostAuthExchange;
use crate::clipboard::{
    self, ClipboardDirections, ClipboardLink, ClipboardOutput, ClipboardWorker, SharedClipboard,
    Side,
};
use crate::encode_thread::{EncodeJob, EncodedOutput, EncoderThread};
use crate::host_cursor::{CursorUpdate, HostCursor};
use crate::host_input::{InputAction, InputInjector, Positions, StandardInput};
use crate::host_output::{DeviceOutputs, OutputReport};
use crate::media::{CursorState, FrameSource};
use crate::send_stats::{SendStats, queue_delay};
use crate::{HostAuthenticator, RuntimeError, is_normal_close};

/// The Stream ID of the motion layer of the host's one monitor.
const MOTION_STREAM_ID: StreamId = StreamId(VarInt::from_u32(1));

/// The Monitor ID of the host's one monitor.
const MONITOR_ID: MonitorId = MonitorId(VarInt::from_u32(1));

/// The H.273 colour primaries of standard dynamic range streams (BT.709).
const SDR_PRIMARIES: u8 = 1;

/// The H.273 transfer characteristics of standard dynamic range streams (sRGB).
const SDR_TRANSFER: u8 = 13;

/// The H.273 matrix coefficients of BT.709 YUV streams.
const BT709_MATRIX: u8 = 1;

/// The bit depth of the streams this host sends.
const BIT_DEPTH: u8 = 8;

/// The AV1 profile of the streams this host sends: profile 0, 4:2:0.
const AV1_PROFILE: u8 = 0;

/// The interface scale the host reports, in percent, for its monitor and the
/// cursor images drawn for it.
pub(crate) const SCALE_PERCENT: u32 = 100;

/// The millihertz in one hertz.
const MILLIHERTZ_PER_HERTZ: u32 = 1000;

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

/// Returns the input classes the host can inject with `injector`.
fn input_classes(injector: &dyn InputInjector) -> InputClasses {
    let mut classes = InputClasses::KEYBOARD
        .union(InputClasses::POINTER)
        .union(InputClasses::MOUSE);
    if injector.supports_text() {
        classes = classes.union(InputClasses::TEXT);
    }
    if injector.supports_touch() {
        classes = classes.union(InputClasses::TOUCHSCREEN);
    }
    if injector.supports_gamepads() {
        classes = classes.union(InputClasses::GAMEPAD);
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

/// Converts a size to a variable-length integer, saturating.
fn varint(value: usize) -> VarInt {
    VarInt::try_from(value).unwrap_or(VarInt::MAX)
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
    /// The Config ID of the last `DISPLAY_CONFIG`.
    display_config_id: u64,
    /// The display configuration, resent unchanged when the client asks again.
    display_config: DisplayConfig,
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
}

/// Runs the host's side of one session on an accepted connection until the
/// client disconnects, streaming frames from `source`, which is captured and
/// encoded on a thread of its own, performing the client's input with
/// `injector`, sharing `clipboard`, if given, with the client, and streaming
/// audio through `audio`'s devices.
pub async fn serve(
    connection: QdpConnection,
    options: &HostOptions,
    source: Box<dyn FrameSource>,
    mut injector: Box<dyn InputInjector>,
    clipboard: Option<SharedClipboard>,
    audio: HostAudio,
) -> Result<HostSummary, RuntimeError> {
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
            capabilities: capabilities(input_classes(&*injector), directions, &audio),
        },
        failure_reason: FAILURE_REASON.to_owned(),
    };
    let mut driver = HostDriver::host(connection, config);
    let mut auth = HostAuthExchange::new(&options.authenticator, binding);
    let mut devices = StandardInput::default();
    let mut request_id = DisplayRequestId::NONE;
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
                driver.session().respond_auth(response, now)?;
            }
            HostEvent::AuthContinue { auth_data } => {
                let response = auth.continue_exchange(&auth_data);
                driver.session().respond_auth(response, now)?;
            }
            HostEvent::Message(ControlMessage::DeviceAttach(attach)) => {
                answer_attach(&mut driver, &attach, &mut devices)?;
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
                request_id = request.request_id;
            }
            HostEvent::Message(ControlMessage::AudioRequest(request)) => {
                audio_request = Some(request);
            }
            HostEvent::Message(ControlMessage::ClientReady(_)) => break,
            HostEvent::Message(_) => {}
        }
    }

    let (width, height) = source.size();
    let display_config = display_config(width, height, options.frame_rate, request_id);
    let clock = SessionClock::new(Instant::now());
    let mut audio = HostAudioStreams::new(audio);
    let quic = driver.connection().quic().clone();
    let session = driver.session();
    session.send(&ControlMessage::DisplayConfig(display_config.clone()))?;
    if let (Some(request), Some(negotiated)) = (audio_request, session.negotiated()) {
        let config = audio.configure(&request, negotiated, &quic, clock)?;
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
    driver.flush().await?;
    let receive_limit = clipboard.as_ref().and_then(ClipboardLink::receive_limit);
    driver.accept_streams(move |header| client_stream_policy(header, receive_limit));
    info!(
        width,
        height,
        input = injector.description(),
        "session active"
    );

    let settings = EncoderSettings {
        width,
        height,
        frame_rate: options.frame_rate,
        speed: options.encoder.speed,
        quantizer: options.encoder.quantizer,
        threads: options.encoder.threads,
    };
    let grid = BlockGrid::for_monitor(width as u32, height as u32);
    let (encoder, description) =
        EncoderThread::spawn(source, settings, options.encoder.preference, grid).await?;
    info!(encoder = description, "encoding");
    let stats = SendStats::new(Instant::now(), driver.connection().path_stats());
    let mut active = ActiveHost {
        driver,
        stats,
        encoder,
        pending_since: None,
        frames_requested: 0,
        sender: VideoSender::new(MOTION_STREAM_ID),
        input: InputReceiver::new(),
        clock,
        devices,
        injector,
        display_config_id: 1,
        display_config,
        summary: HostSummary::default(),
        outputs: DeviceOutputs::default(),
        cursor: HostCursor::default(),
        clipboard,
        audio,
        pointer_mode: PointerKind::Absolute,
        last_input_at: Instant::now(),
    };
    active.update_outputs(Instant::now())?;
    if let Some(link) = &mut active.clipboard {
        let outputs = link.start();
        active.apply_clipboard(outputs)?;
    }
    let result = active
        .run(Duration::from_secs(1) / options.frame_rate.max(1))
        .await;
    // Nothing the client held stays held on the host.
    let actions = active.devices.release_all();
    active.inject(&actions);
    result
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
/// returns whether the device was accepted.
fn answer_attach(
    driver: &mut HostDriver,
    attach: &DeviceAttach,
    devices: &mut StandardInput,
) -> Result<bool, RuntimeError> {
    let negotiated = attach.kind.input_class().is_some_and(|class| {
        driver
            .session()
            .negotiated()
            .is_some_and(|negotiated| negotiated.supports_input(class))
    });
    let accepted = negotiated && devices.attach(attach.device_id, attach.kind);
    let status = if accepted {
        DeviceStatusCode::Accepted
    } else {
        DeviceStatusCode::Unsupported
    };
    driver
        .session()
        .send(&ControlMessage::DeviceStatus(DeviceStatus {
            device_id: attach.device_id,
            status,
            reason: String::new(),
        }))?;
    Ok(accepted)
}

/// Returns the configuration of one monitor of `width × height` with one AV1
/// motion stream.
fn display_config(
    width: usize,
    height: usize,
    frame_rate: u32,
    request_id: DisplayRequestId,
) -> DisplayConfig {
    DisplayConfig {
        config_id: DisplayConfigId(VarInt::from_u32(1)),
        request_id,
        monitors: vec![HostMonitor {
            id: MONITOR_ID,
            client_monitor_id: ClientMonitorId(VarInt::from_u32(1)),
            flags: HostMonitorFlags::PRIMARY,
            x: SignedVarInt::ZERO,
            y: SignedVarInt::ZERO,
            width: varint(width),
            height: varint(height),
            refresh_rate: VarInt::from_u32(frame_rate.saturating_mul(MILLIHERTZ_PER_HERTZ)),
            scale: VarInt::from_u32(SCALE_PERCENT),
        }],
        streams: vec![VideoStreamConfig {
            stream_id: MOTION_STREAM_ID,
            monitor_id: MONITOR_ID,
            layer: VideoLayer::Motion,
            codec: VideoCodec::AV1,
            profile: AV1_PROFILE,
            bit_depth: BIT_DEPTH,
            chroma: Chroma::Yuv420,
            flags: VideoStreamFlags::default(),
            color_primaries: SDR_PRIMARIES,
            transfer_characteristics: SDR_TRANSFER,
            matrix_coefficients: BT709_MATRIX,
        }],
    }
}

impl ActiveHost {
    /// Streams until the client disconnects.
    async fn run(&mut self, frame_interval: Duration) -> Result<HostSummary, RuntimeError> {
        let mut next_frame_at = Instant::now();
        loop {
            let smoothed_rtt = self.driver.connection().smoothed_rtt();
            let deadline = [
                self.pending_since.is_none().then_some(next_frame_at),
                self.sender.poll_timeout(smoothed_rtt),
                self.input.poll_timeout(smoothed_rtt),
                Some(self.stats.next_log_at()),
                self.clipboard.as_ref().map(ClipboardLink::poll_timeout),
                self.outputs.poll_timeout(),
                self.input_silence_deadline(),
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
                self.inject(&actions);
            }
            if let Some(link) = &mut self.clipboard {
                let outputs = link.service(now);
                self.apply_clipboard(outputs)?;
            }
            if let Some(ack) = self.input.poll_ack(now) {
                self.send(ack.to_bytes())?;
            }
            if self.outputs.poll_timeout().is_some_and(|due| due <= now) {
                self.update_outputs(now)?;
            }
            if self.pending_since.is_none() && now >= next_frame_at {
                self.follow_pointer_mode()?;
                self.update_outputs(now)?;
                if self.network_behind(frame_interval) {
                    self.stats.frame_skipped();
                } else {
                    self.request_frame(now)?;
                }
                next_frame_at += frame_interval;
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
            let positions = if event.warp_id < self.cursor.latest_warp() {
                Positions::Ignore
            } else {
                Positions::Apply
            };
            let actions = self.devices.apply(event, positions, self.pointer_mode);
            self.inject(&actions);
        }
    }

    /// Performs input actions, logging the ones that fail.
    fn inject(&mut self, actions: &[InputAction]) {
        let now = Instant::now();
        for action in actions {
            self.cursor.input(action, now);
            if let Err(error) = self.injector.inject(action) {
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
            if let Some(warp) = self.cursor.warp_to_current() {
                session.send(&ControlMessage::CursorWarp(warp))?;
            }
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
        for update in self.cursor.update(state, now) {
            match update {
                CursorUpdate::Image(image) => {
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
                        .send(&ControlMessage::CursorWarp(warp))?;
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
        self.encoder.request(EncodeJob { request, recovery })?;
        self.pending_since = Some(now);
        self.frames_requested += 1;
        Ok(())
    }

    /// Fragments and sends one encoded frame, and the pointer captured with
    /// it.
    fn send_encoded(&mut self, output: EncodedOutput, now: Instant) -> Result<(), RuntimeError> {
        if let Some(cursor) = &output.cursor {
            self.send_cursor(cursor, now)?;
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
        for datagram in sent.datagrams {
            self.send(datagram)?;
        }
        Ok(())
    }

    /// Acts on a control message from the client.
    fn handle_event(&mut self, event: HostEvent) -> Result<(), RuntimeError> {
        match event {
            HostEvent::Message(ControlMessage::KeyframeRequest(request)) => {
                if request.stream_id == MOTION_STREAM_ID {
                    self.sender.handle_keyframe_request();
                }
            }
            HostEvent::Message(ControlMessage::DisplayRequest(request)) => {
                // Resizing is not supported yet: answer with the same streams.
                self.display_config_id += 1;
                self.display_config.config_id = DisplayConfigId(varint_u64(self.display_config_id));
                self.display_config.request_id = request.request_id;
                self.driver
                    .session()
                    .send(&ControlMessage::DisplayConfig(self.display_config.clone()))?;
            }
            HostEvent::Message(ControlMessage::DeviceAttach(attach)) => {
                if answer_attach(&mut self.driver, &attach, &mut self.devices)? {
                    self.update_outputs(Instant::now())?;
                }
            }
            HostEvent::Message(ControlMessage::DeviceDetach(detach)) => {
                let actions = self.devices.detach(detach.device_id);
                self.inject(&actions);
                self.outputs.remove(detach.device_id);
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
                    let config = self
                        .audio
                        .configure(&request, negotiated, &quic, self.clock)?;
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
        let datagram = match Datagram::decode(bytes) {
            Ok(datagram) => datagram,
            Err(error) => {
                warn!(%error, "malformed datagram");
                return Ok(());
            }
        };
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
            FeedbackRecord::Decoded(decoded) if decoded.stream_id == MOTION_STREAM_ID => {
                self.sender.handle_decoded(decoded.last_decoded_frame);
            }
            FeedbackRecord::Nack(nack) if nack.stream_id == MOTION_STREAM_ID => {
                for datagram in self.sender.handle_nack(nack, smoothed_rtt, now) {
                    self.summary.retransmissions += 1;
                    self.send(datagram)?;
                }
            }
            FeedbackRecord::Loss(loss) if loss.stream_id == MOTION_STREAM_ID => {
                self.sender.handle_loss(loss, smoothed_rtt, now);
            }
            _ => {}
        }
        Ok(())
    }
}

/// Converts a counter to a variable-length integer, saturating.
fn varint_u64(value: u64) -> VarInt {
    VarInt::new(value).unwrap_or(VarInt::MAX)
}
