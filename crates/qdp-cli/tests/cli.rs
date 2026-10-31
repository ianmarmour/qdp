//! End-to-end tests of the `qdp` binary: `qdp serve` in one process and
//! `qdp connect` in another, over the loopback interface.

use std::fs;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::Duration;

/// The path of the built `qdp` binary.
const QDP: &str = env!("CARGO_BIN_EXE_qdp");

/// The token the test host accepts.
const TOKEN: &str = "cli-test-token";

/// The size of the streamed test pattern.
const WIDTH: usize = 320;

/// The height of the streamed test pattern.
const HEIGHT: usize = 192;

/// The number of frames the client receives.
const FRAMES: usize = 20;

/// How many times the client retries while the host starts.
const CONNECT_ATTEMPTS: u32 = 20;

/// How long the client waits between attempts.
const RETRY_DELAY: Duration = Duration::from_millis(250);

/// Returns a fresh scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("qdp-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&directory);
    fs::create_dir_all(&directory).unwrap();
    directory
}

/// Returns a UDP port that is free at the moment.
fn free_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Runs `qdp pin` and returns the pin it prints.
fn pin(identity: &Path) -> String {
    let output = Command::new(QDP)
        .args(["pin", "--identity"])
        .arg(identity)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// A `qdp serve` process, stopped when dropped.
struct Host(Child);

impl Host {
    /// Starts `qdp serve` on `port`.
    fn start(identity: &Path, port: u16) -> Self {
        let child = Command::new(QDP)
            .args([
                "serve",
                "--listen",
                &format!("127.0.0.1:{port}"),
                "--identity",
            ])
            .arg(identity)
            .args(["--token", TOKEN, "--size", &format!("{WIDTH}x{HEIGHT}")])
            .args(["--source", "test-pattern"])
            .env("QDP_LOG", "warn")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self(child)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Runs `qdp connect` with `extra` arguments, retrying while the host starts,
/// and returns the last attempt's output.
fn connect(port: u16, pin: &str, token: &str, extra: &[&str]) -> Output {
    let mut last = None;
    for _ in 0..CONNECT_ATTEMPTS {
        let output = Command::new(QDP)
            .args([
                "connect",
                &format!("127.0.0.1:{port}"),
                "--pin",
                pin,
                "--token",
                token,
                "--headless",
            ])
            .args(extra)
            .env("QDP_LOG", "warn")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() || !stderr.contains("cannot connect") {
            return output;
        }
        last = Some(output);
        thread::sleep(RETRY_DELAY);
    }
    last.unwrap()
}

/// `qdp connect` receives frames from `qdp serve` and writes them to a
/// YUV4MPEG2 file.
#[test]
fn serve_and_connect() {
    let directory = scratch("stream");
    let identity = directory.join("identity");
    let pin = pin(&identity);
    assert_eq!(pin.len(), 64);
    let port = free_port();
    let _host = Host::start(&identity, port);

    let video = directory.join("out.y4m");
    let video_argument = video.to_str().unwrap();
    let output = connect(
        port,
        &pin,
        TOKEN,
        &["--frames", &FRAMES.to_string(), "--output", video_argument],
    );
    assert!(output.status.success(), "{output:?}");

    let bytes = fs::read(&video).unwrap();
    let header_end = bytes.iter().position(|&byte| byte == b'\n').unwrap();
    let header = std::str::from_utf8(&bytes[..header_end]).unwrap();
    assert!(
        header.starts_with(&format!("YUV4MPEG2 W{WIDTH} H{HEIGHT} ")),
        "{header}"
    );
    let frame_size = b"FRAME\n".len() + WIDTH * HEIGHT * 3 / 2;
    assert_eq!(bytes.len() - header_end - 1, FRAMES * frame_size);
    fs::remove_dir_all(&directory).unwrap();
}

/// `qdp connect` with a wrong token fails.
#[test]
fn wrong_token_fails() {
    let directory = scratch("token");
    let identity = directory.join("identity");
    let pin = pin(&identity);
    let port = free_port();
    let _host = Host::start(&identity, port);

    let output = connect(port, &pin, "wrong-token", &["--frames", "1"]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("authentication failed"), "{stderr}");
    fs::remove_dir_all(&directory).unwrap();
}
