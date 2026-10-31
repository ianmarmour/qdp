//! End-to-end tests that run a QDP host and client over a real QUIC connection on
//! the loopback interface.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use qdp::auth::scram::{ClientFirst, ScramClient, ScramCredentials, ScramServer};
use qdp::message::{
    AuthMethod, AuthStatus, Av1Profiles, Capability, ClientHello, ClientMonitor,
    ClientMonitorFlags, ClientMonitorId, ControlMessage, CursorId, CursorSet, Datagram,
    DatagramPayload, DeviceAttach, DeviceFeatures, DeviceId, DeviceKind, DeviceStatus,
    DeviceStatusCode, DisplayConfig, DisplayConfigId, DisplayRequest, DisplayRequestId,
    HidOutputDecoder, HostHello, ImageSerial, MinorVersion, NO_LEVEL_LIMIT, PointerKind,
    PointerMode, StreamHeader, VideoCodec, VideoCodecCapability, VideoCodecFlags,
    encode_hid_output_report,
};
use qdp::registry::{ErrorCode, StreamType};
use qdp::session::{AuthResponse, ClientEvent, HostConfig, HostEvent, Phase};
use qdp::time::{ClockSync, SessionClock, answer_ping};
use qdp::wire::{SignedVarInt, VarInt};
use qdp_quic::{
    ClientDriver, ClientEndpoint, DriverInput, HostDriver, HostEndpoint, HostIdentity,
    HostVerification, KeyPin, StreamFailure, StreamLimit, StreamPolicy, TransportError,
};

/// The longest a test may run before it is considered hung.
const TEST_TIMEOUT: Duration = Duration::from_secs(20);

/// The user name the test host knows.
const USERNAME: &str = "ian";

/// The test user's password.
const PASSWORD: &str = "correct horse battery staple";

/// The SCRAM iteration count of the test credentials.
const ITERATIONS: u32 = 4096;

/// The server name the client sends.
const SERVER_NAME: &str = "qdp-host";

/// The data of the cursor image stream the host sends.
const STREAM_DATA: &[u8] = b"cursor image";

/// The most data the client accepts on a cursor image stream.
const STREAM_LIMIT: usize = 16;

/// The device the host's HID output stream is for.
const OUTPUT_DEVICE: DeviceId = DeviceId(VarInt::from_u32(7));

/// The output reports the host sends on its HID output stream, some time
/// apart.
const OUTPUT_REPORTS: [&[u8]; 2] = [&[0x01, 0xff], &[0x02, 0x00, 0x10]];

/// A stream type no version defines.
const UNKNOWN_STREAM_TYPE: StreamType = StreamType(VarInt::from_u32(0x3f));

/// Returns a stream's bytes: its header, then `data`.
fn stream(header: StreamHeader, data: &[u8]) -> Vec<u8> {
    [header.to_bytes(), data.to_vec()].concat()
}

/// The client's policy for the host's streams.
fn stream_policy(header: &StreamHeader) -> StreamPolicy {
    match header {
        StreamHeader::CursorImage => StreamPolicy::Read(StreamLimit {
            max_bytes: STREAM_LIMIT,
            overflow: ErrorCode::PROTOCOL_VIOLATION,
        }),
        StreamHeader::HidOutput { .. } => StreamPolicy::Follow,
        StreamHeader::Unknown(_) => StreamPolicy::Refuse(ErrorCode::STREAM_TYPE_UNKNOWN),
        _ => StreamPolicy::Forbid(ErrorCode::STREAM_CREATION_ERROR),
    }
}

/// Returns an unspecified loopback address with an ephemeral port.
fn loopback() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

/// Returns an AV1 capability.
fn av1() -> Capability {
    Capability::VideoCodec(VideoCodecCapability {
        codec: VideoCodec::AV1,
        profiles: Av1Profiles::MAIN,
        max_bit_depth: 8,
        max_width: VarInt::from_u32(3840),
        max_height: VarInt::from_u32(2160),
        max_pixel_rate: VarInt::ZERO,
        flags: VideoCodecFlags::default(),
        max_level: NO_LEVEL_LIMIT,
    })
}

/// Returns the host's session configuration.
fn host_config() -> HostConfig {
    HostConfig {
        host_hello: HostHello {
            minor_version: MinorVersion::CURRENT,
            host_name: "loopback-host".to_owned(),
            auth_methods: vec![AuthMethod::SCRAM_SHA_256_PLUS],
            capabilities: vec![av1()],
        },
        failure_reason: "authentication failed".to_owned(),
    }
}

/// Returns the client's hello.
fn client_hello() -> ClientHello {
    ClientHello {
        minor_version: MinorVersion::CURRENT,
        client_name: "qdp-loopback-test/0.1.0".to_owned(),
        capabilities: vec![av1()],
    }
}

/// Runs the host's side of one session: SCRAM authentication, configuration,
/// one clock synchronization exchange, and then waits for the client to close.
async fn run_host(host: HostEndpoint, credentials: ScramCredentials) -> TransportError {
    let connection = host.accept().await.expect("a client").expect("a handshake");
    let binding = connection.channel_binding().expect("a TLS exporter");
    let clock = SessionClock::new(Instant::now());
    let mut driver = HostDriver::host(connection, host_config());
    let mut scram = None;
    loop {
        match driver.next_event().await.expect("the session continues") {
            HostEvent::ClientHello(_) => {}
            HostEvent::AuthStart { auth_data, .. } => {
                let first = ClientFirst::parse(&auth_data).expect("a client-first-message");
                let known = (first.username == USERNAME).then(|| credentials.clone());
                let (server, server_first) =
                    ScramServer::new(first, known, b"host secret", ITERATIONS, &[3; 18], binding);
                scram = Some(server);
                driver
                    .session()
                    .respond_auth(AuthResponse::Continue(server_first), Instant::now())
                    .unwrap();
            }
            HostEvent::AuthContinue { auth_data } => {
                let server = scram.take().expect("a SCRAM exchange in progress");
                let response = match server.handle_client_final(&auth_data) {
                    Ok(server_final) => AuthResponse::Accept(server_final),
                    Err(_) => AuthResponse::Reject,
                };
                driver
                    .session()
                    .respond_auth(response, Instant::now())
                    .unwrap();
            }
            HostEvent::Message(ControlMessage::DeviceAttach(attach)) => {
                let status = DeviceStatus {
                    device_id: attach.device_id,
                    status: DeviceStatusCode::Accepted,
                    reason: String::new(),
                };
                driver
                    .session()
                    .send(&ControlMessage::DeviceStatus(status))
                    .unwrap();
            }
            HostEvent::Message(ControlMessage::ClientReady(_)) => break,
            HostEvent::Message(_) => {}
        }
    }

    let session = driver.session();
    let display_config = DisplayConfig {
        config_id: DisplayConfigId(VarInt::from_u32(1)),
        request_id: DisplayRequestId(VarInt::from_u32(1)),
        monitors: Vec::new(),
        streams: Vec::new(),
    };
    session
        .send(&ControlMessage::DisplayConfig(display_config))
        .unwrap();
    session
        .send(&ControlMessage::PointerMode(PointerMode {
            mode: PointerKind::Absolute,
            confinement: None,
        }))
        .unwrap();
    session
        .send(&ControlMessage::CursorSet(CursorSet {
            cursor_id: CursorId::HIDDEN,
            image_serial: ImageSerial::default(),
        }))
        .unwrap();
    session.start().unwrap();
    assert_eq!(session.phase(), Phase::Active);
    driver.flush().await.unwrap();

    let datagram = driver.connection().quic().read_datagram().await.unwrap();
    let Datagram::TimePing(ping) = Datagram::decode(&datagram).unwrap() else {
        panic!("expected TIME_PING");
    };
    let now = Instant::now();
    let pong = answer_ping(&ping, &clock, now, now);
    driver.connection().send_datagram(pong.to_bytes()).unwrap();

    let connection = driver.connection();
    connection.send_stream(stream(
        StreamHeader::Unknown(UNKNOWN_STREAM_TYPE),
        b"ignored",
    ));
    connection.send_stream(stream(StreamHeader::CursorImage, STREAM_DATA));
    connection.send_stream(stream(StreamHeader::CursorImage, &[0; STREAM_LIMIT + 1]));
    // Output reports go out on one stream as they happen, which dropping it
    // finishes.
    let output = connection.open_stream(
        StreamHeader::HidOutput {
            device_id: OUTPUT_DEVICE,
        }
        .to_bytes(),
    );
    for report in OUTPUT_REPORTS {
        assert!(output.send(encode_hid_output_report(report)));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    drop(output);

    driver
        .next_event()
        .await
        .expect_err("the client closes the session")
}

/// Runs the client's side of one session: SCRAM authentication, configuration,
/// one clock synchronization exchange and the host's streams, then closes the
/// session.
async fn run_client(address: SocketAddr, pin: KeyPin) {
    let endpoint = ClientEndpoint::bind(loopback()).unwrap();
    let connection = endpoint
        .connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![pin]),
        )
        .await
        .unwrap();
    let binding = connection.channel_binding().unwrap();
    let mut driver = ClientDriver::client(connection, client_hello()).unwrap();
    let mut scram_first = None;
    let mut scram_final = None;
    loop {
        match driver.next_event().await.expect("the session continues") {
            ClientEvent::HostHello(_) => {
                let (client, first) =
                    ScramClient::new(USERNAME, PASSWORD, &[5; 18], binding).unwrap();
                scram_first = Some(client);
                driver
                    .session()
                    .start_auth(AuthMethod::SCRAM_SHA_256_PLUS, first)
                    .unwrap();
            }
            ClientEvent::AuthContinue { auth_data } => {
                let client = scram_first.take().unwrap();
                let (next, client_final) = client.handle_server_first(&auth_data).unwrap();
                scram_final = Some(next);
                driver.session().continue_auth(client_final).unwrap();
            }
            ClientEvent::AuthResult(result) => {
                assert_eq!(result.status, AuthStatus::Success);
                scram_final
                    .take()
                    .unwrap()
                    .verify_server_final(&result.auth_data)
                    .expect("the host proves it knows the password verifier");
                let session = driver.session();
                session.send(&display_request()).unwrap();
                session.send(&keyboard()).unwrap();
                session.ready(Instant::now()).unwrap();
            }
            ClientEvent::Message(_) => {}
            ClientEvent::SessionStarted => break,
        }
    }
    assert!(driver.session().can_send_media());

    let now = Instant::now();
    let mut sync = ClockSync::new(SessionClock::new(now), now);
    let ping = sync.poll_ping(now).unwrap();
    driver.connection().send_datagram(ping.to_bytes()).unwrap();
    let datagram = driver.connection().quic().read_datagram().await.unwrap();
    let Datagram::TimePong(pong) = Datagram::decode(&datagram).unwrap() else {
        panic!("expected TIME_PONG");
    };
    let sample = sync
        .handle_pong(&pong, Instant::now())
        .expect("a clock sample");
    assert!(sample.delay < Duration::from_secs(1));

    // The unknown stream is refused, so only the cursor images arrive, and
    // the output stream's reports, as they come.
    driver.accept_streams(stream_policy);
    let mut received = Vec::new();
    let mut decoder = HidOutputDecoder::new();
    let mut reports = Vec::new();
    let mut output_end = None;
    while received.len() < 2 || output_end.is_none() {
        match driver.next_input(None, None).await.unwrap() {
            DriverInput::Stream(stream) => {
                assert_eq!(stream.header, StreamHeader::CursorImage);
                received.push(stream.data);
            }
            DriverInput::StreamChunk(chunk) => {
                assert_eq!(
                    chunk.header,
                    StreamHeader::HidOutput {
                        device_id: OUTPUT_DEVICE
                    }
                );
                decoder.push(&chunk.data);
                while let Some(report) = decoder.next_report().unwrap() {
                    reports.push(report);
                }
                output_end = chunk.end;
            }
            _ => {}
        }
    }
    assert!(received.contains(&Ok(STREAM_DATA.to_vec())));
    assert!(received.contains(&Err(StreamFailure::TooLong)));
    assert_eq!(reports, OUTPUT_REPORTS.map(<[u8]>::to_vec));
    assert_eq!(output_end, Some(Ok(())));

    driver.close("test complete");
    endpoint.wait_idle().await;
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

/// Returns the keyboard the client attaches.
fn keyboard() -> ControlMessage {
    ControlMessage::DeviceAttach(DeviceAttach {
        device_id: DeviceId(VarInt::from_u32(1)),
        kind: DeviceKind::KEYBOARD,
        features: DeviceFeatures::default(),
        name: "Loopback Keyboard".to_owned(),
        touchpad: None,
        passthrough: None,
    })
}

/// A client authenticates with SCRAM-SHA-256-PLUS bound to the real TLS
/// connection, configures the session, reaches the active phase and exchanges
/// clock synchronization datagrams with the host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_over_quic() {
    let identity = HostIdentity::generate_self_signed(vec![SERVER_NAME.to_owned()]).unwrap();
    let pin = identity.key_pin().unwrap();
    let host = HostEndpoint::bind(loopback(), identity).unwrap();
    let address = host.local_address().unwrap();
    let credentials =
        ScramCredentials::derive(PASSWORD, b"loopback salt".to_vec(), ITERATIONS).unwrap();

    let host_task = tokio::spawn(run_host(host, credentials));
    tokio::time::timeout(TEST_TIMEOUT, run_client(address, pin))
        .await
        .expect("the client finishes in time");
    let host_error = tokio::time::timeout(TEST_TIMEOUT, host_task)
        .await
        .expect("the host finishes in time")
        .unwrap();
    match host_error {
        TransportError::Read(quinn::ReadError::ConnectionLost(
            quinn::ConnectionError::ApplicationClosed(close),
        )) => assert_eq!(close.error_code, quinn::VarInt::from_u32(0)),
        other => panic!("unexpected host error: {other}"),
    }
}

/// A client refuses a host whose key does not match its pin.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_pin_is_refused() {
    let identity = HostIdentity::generate_self_signed(vec![SERVER_NAME.to_owned()]).unwrap();
    let host = HostEndpoint::bind(loopback(), identity).unwrap();
    let address = host.local_address().unwrap();
    let host_task = tokio::spawn(async move { host.accept().await.map(|result| result.is_ok()) });

    let other_pin = HostIdentity::generate_self_signed(vec![SERVER_NAME.to_owned()])
        .unwrap()
        .key_pin()
        .unwrap();
    let endpoint = ClientEndpoint::bind(loopback()).unwrap();
    let result = tokio::time::timeout(
        TEST_TIMEOUT,
        endpoint.connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![other_pin]),
        ),
    )
    .await
    .expect("the handshake ends in time");
    assert!(matches!(result, Err(TransportError::Connection(_))));
    let accepted = tokio::time::timeout(TEST_TIMEOUT, host_task)
        .await
        .expect("the host finishes in time")
        .unwrap();
    assert_eq!(accepted, Some(false));
}
