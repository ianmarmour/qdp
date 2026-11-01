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
    /// Stores a new frame, and tells the host it arrived if it watches.
    pub(crate) fn store(&self, frame: CapturedFrame) {
        if let Ok(mut state) = self.state.lock() {
            state.frame = Some(frame);
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
}
