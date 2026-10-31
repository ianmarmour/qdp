//! The client's media pipeline: receive, reassemble, decode, present and send
//! feedback.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use bytes::Bytes;
use qdp::input::{DeviceClass, InputSender};
use qdp::message::{
    Av1Profiles, Capability, ClientHello, ClientMonitor, ClientMonitorFlags, ClientMonitorId,
    ControlMessage, CursorImage, Datagram, DatagramPayload, DeviceAttach, DeviceFeatures, DeviceId,
    DeviceKind, DeviceStatusCode, DisplayConfig, DisplayRequest, DisplayRequestId, FeedbackRecord,
    InputClasses, KeyframeRequest, MAX_CURSOR_SIZE, MinorVersion, NO_LEVEL_LIMIT, StreamHeader,
    StreamId, VideoCodec, VideoCodecCapability, VideoCodecFlags, VideoFeedback, VideoLayer,
    WarpFlags,
};
use qdp::registry::ErrorCode;
use qdp::session::ClientEvent;
use qdp::time::{ClockSync, SessionClock, answer_ping};
use qdp::video::VideoReceiver;
use qdp::wire::{SignedVarInt, VarInt};
use qdp_codec::{CodecPreference, VideoDecoder, open_decoder};
use qdp_quic::{
    ClientDriver, DriverInput, QdpConnection, StreamLimit, StreamPolicy, TransportError,
};
use tracing::{debug, info, warn};

use crate::auth::ClientAuthExchange;
use crate::client_cursor::{ClientCursor, CursorChange};
use crate::client_input::{ClientDevices, InputQueue, KEYBOARD_ID, MOUSE_ID, POINTER_ID, TEXT_ID};
use crate::clipboard::{
    self, ClipboardDirections, ClipboardLink, ClipboardOutput, ClipboardWorker, SharedClipboard,
    Side,
};
use crate::media::{CursorWarp, FrameInfo, FrameSink, SinkControl, WarpKind};
use crate::{ClientCredentials, RuntimeError};

/// The client's identifier for its one display area.
const DISPLAY_AREA_ID: ClientMonitorId = ClientMonitorId(VarInt::from_u32(1));

/// The largest frame the client's AV1 capability advertises.
const MAX_FRAME_DIMENSION: u32 = 8192;

/// The bit depth the client decodes.
const MAX_BIT_DEPTH: u8 = 8;

/// The interface scale the client reports, in percent.
const SCALE_PERCENT: u32 = 100;

/// The most datagram bytes buffered for streams not yet defined (Section 3.7.3).
const EARLY_BUFFER_LIMIT: usize = 2 << 20;

/// How long buffered datagrams are kept (Section 3.7.3).
const EARLY_BUFFER_AGE: Duration = Duration::from_secs(2);

/// The overhead of one feedback record's type and length fields, bounded.
const RECORD_OVERHEAD: usize = 16;

/// QDP positions are in 1/256 pixel (Section 1.5.6).
const SUBUNITS: f64 = 256.0;

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
}

/// What happened during one session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientSummary {
    /// The frames decoded and presented.
    pub frames_presented: u64,
    /// The frames that failed to decode.
    pub decode_failures: u64,
}

/// The devices every client attaches during configuration (Section 8.2.1).
const STANDARD_DEVICES: [(DeviceId, DeviceKind, &str); 4] = [
    (KEYBOARD_ID, DeviceKind::KEYBOARD, "Keyboard"),
    (POINTER_ID, DeviceKind::POINTER, "Pointer"),
    (MOUSE_ID, DeviceKind::MOUSE, "Mouse"),
    (TEXT_ID, DeviceKind::TEXT, "Text"),
];

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

/// Returns the client's hello, with clipboard sharing in `clipboard`'s
/// directions if it shares one.
fn client_hello(options: &ClientOptions, clipboard: Option<ClipboardDirections>) -> ClientHello {
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
                max_level: NO_LEVEL_LIMIT,
            }),
            Capability::Input(
                InputClasses::KEYBOARD
                    .union(InputClasses::POINTER)
                    .union(InputClasses::MOUSE),
            ),
        ],
    };
    if let Some(directions) = clipboard {
        hello.capabilities.push(clipboard::capability(directions));
    }
    hello
}

/// Returns the client's display request.
fn display_request(display: DisplaySize) -> DisplayRequest {
    DisplayRequest {
        request_id: DisplayRequestId(VarInt::from_u32(1)),
        monitors: vec![ClientMonitor {
            id: DISPLAY_AREA_ID,
            flags: ClientMonitorFlags::PRIMARY,
            x: SignedVarInt::ZERO,
            y: SignedVarInt::ZERO,
            width: VarInt::from_u32(display.width),
            height: VarInt::from_u32(display.height),
            refresh_rate: VarInt::from_u32(display.refresh_millihertz),
            scale: VarInt::from_u32(SCALE_PERCENT),
            physical_width: VarInt::ZERO,
            physical_height: VarInt::ZERO,
            peak_luminance: VarInt::ZERO,
        }],
    }
}

/// One video stream the client receives.
struct ReceivedStream {
    /// Reassembly, decode ordering and feedback.
    receiver: VideoReceiver,
    /// The decoder.
    decoder: Box<dyn VideoDecoder>,
    /// The monitor's size, to which decoded frames are cropped.
    size: (usize, usize),
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
    /// The session's statistics.
    summary: ClientSummary,
    /// Sends input reports reliably.
    input: InputSender,
    /// The state of the standard devices.
    devices: ClientDevices,
    /// The user's input, if the client has a user interface.
    input_queue: Option<InputQueue>,
    /// The host's cursor images and the one to draw.
    cursor: ClientCursor,
    /// The desktop position of the monitor's top-left corner, in pixels.
    monitor_origin: (f64, f64),
    /// Clipboard sharing, if the session negotiated it.
    clipboard: Option<ClipboardLink>,
}

/// Runs the client's side of one session on a new connection, presenting decoded
/// frames to `sink` until it asks to stop or the host disconnects, sending
/// the user's input from `input`, if given, and sharing `clipboard`, if
/// given, with the host.
pub async fn run(
    connection: QdpConnection,
    options: &ClientOptions,
    sink: &mut dyn FrameSink,
    input: Option<InputQueue>,
    clipboard: Option<SharedClipboard>,
) -> Result<ClientSummary, RuntimeError> {
    let binding = connection.channel_binding()?;
    // The clipboard is read from the start, so that its content is known
    // when the session becomes active.
    let directions = clipboard.as_ref().map(|shared| shared.directions);
    let worker = clipboard
        .map(|shared| ClipboardWorker::spawn(shared.clipboard))
        .transpose()
        .map_err(RuntimeError::Clipboard)?;
    let hello = client_hello(options, directions);
    let mut driver = ClientDriver::client(connection, hello)?;
    let mut auth = ClientAuthExchange::new(options.credentials.clone(), binding);
    let mut display_config = None;
    let mut early_messages = Vec::new();
    loop {
        match driver.next_event().await? {
            ClientEvent::HostHello(hello) => {
                info!(host = %hello.host_name, "host hello");
                let (method, auth_data) = auth.start(&hello.auth_methods)?;
                driver.session().start_auth(method, auth_data)?;
            }
            ClientEvent::AuthContinue { auth_data } => {
                let reply = auth.continue_exchange(&auth_data)?;
                driver.session().continue_auth(reply)?;
            }
            ClientEvent::AuthResult(result) => {
                if let Err(error) = auth.finish(&result) {
                    driver.close("authentication failed");
                    return Err(error);
                }
                let session = driver.session();
                session.send(&ControlMessage::DisplayRequest(display_request(
                    options.display,
                )))?;
                for (device_id, kind, name) in STANDARD_DEVICES {
                    session.send(&ControlMessage::DeviceAttach(DeviceAttach {
                        device_id,
                        kind,
                        features: DeviceFeatures::default(),
                        name: name.to_owned(),
                        touchpad: None,
                        passthrough: None,
                    }))?;
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
        summary: ClientSummary::default(),
        input: InputSender::new(clock),
        devices: ClientDevices::default(),
        input_queue: input,
        cursor: ClientCursor::default(),
        monitor_origin: (0.0, 0.0),
        clipboard,
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
/// 3.5.2), given the clipboard data the client accepts. The client attaches
/// no passthrough devices yet, so it expects no HID output.
fn host_stream_policy(header: &StreamHeader, clipboard_limit: Option<usize>) -> StreamPolicy {
    match header {
        StreamHeader::CursorImage => StreamPolicy::Read(StreamLimit {
            max_bytes: MAX_CURSOR_STREAM,
            overflow: ErrorCode::PROTOCOL_VIOLATION,
        }),
        StreamHeader::ClipboardData { .. } => clipboard::data_stream_policy(clipboard_limit),
        StreamHeader::HidOutput { .. } => StreamPolicy::Refuse(ErrorCode::REQUEST_CANCELLED),
        StreamHeader::Unknown(_) => StreamPolicy::Refuse(ErrorCode::STREAM_TYPE_UNKNOWN),
    }
}

/// Shows a cursor change in the sink.
fn show_cursor(sink: &mut dyn FrameSink, change: CursorChange<'_>) {
    match change {
        CursorChange::Hidden => sink.set_cursor(None),
        CursorChange::Shown(image) => sink.set_cursor(Some(image)),
    }
}

impl ActiveClient<'_> {
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
                    show_cursor(self.sink, change);
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
                    if let Some(class) = device_class(status.device_id) {
                        self.input.add_device(status.device_id, class);
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
            ControlMessage::PointerMode(mode) => self.devices.set_pointer_mode(mode.mode),
            ControlMessage::CursorSet(set) => {
                if let Some(change) = self.cursor.set(set) {
                    show_cursor(self.sink, change);
                }
            }
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
        self.input.set_display_config(config.config_id);
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
        for stream in &config.streams {
            if stream.layer != VideoLayer::Motion {
                continue;
            }
            if let Some(existing) = self.streams.remove(&stream.stream_id) {
                streams.insert(stream.stream_id, existing);
                continue;
            }
            let Some(monitor) = config
                .monitors
                .iter()
                .find(|monitor| monitor.id == stream.monitor_id)
            else {
                continue;
            };
            let decoder = open_decoder(self.decoder_threads, self.decoder_preference)?;
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
                    decoder,
                    size: (
                        monitor.width.into_inner() as usize,
                        monitor.height.into_inner() as usize,
                    ),
                },
            );
        }
        self.streams = streams;
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
                .chain(self.clipboard.as_ref().map(ClipboardLink::poll_timeout))
                .min();
            let wake = self.input_queue.as_ref().map(InputQueue::wake);
            match self.driver.next_input(deadline, wake).await {
                Ok(DriverInput::Event(ClientEvent::Message(message))) => {
                    self.handle_message(message)?;
                }
                Ok(DriverInput::Event(_) | DriverInput::Deadline | DriverInput::Wake) => {}
                Ok(DriverInput::Datagram(datagram)) => self.handle_datagram(datagram)?,
                Ok(DriverInput::Stream(stream)) => self.handle_stream(stream)?,
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
            self.send_input(smoothed_rtt, now)?;
        }
    }

    /// Turns the user's waiting input into reports and sends the `INPUT`
    /// datagrams that are due.
    fn send_input(&mut self, smoothed_rtt: Duration, now: Instant) -> Result<(), RuntimeError> {
        if let Some(queue) = &self.input_queue {
            while let Some(action) = queue.try_next() {
                for report in self.devices.reports(action) {
                    self.input
                        .push(report.device_id, report.data, report.kind, report.rest, now);
                }
            }
        }
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
        let datagram = match Datagram::decode(&bytes) {
            Ok(datagram) => datagram,
            Err(error) => {
                warn!(%error, "malformed datagram");
                return Ok(());
            }
        };
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
            _ => {}
        }
        Ok(())
    }

    /// Decodes and presents every frame that is ready.
    fn decode_ready(&mut self) -> Result<SinkControl, RuntimeError> {
        let mut keyframe_requests = Vec::new();
        for (&stream_id, stream) in &mut self.streams {
            while let Some(frame) = stream.receiver.poll_decodable() {
                match stream.decoder.decode(frame.temporal_unit()) {
                    Ok(picture) => {
                        // A hardware encoder may have padded the frame.
                        let picture = picture.cropped(stream.size.0, stream.size.1);
                        let decoded_at = Instant::now();
                        stream
                            .receiver
                            .frame_decoded(frame.frame_number, decoded_at);
                        let latency = self
                            .sync
                            .peer_to_local(frame.header.capture_timestamp)
                            .map(|captured| decoded_at.saturating_duration_since(captured));
                        let info = FrameInfo {
                            frame_number: frame.frame_number,
                            latency,
                            decoded_at,
                        };
                        self.summary.frames_presented += 1;
                        let control = self.sink.present(&picture, info);
                        stream
                            .receiver
                            .frame_presented(frame.frame_number, Some(Instant::now()));
                        if control == SinkControl::Stop {
                            return Ok(SinkControl::Stop);
                        }
                    }
                    Err(error) => {
                        warn!(%error, "decoding failed");
                        self.summary.decode_failures += 1;
                        stream.receiver.decode_failed();
                        keyframe_requests.push(stream_id);
                        break;
                    }
                }
            }
        }
        for stream_id in keyframe_requests {
            self.driver
                .session()
                .send(&ControlMessage::KeyframeRequest(KeyframeRequest {
                    stream_id,
                }))?;
        }
        Ok(SinkControl::Continue)
    }

    /// Sends the streams' feedback records in as few datagrams as fit.
    fn send_feedback(&mut self, now: Instant) -> Result<(), RuntimeError> {
        let records: Vec<FeedbackRecord> = self
            .streams
            .values_mut()
            .flat_map(|stream| stream.receiver.poll_feedback(now))
            .collect();
        if records.is_empty() {
            return Ok(());
        }
        let max_size = self.driver.connection().max_datagram_size();
        let mut batch = VideoFeedback::default();
        let mut batch_size = 1;
        for record in records {
            let size = record_size(&record);
            if !batch.records.is_empty() && batch_size + size > max_size {
                self.send(std::mem::take(&mut batch).to_bytes())?;
                batch_size = 1;
            }
            batch_size += size;
            batch.records.push(record);
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
