//! The host's media pipeline: capture, encode, send, and answer feedback.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use qdp::input::InputReceiver;
use qdp::message::{
    AudioConfig, AudioConfigId, Av1Profiles, BlockGrid, BlockSet, Capability, Chroma,
    ClientMonitorId, ControlMessage, CursorId, CursorSet, Datagram, DatagramPayload, DeviceAttach,
    DeviceId, DeviceKind, DeviceStatus, DeviceStatusCode, DisplayConfig, DisplayConfigId,
    DisplayRequestId, FeedbackRecord, FrameHeader, HostHello, HostMonitor, HostMonitorFlags,
    ImageSerial, InputClasses, MinorVersion, MonitorId, NO_LEVEL_LIMIT, PointerKind, PointerMode,
    StreamId, VideoCodec, VideoCodecCapability, VideoCodecFlags, VideoLayer, VideoStreamConfig,
    VideoStreamFlags,
};
use qdp::session::{HostConfig, HostEvent};
use qdp::time::{SessionClock, answer_ping};
use qdp::video::{FrameKind, VideoSender};
use qdp::wire::{SignedVarInt, VarInt};
use qdp_codec::{Av1Encoder, EncoderSettings, KeyFrameRequest, Yuv420Frame};
use qdp_quic::{DriverInput, HostDriver, QdpConnection, TransportError};
use tracing::{debug, info, warn};

use crate::auth::HostAuthExchange;
use crate::media::{FrameSource, changed_blocks};
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

/// The interface scale the host reports, in percent.
const SCALE_PERCENT: u32 = 100;

/// The millihertz in one hertz.
const MILLIHERTZ_PER_HERTZ: u32 = 1000;

/// The largest frame the host's AV1 capability advertises.
const MAX_FRAME_DIMENSION: u32 = 8192;

/// The Reason sent with every failed authentication attempt.
const FAILURE_REASON: &str = "authentication failed";

/// How the host encodes video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderTuning {
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

/// Returns the capabilities the host advertises.
fn capabilities() -> Vec<Capability> {
    vec![
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
        Capability::Input(
            InputClasses::KEYBOARD
                .union(InputClasses::POINTER)
                .union(InputClasses::MOUSE),
        ),
    ]
}

/// Returns whether the host accepts a device of `kind`.
fn accepts(kind: DeviceKind) -> bool {
    [DeviceKind::KEYBOARD, DeviceKind::POINTER, DeviceKind::MOUSE].contains(&kind)
}

/// Converts a size to a variable-length integer, saturating.
fn varint(value: usize) -> VarInt {
    VarInt::try_from(value).unwrap_or(VarInt::MAX)
}

/// One host session in the active phase.
struct ActiveHost<'a> {
    /// The connection and session.
    driver: HostDriver,
    /// The capture source.
    source: &'a mut dyn FrameSource,
    /// The encoder.
    encoder: Av1Encoder,
    /// The video stream's sender.
    sender: VideoSender,
    /// The input reports' receiver.
    input: InputReceiver,
    /// The host's session clock.
    clock: SessionClock,
    /// The monitor's composition grid.
    grid: BlockGrid,
    /// The previous frame, for finding changed blocks.
    previous: Option<Yuv420Frame>,
    /// The devices the host accepted.
    devices: BTreeSet<DeviceId>,
    /// The Config ID of the last `DISPLAY_CONFIG`.
    display_config_id: u64,
    /// The display configuration, resent unchanged when the client asks again.
    display_config: DisplayConfig,
    /// The session's statistics.
    summary: HostSummary,
}

/// Runs the host's side of one session on an accepted connection until the
/// client disconnects, streaming frames from `source`.
pub async fn serve(
    connection: QdpConnection,
    options: &HostOptions,
    source: &mut dyn FrameSource,
) -> Result<HostSummary, RuntimeError> {
    let binding = connection.channel_binding()?;
    let config = HostConfig {
        host_hello: HostHello {
            minor_version: MinorVersion::CURRENT,
            host_name: options.host_name.clone(),
            auth_methods: options.authenticator.methods(),
            capabilities: capabilities(),
        },
        failure_reason: FAILURE_REASON.to_owned(),
    };
    let mut driver = HostDriver::host(connection, config);
    let mut auth = HostAuthExchange::new(&options.authenticator, binding);
    let mut devices = BTreeSet::new();
    let mut request_id = DisplayRequestId::NONE;
    let mut audio_requested = false;

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
            HostEvent::Message(ControlMessage::DisplayRequest(request)) => {
                request_id = request.request_id;
            }
            HostEvent::Message(ControlMessage::AudioRequest(_)) => audio_requested = true,
            HostEvent::Message(ControlMessage::ClientReady(_)) => break,
            HostEvent::Message(_) => {}
        }
    }

    let (width, height) = source.size();
    let display_config = display_config(width, height, options.frame_rate, request_id);
    let session = driver.session();
    session.send(&ControlMessage::DisplayConfig(display_config.clone()))?;
    if audio_requested {
        // Audio is not implemented yet: the configuration defines no streams.
        session.send(&ControlMessage::AudioConfig(AudioConfig {
            config_id: AudioConfigId(VarInt::from_u32(1)),
            streams: Vec::new(),
        }))?;
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
    driver.flush().await?;
    info!(width, height, "session active");

    let encoder = Av1Encoder::new(EncoderSettings {
        width,
        height,
        speed: options.encoder.speed,
        quantizer: options.encoder.quantizer,
        threads: options.encoder.threads,
    })?;
    let mut active = ActiveHost {
        driver,
        source,
        encoder,
        sender: VideoSender::new(MOTION_STREAM_ID),
        input: InputReceiver::new(),
        clock: SessionClock::new(Instant::now()),
        grid: BlockGrid::for_monitor(width as u32, height as u32),
        previous: None,
        devices,
        display_config_id: 1,
        display_config,
        summary: HostSummary::default(),
    };
    active
        .run(Duration::from_secs(1) / options.frame_rate.max(1))
        .await
}

/// Answers a `DEVICE_ATTACH`, accepting the standard devices this host supports.
fn answer_attach(
    driver: &mut HostDriver,
    attach: &DeviceAttach,
    devices: &mut BTreeSet<DeviceId>,
) -> Result<(), RuntimeError> {
    let status = if accepts(attach.kind) {
        devices.insert(attach.device_id);
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
    Ok(())
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

impl ActiveHost<'_> {
    /// Streams until the client disconnects.
    async fn run(&mut self, frame_interval: Duration) -> Result<HostSummary, RuntimeError> {
        let mut next_frame_at = Instant::now();
        loop {
            let smoothed_rtt = self.driver.connection().smoothed_rtt();
            let deadline = [
                Some(next_frame_at),
                self.sender.poll_timeout(smoothed_rtt),
                self.input.poll_timeout(smoothed_rtt),
            ]
            .into_iter()
            .flatten()
            .min();
            match self.driver.next_input(deadline).await {
                Ok(DriverInput::Event(event)) => self.handle_event(event)?,
                Ok(DriverInput::Datagram(datagram)) => self.handle_datagram(&datagram)?,
                Ok(DriverInput::Deadline) => {}
                Err(error) if is_normal_close(&error) => return Ok(self.summary),
                Err(error) => return Err(error.into()),
            }

            let now = Instant::now();
            for datagram in self.sender.handle_timeout(smoothed_rtt, now) {
                self.summary.retransmissions += 1;
                self.send(datagram)?;
            }
            let events = self.input.handle_timeout(smoothed_rtt, now);
            self.summary.input_reports += events.len() as u64;
            if let Some(ack) = self.input.poll_ack(now) {
                self.send(ack.to_bytes())?;
            }
            if now >= next_frame_at {
                self.send_frame(now)?;
                next_frame_at += frame_interval;
                if next_frame_at < now {
                    // Encoding fell behind: skip ahead rather than send a burst.
                    next_frame_at = now + frame_interval;
                }
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

    /// Captures, encodes and sends one frame.
    fn send_frame(&mut self, now: Instant) -> Result<(), RuntimeError> {
        let frame = self.source.capture()?;
        let recovery = self.sender.recovery_needed();
        let request = if self.previous.is_none() || recovery.is_some() {
            KeyFrameRequest::Force
        } else {
            KeyFrameRequest::Auto
        };
        for encoded in self.encoder.encode(&frame, request)? {
            let kind = match (encoded.is_key_frame, recovery.is_some()) {
                (true, true) => FrameKind::KeyRecovery,
                (true, false) => FrameKind::Key,
                (false, _) => FrameKind::Delta,
            };
            let blocks = match (&self.previous, encoded.is_key_frame) {
                (Some(previous), false) => changed_blocks(previous, &frame, self.grid),
                _ => BlockSet::full(self.grid.len()),
            };
            let header = FrameHeader {
                capture_timestamp: self.clock.timestamp(now),
                recovery_base: None,
                base_frame_number: None,
                blocks,
            };
            let max_datagram_size = self.driver.connection().max_datagram_size();
            let sent = self.sender.send_frame(
                header,
                &encoded.temporal_unit,
                kind,
                max_datagram_size,
                now,
            )?;
            self.summary.frames_sent += 1;
            if encoded.is_key_frame {
                self.summary.key_frames_sent += 1;
            }
            for datagram in sent.datagrams {
                self.send(datagram)?;
            }
        }
        self.previous = Some(frame);
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
                answer_attach(&mut self.driver, &attach, &mut self.devices)?;
            }
            HostEvent::Message(ControlMessage::DeviceDetach(detach)) => {
                self.devices.remove(&detach.device_id);
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
            Datagram::Input(input) => {
                let devices = &self.devices;
                let events = self.input.handle_input(
                    &input,
                    |device| devices.contains(&device),
                    smoothed_rtt,
                    now,
                );
                self.summary.input_reports += events.len() as u64;
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
