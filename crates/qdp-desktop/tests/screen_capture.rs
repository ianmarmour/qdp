//! Captures the screen of a running desktop session through the ScreenCast
//! portal. It needs a session with a portal that shares a monitor without
//! asking, so it is ignored by default; run it with `--ignored` inside one.
//!
//! When `QDP_TEST_SCREEN_COLOUR` is `red`, the frames are expected to be solid
//! red.

#![cfg(all(target_os = "linux", feature = "screen-capture"))]

use std::time::Duration;

use qdp_desktop::{FrameMemory, ScreenCapture};
use qdp_runtime::FrameSource;

/// How long a source runs before its frames are checked.
const SETTLE_TIME: Duration = Duration::from_millis(200);

/// Checks that `capture` delivers frames of its size, with the expected
/// colour if one is set.
fn check_frames(mut capture: ScreenCapture) {
    let (width, height) = capture.size();
    assert!(width > 0 && height > 0);
    let captured = capture.capture().unwrap();
    assert_eq!(captured.size(), (width, height));
    // A source's first buffer for a new stream may not be drawn yet, as
    // GStreamer's are not, so the colour is checked on a later frame.
    std::thread::sleep(SETTLE_TIME);
    let captured = capture.capture().unwrap();
    let Some(frame) = captured.in_memory() else {
        // Frames the GPU holds cannot be read here.
        return;
    };
    if std::env::var("QDP_TEST_SCREEN_COLOUR").as_deref() == Ok("red") {
        let centre = (height / 2) * width + width / 2;
        let chroma = (height / 4) * frame.chroma_width() + width / 4;
        let sample = (frame.y()[centre], frame.u()[chroma], frame.v()[chroma]);
        assert_eq!(sample, (63, 102, 240), "luma and chroma at the centre");
    }
}

/// A monitor shared through the portal delivers frames.
#[test]
#[ignore = "needs a desktop session with a ScreenCast portal"]
fn captures_the_screen() {
    check_frames(ScreenCapture::open(None, qdp_desktop::FrameMemory::Gpu).unwrap());
}

/// A PipeWire video source named by `QDP_TEST_PIPEWIRE_NODE` delivers frames,
/// whether they are asked for in memory or, first, in GPU buffers. A source
/// that cannot share GPU buffers sends frames in memory, and asking for
/// those later keeps them coming.
#[test]
#[ignore = "needs a PipeWire daemon with a video source"]
fn captures_a_pipewire_node() {
    let node = std::env::var("QDP_TEST_PIPEWIRE_NODE")
        .expect("QDP_TEST_PIPEWIRE_NODE names a video source")
        .parse()
        .unwrap();
    check_frames(ScreenCapture::from_pipewire_node(node, FrameMemory::System).unwrap());
    let mut capture = ScreenCapture::from_pipewire_node(node, FrameMemory::Gpu).unwrap();
    capture.use_memory_frames();
    check_frames(capture);
}
