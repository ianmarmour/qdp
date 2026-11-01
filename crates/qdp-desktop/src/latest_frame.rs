//! The newest captured frame, shared between the thread a capture API
//! delivers frames on and the encoder thread that takes them.

use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use qdp_runtime::{CapturedFrame, FrameArrivals};

/// The newest frame, or why capture stopped.
#[derive(Debug, Default)]
struct State {
    /// The newest frame, shared with the captures that returned it.
    frame: Option<CapturedFrame>,
    /// Why capture stopped, if it did.
    error: Option<String>,
    /// Whether frames the GPU holds are refused, as they are once the
    /// encoder cannot read them.
    memory_only: bool,
}

/// Whether a frame replaces the one stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replace {
    /// It always does.
    Always,
    /// Only when none is stored.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    IfEmpty,
}

/// Holds the newest frame. Capture APIs deliver frames only when the screen
/// changes, so the newest frame is always the current screen.
#[derive(Debug, Default)]
pub(crate) struct LatestFrame {
    /// The newest frame, or why capture stopped.
    state: Mutex<State>,
    /// Notified when a frame or an error arrives.
    arrived: Condvar,
    /// Told of each new frame, once the host watches for them.
    arrivals: OnceLock<Arc<FrameArrivals>>,
}

impl LatestFrame {
    /// Stores a new frame, and tells the host it arrived if it watches. A
    /// frame the GPU holds is dropped once such frames are refused.
    pub(crate) fn store(&self, frame: CapturedFrame) {
        self.put(frame, Replace::Always);
    }

    /// Refuses frames the GPU holds from now on, and returns the newest
    /// frame if the GPU holds it, which is dropped.
    #[cfg(target_os = "linux")]
    pub(crate) fn refuse_gpu_frames(&self) -> Option<CapturedFrame> {
        let mut state = self.state.lock().ok()?;
        state.memory_only = true;
        match state.frame.take() {
            Some(frame) if frame.in_memory().is_some() => {
                state.frame = Some(frame);
                None
            }
            dropped => dropped,
        }
    }

    /// Stores `frame` if no frame is stored, as when it stands in for one
    /// that was dropped and no other has arrived since.
    #[cfg(target_os = "linux")]
    pub(crate) fn fill(&self, frame: CapturedFrame) {
        self.put(frame, Replace::IfEmpty);
    }

    /// Stores `frame` as `replace` says and tells whoever waits for frames.
    fn put(&self, frame: CapturedFrame, replace: Replace) {
        let stored = self.state.lock().is_ok_and(|mut state| {
            let refused = state.memory_only && frame.in_memory().is_none();
            let occupied = replace == Replace::IfEmpty && state.frame.is_some();
            if refused || occupied {
                return false;
            }
            state.frame = Some(frame);
            true
        });
        if !stored {
            return;
        }
        self.arrived.notify_all();
        if let Some(arrivals) = self.arrivals.get() {
            arrivals.arrived();
        }
    }

    /// Tells `arrivals` of each new frame from now on, and returns that it
    /// will. A session watches its capture once.
    pub(crate) fn watch(&self, arrivals: Arc<FrameArrivals>) -> bool {
        // A capture is a session's alone, so it is watched only once.
        let _ = self.arrivals.set(arrivals);
        true
    }

    /// Records that capture stopped.
    pub(crate) fn fail(&self, error: String) {
        if let Ok(mut state) = self.state.lock() {
            state.error.get_or_insert(error);
        }
        self.arrived.notify_all();
    }

    /// Waits up to `timeout` for a frame and returns the newest, shared.
    pub(crate) fn wait(&self, timeout: Duration) -> Result<CapturedFrame, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "the capture thread panicked".to_owned())?;
        let (state, _) = self
            .arrived
            .wait_timeout_while(state, timeout, |state| {
                state.frame.is_none() && state.error.is_none()
            })
            .map_err(|_| "the capture thread panicked".to_owned())?;
        if let Some(error) = &state.error {
            return Err(error.clone());
        }
        state
            .frame
            .clone()
            .ok_or_else(|| "no frame arrived from the screen".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp_codec::Yuv420Frame;

    /// The newest frame is returned, and an error ends waiting.
    #[test]
    fn keeps_the_newest_frame() {
        let latest = LatestFrame::default();
        assert!(latest.wait(Duration::from_millis(1)).is_err());
        latest.store(Yuv420Frame::black(2, 2).into());
        latest.store(Yuv420Frame::black(4, 2).into());
        assert_eq!(latest.wait(Duration::ZERO).unwrap().size(), (4, 2));
        latest.fail("stopped".to_owned());
        assert_eq!(
            latest.wait(Duration::ZERO).unwrap_err(),
            "stopped".to_owned()
        );
    }

    /// A frame the GPU holds, of `width × 2` pixels.
    #[cfg(target_os = "linux")]
    fn gpu_frame(width: usize) -> CapturedFrame {
        let descriptor = std::fs::File::open("/dev/null").unwrap();
        let layout = qdp_codec::DmaBufLayout {
            width,
            height: 2,
            order: qdp_codec::Rgb32Order::Bgr,
            modifier: qdp_codec::DRM_FORMAT_MOD_LINEAR,
            offset: 0,
            pitch: width as u32 * 4,
        };
        let frame = qdp_codec::DmaBufFrame::new(descriptor.into(), layout).unwrap();
        CapturedFrame::DmaBuf(std::sync::Arc::new(frame))
    }

    /// Once frames the GPU holds are refused, the newest is handed back and
    /// later ones are dropped, and a stand-in fills its place only while no
    /// other frame has arrived.
    #[cfg(target_os = "linux")]
    #[test]
    fn refuses_gpu_frames() {
        let latest = LatestFrame::default();
        latest.store(gpu_frame(8));
        let dropped = latest.refuse_gpu_frames().unwrap();
        assert_eq!(dropped.size(), (8, 2));
        assert!(latest.wait(Duration::ZERO).is_err());
        latest.store(gpu_frame(16));
        assert!(latest.wait(Duration::ZERO).is_err());
        latest.fill(Yuv420Frame::black(8, 2).into());
        assert_eq!(latest.wait(Duration::ZERO).unwrap().size(), (8, 2));
        latest.store(Yuv420Frame::black(4, 2).into());
        latest.fill(Yuv420Frame::black(8, 2).into());
        assert_eq!(latest.wait(Duration::ZERO).unwrap().size(), (4, 2));
        assert!(latest.refuse_gpu_frames().is_none());
    }
}
