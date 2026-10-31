//! End-to-end tests that stream AV1 video from a host to a client over a real
//! QUIC connection on the loopback interface.

use std::net::SocketAddr;
use std::time::Duration;

use qdp::auth::scram::ScramCredentials;
use qdp_codec::{EncoderPreference, Yuv420Frame};
use qdp_quic::{ClientEndpoint, HostEndpoint, HostIdentity, HostVerification, KeyPin};
use qdp_runtime::{
    ClientCredentials, ClientOptions, DisplaySize, EncoderTuning, FrameInfo, FrameSink,
    HostAuthenticator, HostOptions, RuntimeError, SinkControl, TestPattern, run, serve,
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

/// Collects what the client presents and stops after [`FRAMES`] frames.
#[derive(Default)]
struct Collector {
    /// The frame number, luma PSNR against the pattern, and frame information
    /// of each presented frame.
    frames: Vec<(u64, f64, FrameInfo)>,
}

impl FrameSink for Collector {
    fn present(&mut self, frame: &Yuv420Frame, info: FrameInfo) -> SinkControl {
        // The host sends every frame it captures, so frame number n shows pattern
        // frame n - 1.
        let number = info.frame_number.0.into_inner();
        let expected = TestPattern::frame(WIDTH, HEIGHT, number - 1);
        let psnr = frame.luma_psnr(&expected).unwrap_or(0.0);
        self.frames.push((number, psnr, info));
        if self.frames.len() >= FRAMES {
            SinkControl::Stop
        } else {
            SinkControl::Continue
        }
    }
}

/// Starts a host serving one connection, and returns its address, its key pin
/// and the task that returns the host's summary.
fn start_host(
    authenticator: HostAuthenticator,
) -> (
    SocketAddr,
    KeyPin,
    tokio::task::JoinHandle<Result<qdp_runtime::HostSummary, RuntimeError>>,
) {
    let identity = HostIdentity::generate_self_signed(vec![SERVER_NAME.to_owned()]).unwrap();
    let pin = identity.key_pin().unwrap();
    let endpoint = HostEndpoint::bind(loopback(), identity).unwrap();
    let address = endpoint.local_address().unwrap();
    let options = HostOptions {
        host_name: "loopback".to_owned(),
        authenticator,
        frame_rate: 60,
        encoder: EncoderTuning {
            preference: EncoderPreference::Software,
            speed: 10,
            quantizer: 60,
            threads: 2,
        },
    };
    let task = tokio::spawn(async move {
        let connection = endpoint.accept().await.expect("a client")?;
        serve(
            connection,
            &options,
            Box::new(TestPattern::new(WIDTH, HEIGHT)),
        )
        .await
    });
    (address, pin, task)
}

/// Connects a client with `credentials` and streams until the collector stops.
async fn stream(
    address: SocketAddr,
    pin: KeyPin,
    credentials: ClientCredentials,
) -> Result<Collector, RuntimeError> {
    let endpoint = ClientEndpoint::bind(loopback()).unwrap();
    let connection = endpoint
        .connect(
            address,
            SERVER_NAME,
            HostVerification::PinnedKeys(vec![pin]),
        )
        .await?;
    let options = ClientOptions {
        client_name: "qdp-streaming-test/0.1.0".to_owned(),
        credentials,
        display: DisplaySize {
            width: WIDTH as u32,
            height: HEIGHT as u32,
            refresh_millihertz: 60_000,
        },
        decoder_threads: 2,
    };
    let mut collector = Collector::default();
    let summary = run(connection, &options, &mut collector).await?;
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
    let (address, pin, host) = start_host(authenticator);
    let collector = tokio::time::timeout(
        TEST_TIMEOUT,
        stream(address, pin, ClientCredentials::Token(TOKEN.to_vec())),
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

/// A client with a password streams video from a host over SCRAM-SHA-256-PLUS.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_with_password() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_user(
        "ian".to_owned(),
        ScramCredentials::derive("pencil", b"streaming salt".to_vec(), 4096).unwrap(),
    );
    let (address, pin, _host) = start_host(authenticator);
    let credentials = ClientCredentials::Password {
        username: "ian".to_owned(),
        password: "pencil".to_owned(),
    };
    let collector = tokio::time::timeout(TEST_TIMEOUT, stream(address, pin, credentials))
        .await
        .expect("streaming finishes in time")
        .unwrap();
    check_frames(&collector);
}

/// A client with the wrong password is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_password_is_refused() {
    let mut authenticator = HostAuthenticator::new().unwrap();
    authenticator.add_user(
        "ian".to_owned(),
        ScramCredentials::derive("pencil", b"streaming salt".to_vec(), 4096).unwrap(),
    );
    let (address, pin, _host) = start_host(authenticator);
    let credentials = ClientCredentials::Password {
        username: "ian".to_owned(),
        password: "wrong".to_owned(),
    };
    let result = tokio::time::timeout(TEST_TIMEOUT, stream(address, pin, credentials))
        .await
        .expect("the attempt finishes in time");
    assert!(matches!(result, Err(RuntimeError::AuthenticationFailed(_))));
}
