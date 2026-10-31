//! Integration tests that run a client and a host session against each other,
//! passing control stream bytes between them as a transport would.

use std::time::{Duration, Instant};

use qdp::message::{
    AuthMethod, AuthResult, AuthStatus, Av1Profiles, Capability, ClientHello, ClientMonitor,
    ClientMonitorFlags, ClientMonitorId, ControlMessage, CursorId, CursorSet, DeviceAttach,
    DeviceFeatures, DeviceId, DeviceKind, DeviceStatus, DeviceStatusCode, DisplayConfig,
    DisplayConfigId, DisplayRequest, DisplayRequestId, HostHello, ImageSerial, MinorVersion,
    NO_LEVEL_LIMIT, PointerKind, PointerMode, VideoCodec, VideoCodecCapability, VideoCodecFlags,
};
use qdp::registry::ErrorCode;
use qdp::session::{
    AuthResponse, CLIENT_HELLO_TIMEOUT, ClientEvent, ClientSession, HostConfig, HostEvent,
    HostSession, Phase, SESSION_START_TIMEOUT, UsageError,
};
use qdp::wire::{SignedVarInt, VarInt};

/// The token the test host accepts.
const VALID_TOKEN: &[u8] = b"valid-token";

/// Returns an AV1 capability.
fn av1() -> Capability {
    Capability::VideoCodec(VideoCodecCapability {
        codec: VideoCodec::AV1,
        profiles: Av1Profiles::MAIN,
        max_bit_depth: 10,
        max_width: VarInt::from_u32(3840),
        max_height: VarInt::from_u32(2160),
        max_pixel_rate: VarInt::ZERO,
        flags: VideoCodecFlags::default(),
        max_level: NO_LEVEL_LIMIT,
    })
}

/// Returns a client hello that supports AV1.
fn client_hello() -> ClientHello {
    ClientHello {
        minor_version: MinorVersion::CURRENT,
        client_name: "qdp-test-client/0.1.0".to_owned(),
        capabilities: vec![av1()],
    }
}

/// Returns a host configuration with `capabilities` that accepts tokens and SCRAM.
fn host_config(capabilities: Vec<Capability>) -> HostConfig {
    HostConfig {
        host_hello: HostHello {
            minor_version: MinorVersion::CURRENT,
            host_name: "test-host".to_owned(),
            auth_methods: vec![AuthMethod::TOKEN, AuthMethod::SCRAM_SHA_256_PLUS],
            capabilities,
        },
        failure_reason: "authentication failed".to_owned(),
    }
}

/// Returns the device that the client attaches during configuration.
fn keyboard() -> ControlMessage {
    ControlMessage::DeviceAttach(DeviceAttach {
        device_id: DeviceId(VarInt::from_u32(1)),
        kind: DeviceKind::KEYBOARD,
        features: DeviceFeatures::default(),
        name: "Keyboard".to_owned(),
        touchpad: None,
        passthrough: None,
    })
}

/// Returns the client's display request.
fn display_request() -> ControlMessage {
    ControlMessage::DisplayRequest(DisplayRequest {
        request_id: DisplayRequestId(VarInt::from_u32(1)),
        monitors: vec![ClientMonitor {
            id: ClientMonitorId(VarInt::from_u32(1)),
            flags: ClientMonitorFlags::PRIMARY,
            x: SignedVarInt::ZERO,
            y: SignedVarInt::ZERO,
            width: VarInt::from_u32(1920),
            height: VarInt::from_u32(1080),
            refresh_rate: VarInt::from_u32(60_000),
            scale: VarInt::from_u32(100),
            physical_width: VarInt::ZERO,
            physical_height: VarInt::ZERO,
            peak_luminance: VarInt::ZERO,
        }],
    })
}

/// A client and a host connected by an ideal transport.
struct Pair {
    /// The client.
    client: ClientSession,
    /// The host.
    host: HostSession,
    /// The test's notion of the current time.
    now: Instant,
}

impl Pair {
    /// Creates a client and a host whose QUIC handshake has just completed.
    fn new(host_capabilities: Vec<Capability>) -> Self {
        let now = Instant::now();
        Self {
            client: ClientSession::new(client_hello()).unwrap(),
            host: HostSession::new(host_config(host_capabilities), now),
            now,
        }
    }

    /// Delivers pending bytes in both directions until neither side has any,
    /// returning the first error either side reports.
    fn pump(&mut self) -> Result<(), ErrorCode> {
        loop {
            let mut moved = false;
            if let Some(bytes) = self.client.poll_transmit() {
                self.host
                    .handle_control_data(&bytes)
                    .map_err(|error| error.code)?;
                moved = true;
            }
            if let Some(bytes) = self.host.poll_transmit() {
                self.client
                    .handle_control_data(&bytes)
                    .map_err(|error| error.code)?;
                moved = true;
            }
            if !moved {
                return Ok(());
            }
        }
    }

    /// Returns every pending host event.
    fn host_events(&mut self) -> Vec<HostEvent> {
        std::iter::from_fn(|| self.host.poll_event()).collect()
    }

    /// Returns every pending client event.
    fn client_events(&mut self) -> Vec<ClientEvent> {
        std::iter::from_fn(|| self.client.poll_event()).collect()
    }

    /// Answers the host's pending authentication step, accepting the valid token.
    fn answer_token(&mut self) {
        let events = self.host_events();
        let Some(HostEvent::AuthStart { auth_data, .. }) = events.last() else {
            panic!("expected AUTH_START, got {events:?}");
        };
        let response = if auth_data == VALID_TOKEN {
            AuthResponse::Accept(Vec::new())
        } else {
            AuthResponse::Reject
        };
        self.host.respond_auth(response, self.now).unwrap();
    }

    /// Sends the host's configuration answer and `SESSION_START`.
    fn host_configures(&mut self) {
        for event in self.host_events() {
            if let HostEvent::Message(ControlMessage::DeviceAttach(attach)) = event {
                let status = DeviceStatus {
                    device_id: attach.device_id,
                    status: DeviceStatusCode::Accepted,
                    reason: String::new(),
                };
                self.host
                    .send(&ControlMessage::DeviceStatus(status))
                    .unwrap();
            }
        }
        let display_config = DisplayConfig {
            config_id: DisplayConfigId(VarInt::from_u32(1)),
            request_id: DisplayRequestId(VarInt::from_u32(1)),
            monitors: Vec::new(),
            streams: Vec::new(),
        };
        self.host
            .send(&ControlMessage::DisplayConfig(display_config))
            .unwrap();
        let pointer_mode = PointerMode {
            mode: PointerKind::Absolute,
            confinement: None,
        };
        self.host
            .send(&ControlMessage::PointerMode(pointer_mode))
            .unwrap();
        let cursor = CursorSet {
            cursor_id: CursorId::HIDDEN,
            image_serial: ImageSerial::default(),
        };
        self.host.send(&ControlMessage::CursorSet(cursor)).unwrap();
        self.host.start().unwrap();
    }
}

/// A token-authenticated session reaches the active phase on both sides.
#[test]
fn token_handshake() {
    let mut pair = Pair::new(vec![av1()]);
    pair.pump().unwrap();
    assert_eq!(pair.host.phase(), Phase::Authenticating);
    assert!(matches!(
        pair.client_events().as_slice(),
        [ClientEvent::HostHello(_)]
    ));

    pair.client
        .start_auth(AuthMethod::TOKEN, VALID_TOKEN.to_vec())
        .unwrap();
    pair.pump().unwrap();
    pair.answer_token();
    pair.pump().unwrap();
    assert_eq!(pair.client.phase(), Phase::Configuring);
    assert_eq!(pair.host.phase(), Phase::Configuring);

    pair.client.send(&display_request()).unwrap();
    pair.client.send(&keyboard()).unwrap();
    pair.client.ready(pair.now).unwrap();
    pair.pump().unwrap();
    pair.host_configures();
    pair.pump().unwrap();

    assert_eq!(pair.host.phase(), Phase::Active);
    assert_eq!(pair.client.phase(), Phase::Active);
    assert!(pair.host.accepts_datagrams());
    assert!(pair.client.can_send_media());
    let events = pair.client_events();
    assert!(matches!(events.last(), Some(ClientEvent::SessionStarted)));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, ClientEvent::Message(ControlMessage::DeviceStatus(_))))
    );
}

/// A multi-step method exchanges `AUTH_CONTINUE` in both directions.
#[test]
fn multi_step_authentication() {
    let mut pair = Pair::new(vec![av1()]);
    pair.pump().unwrap();
    pair.client
        .start_auth(AuthMethod::SCRAM_SHA_256_PLUS, b"client-first".to_vec())
        .unwrap();
    pair.pump().unwrap();
    assert!(matches!(
        pair.host_events().last(),
        Some(HostEvent::AuthStart { method, .. }) if *method == AuthMethod::SCRAM_SHA_256_PLUS
    ));
    pair.host
        .respond_auth(AuthResponse::Continue(b"server-first".to_vec()), pair.now)
        .unwrap();
    pair.pump().unwrap();
    assert!(matches!(
        pair.client_events().last(),
        Some(ClientEvent::AuthContinue { auth_data }) if auth_data == b"server-first"
    ));
    pair.client.continue_auth(b"client-final".to_vec()).unwrap();
    pair.pump().unwrap();
    assert!(matches!(
        pair.host_events().last(),
        Some(HostEvent::AuthContinue { auth_data }) if auth_data == b"client-final"
    ));
    pair.host
        .respond_auth(AuthResponse::Accept(b"server-final".to_vec()), pair.now)
        .unwrap();
    pair.pump().unwrap();
    assert!(matches!(
        pair.client_events().last(),
        Some(ClientEvent::AuthResult(AuthResult { status: AuthStatus::Success, auth_data, .. }))
            if auth_data == b"server-final"
    ));
}

/// The third failed attempt ends the connection with `QDP_AUTH_FAILED`.
#[test]
fn three_failures_close_the_connection() {
    let mut pair = Pair::new(vec![av1()]);
    pair.pump().unwrap();
    for attempt in 1..=3 {
        pair.client
            .start_auth(AuthMethod::TOKEN, b"wrong".to_vec())
            .unwrap();
        pair.pump().unwrap();
        pair.answer_token();
        pair.pump().unwrap();
        let events = pair.client_events();
        let Some(ClientEvent::AuthResult(result)) = events.last() else {
            panic!("expected AUTH_RESULT, got {events:?}");
        };
        let expected = if attempt < 3 {
            AuthStatus::FailedRetry
        } else {
            AuthStatus::FailedFinal
        };
        assert_eq!(result.status, expected);
        assert_eq!(result.reason, "authentication failed");
    }
    assert_eq!(pair.host.phase(), Phase::Closed);
    assert_eq!(
        pair.host.close_reason().map(|error| error.code),
        Some(ErrorCode::AUTH_FAILED)
    );
}

/// A method the host did not offer fails the attempt without the application.
#[test]
fn unoffered_method_fails() {
    let mut pair = Pair::new(vec![av1()]);
    // Pipelined before HOST_HELLO, so the client cannot know it is not offered.
    pair.client
        .start_auth(AuthMethod::ANONYMOUS, Vec::new())
        .unwrap();
    pair.pump().unwrap();
    assert!(
        !pair
            .host_events()
            .iter()
            .any(|event| matches!(event, HostEvent::AuthStart { .. }))
    );
    assert!(matches!(
        pair.client_events().last(),
        Some(ClientEvent::AuthResult(AuthResult {
            status: AuthStatus::FailedRetry,
            ..
        }))
    ));
}

/// A fully pipelined client becomes configured as soon as the host accepts it.
#[test]
fn pipelined_configuration() {
    let mut pair = Pair::new(vec![av1()]);
    pair.client
        .start_auth(AuthMethod::TOKEN, VALID_TOKEN.to_vec())
        .unwrap();
    pair.client.send(&display_request()).unwrap();
    pair.client.send(&keyboard()).unwrap();
    pair.client.ready(pair.now).unwrap();
    pair.pump().unwrap();

    pair.answer_token();
    let events = pair.host_events();
    assert!(matches!(
        events.as_slice(),
        [
            HostEvent::Message(ControlMessage::DisplayRequest(_)),
            HostEvent::Message(ControlMessage::DeviceAttach(_)),
            HostEvent::Message(ControlMessage::ClientReady(_)),
        ]
    ));
    // Feed the replayed attachment back so the host can answer it.
    for event in events {
        if let HostEvent::Message(ControlMessage::DeviceAttach(attach)) = event {
            let status = DeviceStatus {
                device_id: attach.device_id,
                status: DeviceStatusCode::Accepted,
                reason: String::new(),
            };
            pair.host
                .send(&ControlMessage::DeviceStatus(status))
                .unwrap();
        }
    }
    pair.host_configures();
    pair.pump().unwrap();
    assert_eq!(pair.client.phase(), Phase::Active);
}

/// After a failed pipelined attempt the host discards the configuration, and the
/// client must send it again.
#[test]
fn failed_pipelined_attempt_discards_configuration() {
    let mut pair = Pair::new(vec![av1()]);
    pair.client
        .start_auth(AuthMethod::TOKEN, b"wrong".to_vec())
        .unwrap();
    pair.client.send(&display_request()).unwrap();
    pair.client.ready(pair.now).unwrap();
    pair.pump().unwrap();
    pair.answer_token();
    pair.pump().unwrap();

    pair.client
        .start_auth(AuthMethod::TOKEN, VALID_TOKEN.to_vec())
        .unwrap();
    pair.pump().unwrap();
    pair.answer_token();
    pair.pump().unwrap();
    assert!(pair.host_events().is_empty());
    assert_eq!(pair.host.phase(), Phase::Configuring);
    // CLIENT_READY counts once per successful attempt, so it may be sent again.
    pair.client.send(&display_request()).unwrap();
    pair.client.ready(pair.now).unwrap();
    pair.pump().unwrap();
    assert!(matches!(
        pair.host_events().last(),
        Some(HostEvent::Message(ControlMessage::ClientReady(_)))
    ));
}

/// Without a common video codec the host closes after sending `HOST_HELLO`.
#[test]
fn no_common_codec() {
    let mut pair = Pair::new(Vec::new());
    assert_eq!(pair.pump(), Err(ErrorCode::NO_COMMON_CAPABILITY));
    let bytes = pair.host.poll_transmit().expect("HOST_HELLO is still sent");
    assert_eq!(bytes[0], 0x02);
}

/// The host's handshake timers close the session with `QDP_HANDSHAKE_TIMEOUT`.
#[test]
fn host_hello_timeout() {
    let pair = Pair::new(vec![av1()]);
    let mut host = pair.host;
    let deadline = host.poll_timeout().unwrap();
    assert_eq!(deadline, pair.now + CLIENT_HELLO_TIMEOUT);
    host.handle_timeout(deadline - Duration::from_millis(1))
        .unwrap();
    let error = host.handle_timeout(deadline).unwrap_err();
    assert_eq!(error.code, ErrorCode::HANDSHAKE_TIMEOUT);
}

/// The client gives up if `SESSION_START` does not follow `CLIENT_READY`.
#[test]
fn client_session_start_timeout() {
    let mut pair = Pair::new(vec![av1()]);
    pair.pump().unwrap();
    pair.client
        .start_auth(AuthMethod::TOKEN, VALID_TOKEN.to_vec())
        .unwrap();
    pair.pump().unwrap();
    pair.answer_token();
    pair.pump().unwrap();
    pair.client.ready(pair.now).unwrap();
    assert_eq!(
        pair.client.poll_timeout(),
        Some(pair.now + SESSION_START_TIMEOUT)
    );
    let error = pair
        .client
        .handle_timeout(pair.now + SESSION_START_TIMEOUT)
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::HANDSHAKE_TIMEOUT);
}

/// The host refuses to start before every required message is sent.
#[test]
fn start_requires_configuration() {
    let mut pair = Pair::new(vec![av1()]);
    pair.client
        .start_auth(AuthMethod::TOKEN, VALID_TOKEN.to_vec())
        .unwrap();
    pair.client.send(&keyboard()).unwrap();
    pair.client.ready(pair.now).unwrap();
    pair.pump().unwrap();
    pair.answer_token();
    assert!(matches!(
        pair.host.start(),
        Err(UsageError::InvalidState(_))
    ));
}

/// Messages outside the table of Section 4.2 are protocol violations or usage
/// errors.
#[test]
fn messages_out_of_phase() {
    let mut pair = Pair::new(vec![av1()]);
    pair.pump().unwrap();
    assert!(matches!(
        pair.client.send(&display_request()),
        Err(UsageError::NotAllowed { .. })
    ));
    assert!(matches!(
        pair.host.send(&keyboard()),
        Err(UsageError::NotAllowed { .. })
    ));

    // A raw CLIENT_READY before authentication is pipelined, so a second one is a
    // violation once the host processes both after accepting.
    let mut host = HostSession::new(host_config(vec![av1()]), pair.now);
    let mut bytes = qdp::message::MessagePayload::to_frame(&client_hello())
        .to_bytes()
        .unwrap();
    bytes.extend([0x06, 0x00]);
    bytes.extend([0x03, 0x02, 0x02, 0x00]);
    bytes.extend([0x06, 0x00, 0x06, 0x00]);
    host.handle_control_data(&bytes).unwrap();
    let _ = std::iter::from_fn(|| host.poll_event()).count();
    host.respond_auth(AuthResponse::Accept(Vec::new()), pair.now)
        .unwrap();
    assert_eq!(
        host.close_reason().map(|error| error.code),
        Some(ErrorCode::PROTOCOL_VIOLATION)
    );
}

/// A message before the hello is a protocol violation.
#[test]
fn message_before_hello() {
    let mut host = HostSession::new(host_config(vec![av1()]), Instant::now());
    let error = host.handle_control_data(&[0x06, 0x00]).unwrap_err();
    assert_eq!(error.code, ErrorCode::PROTOCOL_VIOLATION);
}
