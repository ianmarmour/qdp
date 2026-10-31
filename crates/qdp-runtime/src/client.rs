//! The client's media pipeline: receive, reassemble, decode, present and send
//! feedback.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use bytes::Bytes;
use qdp::message::{
    Av1Profiles, Capability, ClientHello, ClientMonitor, ClientMonitorFlags, ClientMonitorId,
    ControlMessage, Datagram, DatagramPayload, DeviceAttach, DeviceFeatures, DeviceId, DeviceKind,
    DisplayConfig, DisplayRequest, DisplayRequestId, FeedbackRecord, InputClasses, KeyframeRequest,
    MinorVersion, NO_LEVEL_LIMIT, StreamId, VideoCodec, VideoCodecCapability, VideoCodecFlags,
    VideoFeedback, VideoLayer,
};
use qdp::session::ClientEvent;
use qdp::time::{ClockSync, SessionClock, answer_ping};
use qdp::video::VideoReceiver;
use qdp::wire::{SignedVarInt, VarInt};
use qdp_codec::Av1Decoder;
use qdp_quic::{ClientDriver, DriverInput, QdpConnection, TransportError};
use tracing::{debug, info, warn};

use crate::auth::ClientAuthExchange;
use crate::media::{FrameInfo, FrameSink, SinkControl};
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
    /// The number of decoder threads, or 0 to choose automatically.
    pub decoder_threads: u32,
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
const STANDARD_DEVICES: [(u32, DeviceKind, &str); 3] = [
    (1, DeviceKind::KEYBOARD, "Keyboard"),
    (2, DeviceKind::POINTER, "Pointer"),
    (3, DeviceKind::MOUSE, "Mouse"),
];

/// Returns the client's hello.
fn client_hello(options: &ClientOptions) -> ClientHello {
    ClientHello {
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
    }
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
    decoder: Av1Decoder,
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
    /// The decoder thread count.
    decoder_threads: u32,
    /// The session's statistics.
    summary: ClientSummary,
}

/// Runs the client's side of one session on a new connection, presenting decoded
/// frames to `sink` until it asks to stop or the host disconnects.
pub async fn run(
    connection: QdpConnection,
    options: &ClientOptions,
    sink: &mut dyn FrameSink,
) -> Result<ClientSummary, RuntimeError> {
    let binding = connection.channel_binding()?;
    let mut driver = ClientDriver::client(connection, client_hello(options))?;
    let mut auth = ClientAuthExchange::new(options.credentials.clone(), binding);
    let mut display_config = None;
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
                for (id, kind, name) in STANDARD_DEVICES {
                    session.send(&ControlMessage::DeviceAttach(DeviceAttach {
                        device_id: DeviceId(VarInt::from_u32(id)),
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
            ClientEvent::Message(_) => {}
            ClientEvent::SessionStarted => break,
        }
    }
    info!("session active");

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
        summary: ClientSummary::default(),
    };
    if let Some(config) = display_config {
        active.configure(&config)?;
    }
    active.run().await
}

impl ActiveClient<'_> {
    /// Replaces the video streams with those of `config` (Section 6.3.1).
    fn configure(&mut self, config: &DisplayConfig) -> Result<(), RuntimeError> {
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
            info!(
                stream = stream.stream_id.0.into_inner(),
                width = monitor.width.into_inner(),
                height = monitor.height.into_inner(),
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
                    decoder: Av1Decoder::new(self.decoder_threads)?,
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
                .min();
            match self.driver.next_input(deadline).await {
                Ok(DriverInput::Event(ClientEvent::Message(ControlMessage::DisplayConfig(
                    config,
                )))) => self.configure(&config)?,
                Ok(DriverInput::Event(_) | DriverInput::Deadline) => {}
                Ok(DriverInput::Datagram(datagram)) => self.handle_datagram(datagram)?,
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
            if let Some(ping) = self.sync.poll_ping(now) {
                self.send(ping.to_bytes())?;
            }
        }
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
