//! The host's media pipeline: capture, encode, send, and answer feedback.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use qdp::input::InputReceiver;
use qdp::message::{
    AudioConfig, AudioConfigId, Av1Profiles, BlockGrid, Capability, Chroma, ClientMonitorId,
    ControlMessage, CursorId, CursorSet, Datagram, DatagramPayload, DeviceAttach, DeviceId,
    DeviceKind, DeviceStatus, DeviceStatusCode, DisplayConfig, DisplayConfigId, DisplayRequestId,
    FeedbackRecord, FrameHeader, HostHello, HostMonitor, HostMonitorFlags, ImageSerial,
    InputClasses, MinorVersion, MonitorId, NO_LEVEL_LIMIT, PointerKind, PointerMode, StreamId,
    VideoCodec, VideoCodecCapability, VideoCodecFlags, VideoLayer, VideoStreamConfig,
    VideoStreamFlags,
};
use qdp::session::{HostConfig, HostEvent};
use qdp::time::{SessionClock, answer_ping};
use qdp::video::{FrameKind, VideoSender};
use qdp::wire::{SignedVarInt, VarInt};
use qdp_codec::{EncoderPreference, EncoderSettings, KeyFrameRequest};
use qdp_quic::{DriverInput, HostDriver, QdpConnection, TransportError};
use tracing::{debug, info, warn};

use crate::auth::HostAuthExchange;
use crate::encode_thread::{EncodeJob, EncodedOutput, EncoderThread};
use crate::media::FrameSource;
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
    /// Whether to encode in hardware or software.
    pub preference: EncoderPreference,
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
struct ActiveHost {
    /// The connection and session.
    driver: HostDriver,
    /// Captures and encodes frames on its own thread.
    encoder: EncoderThread,
    /// Whether a frame has been requested from the encoder and not yet returned.
    encoding: bool,
    /// The number of frames requested from the encoder so far.
    frames_requested: u64,
    /// The video stream's sender.
    sender: VideoSender,
    /// The input reports' receiver.
    input: InputReceiver,
    /// The host's session clock.
    clock: SessionClock,
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
/// client disconnects, streaming frames from `source`, which is captured and
/// encoded on a thread of its own.
pub async fn serve(
    connection: QdpConnection,
    options: &HostOptions,
    source: Box<dyn FrameSource>,
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
    let mut active = ActiveHost {
        driver,
        encoder,
        encoding: false,
        frames_requested: 0,
        sender: VideoSender::new(MOTION_STREAM_ID),
        input: InputReceiver::new(),
        clock: SessionClock::new(Instant::now()),
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

impl ActiveHost {
    /// Streams until the client disconnects.
    async fn run(&mut self, frame_interval: Duration) -> Result<HostSummary, RuntimeError> {
        let mut next_frame_at = Instant::now();
        loop {
            let smoothed_rtt = self.driver.connection().smoothed_rtt();
            let deadline = [
                (!self.encoding).then_some(next_frame_at),
                self.sender.poll_timeout(smoothed_rtt),
                self.input.poll_timeout(smoothed_rtt),
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
                Ok(DriverInput::Deadline | DriverInput::Wake) => {}
                Err(error) if is_normal_close(&error) => return Ok(self.summary),
                Err(error) => return Err(error.into()),
            }

            let now = Instant::now();
            while let Some(result) = self.encoder.try_result() {
                self.encoding = false;
                self.send_encoded(result?, now)?;
            }
            for datagram in self.sender.handle_timeout(smoothed_rtt, now) {
                self.summary.retransmissions += 1;
                self.send(datagram)?;
            }
            let events = self.input.handle_timeout(smoothed_rtt, now);
            self.summary.input_reports += events.len() as u64;
            if let Some(ack) = self.input.poll_ack(now) {
                self.send(ack.to_bytes())?;
            }
            if !self.encoding && now >= next_frame_at {
                self.request_frame()?;
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

    /// Asks the encoder thread for the next frame: a key frame for the first one
    /// and whenever the client needs to recover.
    fn request_frame(&mut self) -> Result<(), RuntimeError> {
        let recovery = self.sender.recovery_needed().is_some();
        let request = if self.frames_requested == 0 || recovery {
            KeyFrameRequest::Force
        } else {
            KeyFrameRequest::Auto
        };
        self.encoder.request(EncodeJob { request, recovery })?;
        self.encoding = true;
        self.frames_requested += 1;
        Ok(())
    }

    /// Fragments and sends one encoded frame.
    fn send_encoded(&mut self, output: EncodedOutput, now: Instant) -> Result<(), RuntimeError> {
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
