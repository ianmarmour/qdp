//! The client's media pipeline: receive, reassemble, decode, present and send
//! feedback.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use qdp::input::{DeviceClass, InputSender};
use qdp::message::{
    Av1Profiles, Capability, ClientHello, ControlMessage, CursorImage, Datagram, DatagramPayload,
    DeviceAttach, DeviceDetach, DeviceFeatures, DeviceId, DeviceKind, DeviceOutput,
    DeviceStatusCode, DisplayConfig, DisplayConfigId, FeatureStatus, FeedbackRecord, FragmentFlags,
    GamepadOutput, HidFeatureRequest, HidFeatureResponse, InputClasses, KeyboardInfo,
    KeyboardOutput, KeyframeRequest, MAX_CURSOR_SIZE, MinorVersion, MonitorId, NO_LEVEL_LIMIT,
    StreamHeader, StreamId, VideoCodec, VideoCodecCapability, VideoCodecFlags, VideoFeedback,
    VideoLayer, WarpFlags,
};
use qdp::registry::ErrorCode;
use qdp::session::{ClientEvent, Negotiated};
use qdp::time::{ClockSync, SessionClock, answer_ping};
use qdp::video::VideoReceiver;
use qdp::wire::{SignedVarInt, VarInt};
use qdp_codec::{CodecPreference, DecoderCommand, ProcessDecoder, VideoDecoder, open_decoder};
use qdp_quic::{
    ClientDriver, DriverInput, QdpConnection, StreamLimit, StreamPolicy, TransportError,
};
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::audio::{ClientAudio, ClientAudioStreams};
use crate::auth::ClientAuthExchange;
use crate::client_cursor::{ClientCursor, CursorChange};
use crate::client_display::{DisplayRequests, DisplayTimingReports};
use crate::client_input::{
    ClientDevices, GamepadFeedback, InputQueue, KEYBOARD_ID, MOUSE_ID, PEN_ID, POINTER_ID,
    PassthroughFeedback, TEXT_ID, TOUCHSCREEN_ID, UserInput,
};
use crate::client_passthrough::HidOutputs;
use crate::clipboard::{
    self, ClipboardDirections, ClipboardLink, ClipboardOutput, ClipboardWorker, SharedClipboard,
    Side,
};
use crate::decode_thread::DecodeThread;
use crate::lock_keys::LockSync;
use crate::media::{CursorWarp, FrameInfo, FrameSink, PresentationTiming, SinkControl, WarpKind};
use crate::pairing::PairingPrompt;
use crate::resumption::ResumptionSlot;
use crate::stream_format::StreamFormat;
use crate::{ClientCredentials, RuntimeError, close_on_error};

/// The largest frame the client's AV1 capability advertises.
const MAX_FRAME_DIMENSION: u32 = 8192;

/// The bit depth the client decodes.
const MAX_BIT_DEPTH: u8 = 8;

/// The highest AV1 level the client decodes: its decoders take every level.
const MAX_LEVEL: u8 = NO_LEVEL_LIMIT;

/// The most datagram bytes buffered for streams not yet defined (Section 3.7.3).
const EARLY_BUFFER_LIMIT: usize = 2 << 20;

/// How long buffered datagrams are kept (Section 3.7.3).
const EARLY_BUFFER_AGE: Duration = Duration::from_secs(2);

/// The overhead of one feedback record's type and length fields, bounded.
const RECORD_OVERHEAD: usize = 16;

/// QDP positions are in 1/256 pixel (Section 1.5.6).
const SUBUNITS: f64 = 256.0;

/// How long a gamepad keeps rumbling without its output being repeated
/// (Section 8.8.1).
const RUMBLE_TIMEOUT: Duration = Duration::from_millis(250);

/// The most bytes a `CURSOR_IMAGE` stream carries after its Stream Type: the
/// largest image's pixels and eight variable-length integers.
const MAX_CURSOR_STREAM: usize = (MAX_CURSOR_SIZE * MAX_CURSOR_SIZE) as usize * 4 + 8 * 8;

/// The client's display, as it describes it to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplaySize {
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The refresh rate in millihertz, or 0 if unknown.
    pub refresh_millihertz: u32,
    /// The interface scale in percent: 100 means no scaling.
    pub scale_percent: u32,
}

impl DisplaySize {
    /// The interface scale of a display that does not scale.
    pub const UNSCALED: u32 = 100;
}

/// The configuration of a client.
#[derive(Debug, Clone)]
pub struct ClientOptions {
    /// The product name the client sends in `CLIENT_HELLO`.
    pub client_name: String,
    /// How the client proves its identity.
    pub credentials: ClientCredentials,
    /// The display the session is shown on.
    pub display: DisplaySize,
    /// The number of software decoder threads, or 0 to choose automatically.
    pub decoder_threads: u32,
    /// Whether to decode in hardware or software.
    pub decoder: CodecPreference,
    /// The command that starts a separate decoder process, which isolates
    /// decoding the host's untrusted video from the client (Section 11.5),
    /// or `None` to decode in the client's own process, which is faster.
    pub decoder_process: Option<DecoderCommand>,
    /// The user's input language and keyboard layout, sent so that the host
    /// can match them (Section 8.2.3).
    pub keyboard_info: Option<KeyboardInfo>,
    /// Where the client keeps the resumption token the host gives it, with
    /// which it can reconnect without the user's credentials (Section
    /// 4.4.7).
    pub resumption: ResumptionSlot,
    /// Asks the user for the code the host shows, for credentials that pair
    /// (Section 4.4.9).
    pub pairing_prompt: Option<PairingPrompt>,
}

/// What happened during one session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientSummary {
    /// The frames decoded and presented.
    pub frames_presented: u64,
    /// The frames that failed to decode.
    pub decode_failures: u64,
}

/// The devices every client attaches during configuration, if the session
/// negotiated their input class (Section 8.2.1).
const STANDARD_DEVICES: [(DeviceId, DeviceKind, &str); 6] = [
    (KEYBOARD_ID, DeviceKind::KEYBOARD, "Keyboard"),
    (POINTER_ID, DeviceKind::POINTER, "Pointer"),
    (MOUSE_ID, DeviceKind::MOUSE, "Mouse"),
    (TEXT_ID, DeviceKind::TEXT, "Text"),
    (TOUCHSCREEN_ID, DeviceKind::TOUCHSCREEN, "Touchscreen"),
    (PEN_ID, DeviceKind::PEN, "Pen"),
];

/// Returns the features the client attaches a standard device of `kind`
/// with: its pen reports everything a pen can.
fn standard_features(kind: DeviceKind) -> DeviceFeatures {
    if kind == DeviceKind::PEN {
        [
            DeviceFeatures::PEN_PRESSURE,
            DeviceFeatures::PEN_TILT,
            DeviceFeatures::PEN_TWIST,
            DeviceFeatures::PEN_ERASER,
            DeviceFeatures::PEN_BARREL_BUTTON,
        ]
        .into_iter()
        .fold(DeviceFeatures::default(), DeviceFeatures::union)
    } else {
        DeviceFeatures::default()
    }
}

/// Returns how the input sender treats a standard device's reports.
fn device_class(device_id: DeviceId) -> Option<DeviceClass> {
    let (_, kind, _) = STANDARD_DEVICES
        .iter()
        .find(|(id, _, _)| *id == device_id)?;
    Some(if *kind == DeviceKind::TEXT {
        DeviceClass::Text
    } else {
        DeviceClass::Standard
    })
}

/// Returns the input classes the client can attach: its standard devices and
/// the gamepads the user connects.
fn input_classes() -> InputClasses {
    InputClasses::KEYBOARD
        .union(InputClasses::POINTER)
        .union(InputClasses::MOUSE)
        .union(InputClasses::TEXT)
        .union(InputClasses::TOUCHSCREEN)
        .union(InputClasses::PEN)
        .union(InputClasses::TOUCHPAD)
        .union(InputClasses::GAMEPAD)
        .union(InputClasses::PASSTHROUGH)
}

/// Returns whether a session that negotiated `negotiated` can use devices
/// of `kind` (Section 4.6.5).
fn supports_kind(negotiated: Option<&Negotiated>, kind: DeviceKind) -> bool {
    let class = kind.input_class();
    negotiated
        .zip(class)
        .is_some_and(|(negotiated, class)| negotiated.supports_input(class))
}

/// Returns the client's hello, with clipboard sharing in `clipboard`'s
/// directions if it shares one, and the audio its devices handle.
fn client_hello(
    options: &ClientOptions,
    clipboard: Option<ClipboardDirections>,
    audio: &ClientAudio,
) -> ClientHello {
    let mut hello = ClientHello {
        minor_version: MinorVersion::CURRENT,
        client_name: options.client_name.clone(),
        capabilities: vec![
            Capability::VideoCodec(VideoCodecCapability {
                codec: VideoCodec::AV1,
                profiles: Av1Profiles::MAIN,
                max_bit_depth: MAX_BIT_DEPTH,
                max_width: VarInt::from_u32(MAX_FRAME_DIMENSION),
                max_height: VarInt::from_u32(MAX_FRAME_DIMENSION),
                max_pixel_rate: VarInt::ZERO,
                flags: VideoCodecFlags::default(),
                max_level: MAX_LEVEL,
            }),
            Capability::Input(input_classes()),
        ],
    };
    if let Some(directions) = clipboard {
        hello.capabilities.push(clipboard::capability(directions));
    }
    hello.capabilities.extend(audio.capabilities());
    hello
}

/// One video stream the client receives.
struct ReceivedStream {
    /// Reassembly, decode ordering and feedback.
    receiver: VideoReceiver,
    /// The decoder, on a thread of its own.
    decoder: DecodeThread,
    /// The monitor's size, to which decoded frames are cropped.
    size: (usize, usize),
    /// The format its key frames are checked against.
    format: StreamFormat,
    /// The newest `DISPLAY_CONFIG` that defines the stream: once its frames
    /// are shown, the user's positions use that configuration's layout.
    config_id: DisplayConfigId,
}

/// Datagrams for streams whose `DISPLAY_CONFIG` has not arrived (Section 3.7.3).
#[derive(Default)]
struct EarlyBuffer {
    /// The datagrams and when they arrived, oldest first.
    datagrams: VecDeque<(Instant, Bytes)>,
    /// The total size of the buffered datagrams.
    bytes: usize,
}

impl EarlyBuffer {
    /// Keeps a datagram if it fits, after discarding expired ones.
    fn push(&mut self, datagram: Bytes, now: Instant) {
        while let Some((arrived, oldest)) = self.datagrams.front() {
            if now.saturating_duration_since(*arrived) < EARLY_BUFFER_AGE {
                break;
            }
            self.bytes -= oldest.len();
            self.datagrams.pop_front();
        }
        if self.bytes + datagram.len() <= EARLY_BUFFER_LIMIT {
            self.bytes += datagram.len();
            self.datagrams.push_back((now, datagram));
        }
    }

    /// Returns every buffered datagram, emptying the buffer.
    fn take(&mut self) -> Vec<Bytes> {
        self.bytes = 0;
        self.datagrams
            .drain(..)
            .map(|(_, datagram)| datagram)
            .collect()
    }
}

/// One client session in the active phase.
struct ActiveClient<'a> {
    /// The connection and session.
    driver: ClientDriver,
    /// Where decoded frames go.
    sink: &'a mut dyn FrameSink,
    /// The client's session clock.
    clock: SessionClock,
    /// The estimate of the host's clock.
    sync: ClockSync,
    /// The video streams, by Stream ID.
    streams: HashMap<StreamId, ReceivedStream>,
    /// Datagrams for streams not yet defined.
    early: EarlyBuffer,
    /// The software decoder thread count.
    decoder_threads: u32,
    /// Whether to decode in hardware or software.
    decoder_preference: CodecPreference,
    /// The command that starts a separate decoder process, if decoding is
    /// isolated.
    decoder_process: Option<DecoderCommand>,
    /// The session's statistics.
    summary: ClientSummary,
    /// Sends input reports reliably.
    input: InputSender,
    /// The state of the standard devices.
    devices: ClientDevices,
    /// The user's input, if the client has a user interface.
    input_queue: Option<&'a mut InputQueue>,
    /// Notified when user input arrives or a decoder finishes a frame.
    wake: Arc<Notify>,
    /// The host's cursor images and the one to draw.
    cursor: ClientCursor,
    /// The desktop position of the monitor's top-left corner, in pixels.
    monitor_origin: (f64, f64),
    /// The host monitor's interface scale, in percent, which cursor images'
    /// Scale is made relative to.
    monitor_scale: u32,
    /// Clipboard sharing, if the session negotiated it.
    clipboard: Option<ClipboardLink>,
    /// The audio streams.
    audio: ClientAudioStreams,
    /// The Output Sequence last applied to each gamepad.
    output_sequences: HashMap<DeviceId, u64>,
    /// When each rumbling gamepad's output last arrived.
    rumbling: HashMap<DeviceId, Instant>,
    /// The output reports arriving for passthrough devices.
    hid_outputs: HidOutputs,
    /// Brings the host's lock keys to the client's.
    lock_keys: LockSync,
    /// The display the session is shown on, and the requests describing it.
    display: DisplayRequests,
    /// The reports of the display's refresh timing.
    display_timing: DisplayTimingReports,
    /// The host monitor the client shows, once configured.
    monitor_id: Option<MonitorId>,
    /// Where the host's resumption token is kept.
    resumption: ResumptionSlot,
}

/// Runs the client's side of one session on a new connection, presenting decoded
/// frames to `sink` until it asks to stop or the host disconnects, sending
/// the user's input from `input`, if given, sharing `clipboard`, if given,
/// with the host, and playing and capturing audio through `audio`'s devices.
pub async fn run(
    connection: QdpConnection,
    options: &ClientOptions,
    sink: &mut dyn FrameSink,
    input: Option<&mut InputQueue>,
    clipboard: Option<SharedClipboard>,
    audio: ClientAudio,
) -> Result<ClientSummary, RuntimeError> {
    let quic = connection.quic().clone();
    let result = run_session(connection, options, sink, input, clipboard, audio).await;
    // The resumption token's lifetime starts as the connection ends.
    options.resumption.connection_ended(Instant::now());
    close_on_error(&quic, result)
}

/// Runs the client's side of one session, as [`run`] describes.
async fn run_session(
    connection: QdpConnection,
    options: &ClientOptions,
    sink: &mut dyn FrameSink,
    mut input: Option<&mut InputQueue>,
    clipboard: Option<SharedClipboard>,
    audio: ClientAudio,
) -> Result<ClientSummary, RuntimeError> {
    let binding = connection.channel_binding()?;
    let host_key = connection.peer_key_pin();
    // The session starts from the client's devices, focus and display as
    // they are now, which matters when it resumes an earlier one.
    let mut display_size = options.display;
    if let Some(queue) = input.as_deref_mut() {
        queue.begin_session();
        display_size = queue.display().unwrap_or(display_size);
    }
    // The clipboard is read from the start, so that its content is known
    // when the session becomes active.
    let directions = clipboard.as_ref().map(|shared| shared.directions);
    let worker = clipboard
        .map(|shared| ClipboardWorker::spawn(shared.clipboard))
        .transpose()
        .map_err(RuntimeError::Clipboard)?;
    let hello = client_hello(options, directions, &audio);
    let audio_request = audio.request();
    let mut driver = ClientDriver::client(connection, hello)?;
    let mut auth = ClientAuthExchange::new(options.credentials.clone(), binding, host_key);
    let mut display_config = None;
    let mut early_messages = Vec::new();
    let (display, first_request) = DisplayRequests::start(display_size, Instant::now());
    loop {
        match driver.next_event().await? {
            ClientEvent::HostHello(hello) => {
                info!(host = %hello.host_name, "host hello");
                let (method, auth_data) = auth.start(&hello.auth_methods)?;
                driver.session().start_auth(method, auth_data)?;
            }
            ClientEvent::AuthContinue { auth_data } => {
                let reply = if auth.awaits_pairing_code() {
                    // The host shows the code now; the user reads it there.
                    let prompt = options
                        .pairing_prompt
                        .as_ref()
                        .ok_or(RuntimeError::NoPairingPrompt)?;
                    let code = prompt.ask().await.ok_or(RuntimeError::PairingCancelled)?;
                    auth.continue_pairing(&auth_data, &code)?
                } else {
                    auth.continue_exchange(&auth_data)?
                };
                driver.session().continue_auth(reply)?;
            }
            ClientEvent::AuthResult(result) => {
                // A failure closes the connection with QDP_AUTH_FAILED.
                auth.finish(&result)?;
                if let (Some(host_key), Some(prompt)) =
                    (auth.paired_host(), &options.pairing_prompt)
                {
                    info!("paired with the host");
                    prompt.paired(host_key);
                }
                let session = driver.session();
                session.send(&ControlMessage::DisplayRequest(first_request.clone()))?;
                for (device_id, kind, name) in STANDARD_DEVICES {
                    if !supports_kind(session.negotiated(), kind) {
                        continue;
                    }
                    session.send(&ControlMessage::DeviceAttach(DeviceAttach {
                        device_id,
                        kind,
                        features: standard_features(kind),
                        name: name.to_owned(),
                        touchpad: None,
                        passthrough: None,
                    }))?;
                }
                if let Some(keyboard) = &options.keyboard_info {
                    session.send(&ControlMessage::KeyboardInfo(keyboard.clone()))?;
                }
                if let Some(request) = &audio_request {
                    session.send(&ControlMessage::AudioRequest(*request))?;
                }
                session.ready(Instant::now())?;
            }
            ClientEvent::Message(ControlMessage::DisplayConfig(config)) => {
                display_config = Some(config);
            }
            ClientEvent::Message(message) => early_messages.push(message),
            ClientEvent::SessionStarted => break,
        }
    }
    info!("session active");
    let clipboard = match (worker, driver.session().negotiated()) {
        (Some(worker), Some(negotiated)) => ClipboardLink::new(Side::Client, negotiated, worker),
        _ => None,
    };
    let receive_limit = clipboard.as_ref().and_then(ClipboardLink::receive_limit);
    driver.accept_streams(move |header| host_stream_policy(header, receive_limit));

    let now = Instant::now();
    let clock = SessionClock::new(now);
    let mut active = ActiveClient {
        driver,
        sink,
        clock,
        sync: ClockSync::new(clock, now),
        streams: HashMap::new(),
        early: EarlyBuffer::default(),
        decoder_threads: options.decoder_threads,
        decoder_preference: options.decoder,
        decoder_process: options.decoder_process.clone(),
        summary: ClientSummary::default(),
        input: InputSender::new(clock),
        devices: ClientDevices::default(),
        wake: input
            .as_deref()
            .map_or_else(|| Arc::new(Notify::new()), InputQueue::wake),
        input_queue: input,
        audio: ClientAudioStreams::new(audio),
        output_sequences: HashMap::new(),
        rumbling: HashMap::new(),
        hid_outputs: HidOutputs::default(),
        lock_keys: LockSync::default(),
        cursor: ClientCursor::default(),
        monitor_origin: (0.0, 0.0),
        monitor_scale: DisplaySize::UNSCALED,
        clipboard,
        display,
        display_timing: DisplayTimingReports::default(),
        monitor_id: None,
        resumption: options.resumption.clone(),
    };
    if let Some(config) = display_config {
        active.configure(&config)?;
    }
    if let Some(link) = &mut active.clipboard {
        let outputs = link.start();
        active.apply_clipboard(outputs)?;
    }
    for message in early_messages {
        active.handle_message(message)?;
    }
    active.run().await
}

/// Returns what the client does with a stream the host opened (Section
/// 3.5.2), given the clipboard data the client accepts. Passthrough devices'
/// output streams are followed as their reports arrive.
fn host_stream_policy(header: &StreamHeader, clipboard_limit: Option<usize>) -> StreamPolicy {
    match header {
        StreamHeader::CursorImage => StreamPolicy::Read(StreamLimit {
            max_bytes: MAX_CURSOR_STREAM,
            overflow: ErrorCode::PROTOCOL_VIOLATION,
        }),
        StreamHeader::ClipboardData { .. } => clipboard::data_stream_policy(clipboard_limit),
        StreamHeader::HidOutput { .. } => StreamPolicy::Follow,
        StreamHeader::Unknown(_) => StreamPolicy::Refuse(ErrorCode::STREAM_TYPE_UNKNOWN),
    }
}

/// Shows a cursor change in the sink, the image's Scale made relative to
/// the host monitor's, `monitor_scale` percent, as [`FrameSink::set_cursor`]
/// takes it.
fn show_cursor(sink: &mut dyn FrameSink, change: CursorChange<'_>, monitor_scale: u32) {
    match change {
        CursorChange::Hidden => sink.set_cursor(None),
        CursorChange::Shown(image) => {
            let mut image = image.clone();
            image.scale = relative_scale(image.scale, monitor_scale);
            sink.set_cursor(Some(&image));
        }
    }
}

/// Returns a cursor image's Scale, `image_scale` percent, relative to the
/// host monitor's, `monitor_scale` percent: 100 for an image drawn at the
/// monitor's scale, which is shown at the size it has on the monitor
/// (Section 9.2).
fn relative_scale(image_scale: VarInt, monitor_scale: u32) -> VarInt {
    let relative = u128::from(image_scale.into_inner()) * u128::from(DisplaySize::UNSCALED)
        / u128::from(monitor_scale.max(1));
    u64::try_from(relative)
        .ok()
        .and_then(|relative| VarInt::new(relative).ok())
        .unwrap_or(VarInt::MAX)
}

impl ActiveClient<'_> {
    /// Carries out what clipboard sharing asks for.
    fn apply_clipboard(&mut self, outputs: Vec<ClipboardOutput>) -> Result<(), RuntimeError> {
        for output in outputs {
            match output {
                ClipboardOutput::Message(message) => self.driver.session().send(&message)?,
                ClipboardOutput::Cancel { request_id } => self.driver.cancel_stream(
                    StreamHeader::ClipboardData { request_id },
                    ErrorCode::REQUEST_CANCELLED,
                ),
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

    /// Passes a gamepad's output to its device, if it is newer than the last
    /// applied (Section 8.8.1).
    fn handle_device_output(&mut self, output: &DeviceOutput<'_>, now: Instant) {
        if output.device_id == KEYBOARD_ID {
            self.handle_keyboard_output(output, now);
            return;
        }
        let (Some(slot), Some(queue)) = (
            self.devices.gamepad_slot(output.device_id),
            &self.input_queue,
        ) else {
            return;
        };
        let sequence = output.output_sequence.into_inner();
        let last = self.output_sequences.entry(output.device_id).or_default();
        if sequence <= *last {
            return;
        }
        *last = sequence;
        let Ok(gamepad) = GamepadOutput::decode(output.data) else {
            debug!("malformed gamepad output ignored");
            return;
        };
        if gamepad.is_rumbling() {
            self.rumbling.insert(output.device_id, now);
        } else {
            self.rumbling.remove(&output.device_id);
        }
        queue.send_gamepad_feedback(GamepadFeedback {
            slot,
            output: gamepad,
        });
    }

    /// Takes the host keyboard's LEDs from its output report, if it is newer
    /// than the last applied, bringing the host's lock keys to the client's
    /// if the session gained focus before they were known (Section 8.4.1).
    fn handle_keyboard_output(&mut self, output: &DeviceOutput<'_>, now: Instant) {
        let sequence = output.output_sequence.into_inner();
        let last = self.output_sequences.entry(output.device_id).or_default();
        if sequence <= *last {
            return;
        }
        *last = sequence;
        let Ok(keyboard) = KeyboardOutput::decode(output.data) else {
            debug!("malformed keyboard output ignored");
            return;
        };
        let toggles = self
            .lock_keys
            .host_leds(keyboard.leds, self.input.acknowledged());
        self.toggle_lock_keys(&toggles, now);
    }

    /// Presses and releases the lock keys with these usages on the host.
    fn toggle_lock_keys(&mut self, usages: &[u8], now: Instant) {
        for &usage in usages {
            for pressed in [true, false] {
                for report in self.devices.reports(UserInput::Key { usage, pressed }) {
                    if let Some(sequence) = self.input.push(
                        report.device_id,
                        report.data,
                        report.kind,
                        report.rest,
                        now,
                    ) {
                        self.lock_keys.sent(sequence);
                    }
                }
            }
        }
    }

    /// Stops the rumble of gamepads whose output has not been repeated for
    /// 250 ms while they rumbled (Section 8.8.1).
    fn stop_stale_rumble(&mut self, now: Instant) {
        let stale: Vec<DeviceId> = self
            .rumbling
            .iter()
            .filter(|(_, last)| now.saturating_duration_since(**last) >= RUMBLE_TIMEOUT)
            .map(|(&device_id, _)| device_id)
            .collect();
        for device_id in stale {
            self.rumbling.remove(&device_id);
            if let (Some(slot), Some(queue)) =
                (self.devices.gamepad_slot(device_id), &self.input_queue)
            {
                queue.send_gamepad_feedback(GamepadFeedback {
                    slot,
                    output: GamepadOutput::default(),
                });
            }
        }
    }

    /// Passes the output reports on a passthrough device's stream to the
    /// device (Section 8.8.2).
    fn handle_output_chunk(&mut self, chunk: qdp_quic::StreamChunk) {
        let Some(queue) = &self.input_queue else {
            return;
        };
        for (device_id, report) in self.hid_outputs.chunk(chunk) {
            if let Some(slot) = self.devices.passthrough_slot(device_id) {
                queue.send_passthrough_feedback(PassthroughFeedback::Output { slot, report });
            }
        }
    }

    /// Passes a feature report request to its passthrough device, or fails
    /// it at once for a device the client does not have (Section 8.8.3).
    fn handle_feature_request(&mut self, request: HidFeatureRequest) -> Result<(), RuntimeError> {
        let slot = self.devices.passthrough_slot(request.device_id);
        if let (Some(slot), Some(queue)) = (slot, &self.input_queue) {
            queue.send_passthrough_feedback(PassthroughFeedback::FeatureRequest {
                slot,
                request_id: request.request_id,
                operation: request.operation,
                report: request.report_data,
            });
            return Ok(());
        }
        let failed = HidFeatureResponse {
            request_id: request.request_id,
            status: FeatureStatus::Failed,
            report_data: Vec::new(),
        };
        self.driver
            .session()
            .send(&ControlMessage::HidFeatureResponse(failed))?;
        Ok(())
    }

    /// Acts on a unidirectional stream from the host.
    fn handle_stream(&mut self, stream: qdp_quic::ReceivedStream) -> Result<(), RuntimeError> {
        if let (StreamHeader::ClipboardData { request_id }, Some(link)) =
            (stream.header, &mut self.clipboard)
        {
            let outputs = link.handle_data(request_id, stream.data);
            return self.apply_clipboard(outputs);
        }
        let data = match (stream.header, stream.data) {
            (StreamHeader::CursorImage, Ok(data)) => data,
            (header, Err(failure)) => {
                debug!(?header, ?failure, "stream not received");
                return Ok(());
            }
            (header, Ok(_)) => {
                debug!(?header, "unexpected stream ignored");
                return Ok(());
            }
        };
        match CursorImage::decode(&data) {
            Ok(image) => {
                if let Some(change) = self.cursor.image(image) {
                    show_cursor(self.sink, change, self.monitor_scale);
                }
            }
            Err(error) => warn!(%error, "malformed cursor image ignored"),
        }
        Ok(())
    }

    /// Acts on a control message from the host.
    fn handle_message(&mut self, message: ControlMessage) -> Result<(), RuntimeError> {
        match message {
            ControlMessage::DisplayConfig(config) => self.configure(&config)?,
            ControlMessage::DeviceStatus(status) => {
                if status.status == DeviceStatusCode::Accepted {
                    let gamepad = self
                        .devices
                        .gamepad_slot(status.device_id)
                        .map(|_| DeviceClass::Standard);
                    let touchpad = self
                        .devices
                        .touchpad_slot(status.device_id)
                        .map(|_| DeviceClass::Standard);
                    let passthrough = self
                        .devices
                        .passthrough_slot(status.device_id)
                        .map(|_| DeviceClass::Passthrough);
                    let class = device_class(status.device_id)
                        .or(gamepad)
                        .or(touchpad)
                        .or(passthrough);
                    if let Some(class) = class {
                        self.input.add_device(status.device_id, class);
                    }
                    let now = Instant::now();
                    let held = self
                        .devices
                        .gamepad_state(status.device_id)
                        .into_iter()
                        .chain(self.devices.touchpad_state(status.device_id))
                        .chain(self.devices.passthrough_state(status.device_id));
                    for report in held {
                        self.input.push(
                            report.device_id,
                            report.data,
                            report.kind,
                            report.rest,
                            now,
                        );
                    }
                } else {
                    debug!(
                        device = status.device_id.0.into_inner(),
                        reason = status.reason,
                        "device not accepted"
                    );
                    self.input.remove_device(status.device_id);
                }
            }
            ControlMessage::PointerMode(mode) => {
                let now = Instant::now();
                for report in self.devices.set_pointer_mode(mode.mode) {
                    self.input
                        .push(report.device_id, report.data, report.kind, report.rest, now);
                }
                self.sink.set_pointer_mode(&mode);
            }
            ControlMessage::AudioConfig(config) => {
                let quic = self.driver.connection().quic().clone();
                self.audio.configure(&config, &quic, self.clock)?;
            }
            ControlMessage::CursorSet(set) => {
                if let Some(change) = self.cursor.set(set) {
                    show_cursor(self.sink, change, self.monitor_scale);
                }
            }
            ControlMessage::HidFeatureRequest(request) => self.handle_feature_request(request)?,
            ControlMessage::CursorWarp(warp) => {
                let coordinate = |value: SignedVarInt| value.into_inner() as f64 / SUBUNITS;
                let kind = if warp.flags.contains(WarpFlags::FOLLOW) {
                    WarpKind::Follow
                } else {
                    WarpKind::Move
                };
                self.sink.warp_cursor(CursorWarp {
                    position: (
                        coordinate(warp.x) - self.monitor_origin.0,
                        coordinate(warp.y) - self.monitor_origin.1,
                    ),
                    kind,
                });
                self.input.set_warp(warp.warp_id);
            }
            ControlMessage::ResumptionToken(token) => {
                debug!(lifetime = ?token.lifetime, "resumption token received");
                self.resumption.received(&token);
            }
            message => {
                if let Some(link) = &mut self.clipboard {
                    let outputs = link.handle_message(&message, Instant::now());
                    self.apply_clipboard(outputs)?;
                }
            }
        }
        Ok(())
    }

    /// Replaces the video streams with those of `config` (Section 6.3.1).
    fn configure(&mut self, config: &DisplayConfig) -> Result<(), RuntimeError> {
        self.monitor_id = config.monitors.first().map(|monitor| monitor.id);
        let monitor_scale = config
            .monitors
            .first()
            .map_or(DisplaySize::UNSCALED, |monitor| {
                u32::try_from(monitor.scale.into_inner()).unwrap_or(u32::MAX)
            });
        if monitor_scale != self.monitor_scale {
            // The cursor drawn keeps its size on the monitor at its new
            // scale.
            self.monitor_scale = monitor_scale;
            if let Some(change) = self.cursor.shown() {
                show_cursor(self.sink, change, monitor_scale);
            }
        }
        if let Some(monitor) = config.monitors.first() {
            let coordinate =
                |value: SignedVarInt| i32::try_from(value.into_inner()).unwrap_or_default();
            self.devices
                .set_monitor_origin(coordinate(monitor.x), coordinate(monitor.y));
            self.monitor_origin = (
                f64::from(coordinate(monitor.x)),
                f64::from(coordinate(monitor.y)),
            );
        }
        let mut streams = HashMap::new();
        let mut new_streams = false;
        for stream in &config.streams {
            if stream.layer != VideoLayer::Motion {
                continue;
            }
            if let Some(mut existing) = self.streams.remove(&stream.stream_id) {
                existing.config_id = config.config_id;
                streams.insert(stream.stream_id, existing);
                continue;
            }
            new_streams = true;
            let Some(monitor) = config
                .monitors
                .iter()
                .find(|monitor| monitor.id == stream.monitor_id)
            else {
                continue;
            };
            let decoder: Box<dyn VideoDecoder> = match &self.decoder_process {
                Some(command) => Box::new(ProcessDecoder::spawn(
                    command.clone(),
                    self.decoder_threads,
                    self.decoder_preference,
                )?),
                None => open_decoder(self.decoder_threads, self.decoder_preference)?,
            };
            info!(
                stream = stream.stream_id.0.into_inner(),
                width = monitor.width.into_inner(),
                height = monitor.height.into_inner(),
                decoder = decoder.description(),
                "video stream"
            );
            streams.insert(
                stream.stream_id,
                ReceivedStream {
                    receiver: VideoReceiver::new(
                        stream.stream_id,
                        stream.layer,
                        monitor.block_grid(),
                        self.clock,
                    ),
                    decoder: DecodeThread::spawn(decoder, Arc::clone(&self.wake))?,
                    size: (
                        monitor.width.into_inner() as usize,
                        monitor.height.into_inner() as usize,
                    ),
                    format: StreamFormat::new(*stream, MAX_LEVEL),
                    config_id: config.config_id,
                },
            );
        }
        self.streams = streams;
        // The user points at the picture shown, which changes only once a
        // new stream's first frame is (Section 8.3).
        if !new_streams {
            self.input.set_display_config(config.config_id);
        }
        for datagram in self.early.take() {
            self.handle_datagram(datagram)?;
        }
        Ok(())
    }

    /// Streams until the sink asks to stop or the host disconnects.
    async fn run(&mut self) -> Result<ClientSummary, RuntimeError> {
        loop {
            let smoothed_rtt = self.driver.connection().smoothed_rtt();
            let deadline = self
                .streams
                .values()
                .filter_map(|stream| stream.receiver.poll_timeout(smoothed_rtt))
                .chain(Some(self.sync.next_ping_at()))
                .chain(self.input.poll_timeout(smoothed_rtt))
                .chain(self.display.poll_timeout())
                .chain(self.clipboard.as_ref().map(ClipboardLink::poll_timeout))
                .chain(self.rumbling.values().map(|last| *last + RUMBLE_TIMEOUT))
                .min();
            match self.driver.next_input(deadline, Some(&self.wake)).await {
                Ok(DriverInput::Event(ClientEvent::Message(message))) => {
                    self.handle_message(message)?;
                }
                Ok(DriverInput::Event(_) | DriverInput::Deadline | DriverInput::Wake) => {}
                Ok(DriverInput::Datagram(datagram)) => self.handle_datagram(datagram)?,
                Ok(DriverInput::Stream(stream)) => self.handle_stream(stream)?,
                Ok(DriverInput::StreamChunk(chunk)) => self.handle_output_chunk(chunk),
                Err(error) if crate::is_normal_close(&error) => return Ok(self.summary),
                Err(error) => return Err(error.into()),
            }

            let now = Instant::now();
            for stream in self.streams.values_mut() {
                stream.receiver.handle_timeout(smoothed_rtt, now);
            }
            if self.decode_ready()? == SinkControl::Stop {
                self.send_feedback(Instant::now())?;
                self.driver.close("viewer closed");
                return Ok(self.summary);
            }
            self.send_feedback(now)?;
            if let Some(link) = &mut self.clipboard {
                let outputs = link.service(now);
                self.apply_clipboard(outputs)?;
            }
            if let Some(ping) = self.sync.poll_ping(now) {
                self.send(ping.to_bytes())?;
            }
            self.stop_stale_rumble(now);
            self.send_input(smoothed_rtt, now)?;
            if let Some(request) = self.display.poll_request(now) {
                self.driver
                    .session()
                    .send(&ControlMessage::DisplayRequest(request))?;
            }
        }
    }

    /// Turns the user's waiting input into reports and sends the `INPUT`
    /// datagrams that are due.
    fn send_input(&mut self, smoothed_rtt: Duration, now: Instant) -> Result<(), RuntimeError> {
        let mut clipboard_outputs = Vec::new();
        let mut lock_toggles = Vec::new();
        if let Some(queue) = self.input_queue.as_deref_mut() {
            while let Some(action) = queue.try_next() {
                match action {
                    UserInput::GamepadConnected { .. }
                        if !supports_kind(
                            self.driver.session().negotiated(),
                            DeviceKind::GAMEPAD,
                        ) =>
                    {
                        debug!("the host does not take gamepads");
                    }
                    UserInput::GamepadConnected {
                        slot,
                        features,
                        name,
                    } => {
                        let attach = self.devices.connect_gamepad(slot, features, name);
                        self.driver
                            .session()
                            .send(&ControlMessage::DeviceAttach(attach))?;
                    }
                    UserInput::GamepadDisconnected { slot } => {
                        if let Some(device_id) = self.devices.disconnect_gamepad(slot) {
                            self.input.remove_device(device_id);
                            self.rumbling.remove(&device_id);
                            self.driver
                                .session()
                                .send(&ControlMessage::DeviceDetach(DeviceDetach { device_id }))?;
                        }
                    }
                    UserInput::TouchpadConnected { .. }
                        if !supports_kind(
                            self.driver.session().negotiated(),
                            DeviceKind::TOUCHPAD,
                        ) =>
                    {
                        debug!("the host does not take touchpads");
                    }
                    UserInput::TouchpadConnected {
                        slot,
                        name,
                        surface,
                        clickpad,
                    } => {
                        let attach = self.devices.connect_touchpad(slot, name, surface, clickpad);
                        self.driver
                            .session()
                            .send(&ControlMessage::DeviceAttach(attach))?;
                    }
                    UserInput::TouchpadDisconnected { slot } => {
                        if let Some(device_id) = self.devices.disconnect_touchpad(slot) {
                            self.input.remove_device(device_id);
                            self.driver
                                .session()
                                .send(&ControlMessage::DeviceDetach(DeviceDetach { device_id }))?;
                        }
                    }
                    UserInput::PassthroughConnected { .. }
                        if !supports_kind(
                            self.driver.session().negotiated(),
                            DeviceKind::PASSTHROUGH,
                        ) =>
                    {
                        debug!("the host does not take passthrough devices");
                    }
                    UserInput::PassthroughConnected { slot, name, info } => {
                        let attach = self.devices.connect_passthrough(slot, name, info);
                        self.driver
                            .session()
                            .send(&ControlMessage::DeviceAttach(attach))?;
                    }
                    UserInput::PassthroughDisconnected { slot } => {
                        if let Some(device_id) = self.devices.disconnect_passthrough(slot) {
                            self.input.remove_device(device_id);
                            self.driver
                                .session()
                                .send(&ControlMessage::DeviceDetach(DeviceDetach { device_id }))?;
                        }
                    }
                    UserInput::Display(display) => self.display.display_changed(display),
                    UserInput::Focus(focus) => {
                        if let Some(link) = &mut self.clipboard {
                            clipboard_outputs.extend(link.set_focus(focus));
                        }
                    }
                    UserInput::LockKeys(local) => {
                        lock_toggles.extend(
                            self.lock_keys
                                .focus_gained(local, self.input.acknowledged()),
                        );
                    }
                    UserInput::Microphone(microphone) => {
                        if let Some(request) = self.audio.set_microphone(microphone) {
                            self.driver
                                .session()
                                .send(&ControlMessage::AudioRequest(request))?;
                        }
                    }
                    UserInput::FeatureResponse {
                        request_id,
                        status,
                        report,
                    } => {
                        self.driver
                            .session()
                            .send(&ControlMessage::HidFeatureResponse(HidFeatureResponse {
                                request_id,
                                status,
                                report_data: report,
                            }))?;
                    }
                    action => {
                        for report in self.devices.reports(action) {
                            self.input.push(
                                report.device_id,
                                report.data,
                                report.kind,
                                report.rest,
                                now,
                            );
                        }
                    }
                }
            }
        }
        self.apply_clipboard(clipboard_outputs)?;
        self.toggle_lock_keys(&lock_toggles, now);
        let max_datagram_size = self.driver.connection().max_datagram_size();
        while let Some(datagram) = self
            .input
            .poll_transmit(smoothed_rtt, max_datagram_size, now)
        {
            self.send(datagram)?;
        }
        Ok(())
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

    /// Acts on a datagram from the host.
    fn handle_datagram(&mut self, bytes: Bytes) -> Result<(), RuntimeError> {
        let now = Instant::now();
        let smoothed_rtt = self.driver.connection().smoothed_rtt();
        // A datagram shorter than its fields is a connection error (Section
        // 1.6); unknown types decode as such and are ignored.
        let datagram = Datagram::decode(&bytes).map_err(|error| {
            RuntimeError::ProtocolViolation(format!("malformed datagram: {error}"))
        })?;
        match datagram {
            Datagram::VideoFragment(fragment) => match self.streams.get_mut(&fragment.stream_id) {
                Some(stream) => stream
                    .receiver
                    .handle_fragment(&fragment, smoothed_rtt, now),
                None => self.early.push(bytes.clone(), now),
            },
            Datagram::TimePong(pong) => {
                if let Some(sample) = self.sync.handle_pong(&pong, now) {
                    debug!(
                        offset_micros = sample.offset_micros,
                        delay_micros = sample.delay.as_micros() as u64,
                        "clock sample"
                    );
                }
            }
            Datagram::TimePing(ping) => {
                let pong = answer_ping(&ping, &self.clock, now, Instant::now());
                self.send(pong.to_bytes())?;
            }
            Datagram::InputAck(ack) => self.input.handle_ack(&ack, now),
            Datagram::DeviceOutput(output) => self.handle_device_output(&output, now),
            Datagram::AudioPacket(packet) => self.audio.handle_packet(&packet, now),
            _ => {}
        }
        Ok(())
    }

    /// Presents the frames the decoders finished, and passes each stream's
    /// next decodable frame to its decoder. The next frame goes to the
    /// decoder before the finished ones are presented, so that it decodes
    /// while this thread presents rather than after.
    fn decode_ready(&mut self) -> Result<SinkControl, RuntimeError> {
        let mut keyframe_requests = Vec::new();
        let mut finished = Vec::new();
        for (&stream_id, stream) in &mut self.streams {
            while let Some(decoded) = stream.decoder.try_result()? {
                let frame_number = decoded.frame.frame_number;
                match decoded.picture {
                    Ok(picture) => {
                        // The frame counts as decoded once this loop takes
                        // it, which is when DECODED reports it, rather than
                        // when its decoder finished: a frame that waits for
                        // this loop, busy presenting the one before, waits
                        // just as one queued for its decoder does, and its
                        // FRAME_TIMING must show the host so (Section 6.11).
                        stream.receiver.frame_decoded(frame_number, Instant::now());
                        // A hardware encoder may have padded the frame.
                        let picture = picture.cropped(stream.size.0, stream.size.1);
                        let latency = self
                            .sync
                            .peer_to_local(decoded.frame.header.capture_timestamp)
                            .map(|captured| decoded.decoded_at.saturating_duration_since(captured));
                        let info = FrameInfo {
                            stream_id,
                            frame_number,
                            latency,
                            decoded_at: decoded.decoded_at,
                            decode_time: decoded.decode_time,
                            first_fragment_at: decoded.frame.first_arrival,
                            completed_at: decoded.frame.completed_at,
                        };
                        finished.push((picture, info, stream.config_id));
                    }
                    Err(error) => {
                        warn!(%error, "decoding failed");
                        self.summary.decode_failures += 1;
                        stream.receiver.decode_failed();
                        keyframe_requests.push(stream_id);
                    }
                }
            }
            // The receiver hands out one frame at a time, until the decoder
            // reports on it.
            if let Some(frame) = stream.receiver.poll_decodable() {
                if frame.flags.contains(FragmentFlags::KEY_FRAME) {
                    stream.format.check_key_frame(frame.temporal_unit())?;
                }
                stream.decoder.decode(frame)?;
            }
        }
        for stream_id in keyframe_requests {
            self.driver
                .session()
                .send(&ControlMessage::KeyframeRequest(KeyframeRequest {
                    stream_id,
                }))?;
        }
        for (picture, info, config_id) in finished {
            self.summary.frames_presented += 1;
            let (stream_id, frame_number) = (info.stream_id, info.frame_number);
            let control = self.sink.present(picture, info);
            self.input.set_display_config(config_id);
            if self.sink.presentation_timing() == PresentationTiming::Immediate {
                if let Some(stream) = self.streams.get_mut(&stream_id) {
                    stream
                        .receiver
                        .frame_presented(frame_number, Some(Instant::now()));
                }
            }
            if control == SinkControl::Stop {
                return Ok(SinkControl::Stop);
            }
        }
        Ok(SinkControl::Continue)
    }

    /// Sends the streams' feedback records in as few datagrams as fit.
    /// Every datagram carries the `LOSS` record of each stream in the `LOSS`
    /// or `KEY_ONLY` state, however the other records are split (Section
    /// 6.8.3, rule 3).
    fn send_feedback(&mut self, now: Instant) -> Result<(), RuntimeError> {
        for presentation in self.sink.take_presentations() {
            if let Some(stream) = self.streams.get_mut(&presentation.stream_id) {
                stream
                    .receiver
                    .frame_presented(presentation.frame_number, presentation.presented_at);
            }
        }
        let mut polled: Vec<FeedbackRecord> = self
            .streams
            .values_mut()
            .flat_map(|stream| stream.receiver.poll_feedback(now))
            .collect();
        if let (Some(timing), Some(monitor_id)) = (self.sink.display_timing(), self.monitor_id) {
            polled.extend(
                self.display_timing
                    .poll(timing, monitor_id, &self.clock, now)
                    .map(FeedbackRecord::DisplayTiming),
            );
        }
        if polled.is_empty() {
            return Ok(());
        }
        let losses: Vec<FeedbackRecord> = self
            .streams
            .values()
            .filter_map(|stream| stream.receiver.loss_record())
            .collect();
        let others: Vec<FeedbackRecord> = polled
            .into_iter()
            .filter(|record| !matches!(record, FeedbackRecord::Loss(_)))
            .collect();
        let max_size = self.driver.connection().max_datagram_size();
        let losses_size = 1 + losses.iter().map(record_size).sum::<usize>();
        let start = || VideoFeedback {
            records: losses.clone(),
        };
        let mut batch = start();
        let mut batch_size = losses_size;
        let mut batch_has_others = false;
        for record in others {
            let size = record_size(&record);
            if batch_has_others && batch_size + size > max_size {
                self.send(std::mem::replace(&mut batch, start()).to_bytes())?;
                batch_size = losses_size;
            }
            batch_size += size;
            batch.records.push(record);
            batch_has_others = true;
        }
        self.send(batch.to_bytes())
    }
}

/// Returns an upper bound on the encoded size of a feedback record.
fn record_size(record: &FeedbackRecord) -> usize {
    let mut writer = qdp::wire::Writer::new();
    record.encode(&mut writer);
    writer.len() + RECORD_OVERHEAD
}

#[cfg(test)]
mod tests {
    use qdp::wire::VarInt;

    /// A cursor image drawn at the monitor's scale is shown at its size on
    /// the monitor, and one drawn at another scale keeps its size relative
    /// to the monitor's.
    #[test]
    fn makes_cursor_scales_relative() {
        let relative =
            |image: u32, monitor: u32| super::relative_scale(VarInt::from_u32(image), monitor);
        assert_eq!(relative(300, 300), VarInt::from_u32(100));
        assert_eq!(relative(100, 300), VarInt::from_u32(33));
        assert_eq!(relative(200, 100), VarInt::from_u32(200));
        assert_eq!(relative(150, 0), VarInt::from_u32(15_000));
    }
}
