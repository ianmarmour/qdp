//! The newest captured frame, shared between the thread a capture API
//! delivers frames on and the encoder thread that takes them.

use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use qdp_codec::Yuv420Frame;

/// The newest frame, or why capture stopped.
#[derive(Debug, Default)]
struct State {
    /// The newest frame, shared with the captures that returned it.
    frame: Option<Arc<Yuv420Frame>>,
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
}

impl LatestFrame {
    /// Stores a new frame.
    pub(crate) fn store(&self, frame: Yuv420Frame) {
        if let Ok(mut state) = self.state.lock() {
            state.frame = Some(Arc::new(frame));
        }
        self.arrived.notify_all();
    }

    /// Records that capture stopped.
    pub(crate) fn fail(&self, error: String) {
        if let Ok(mut state) = self.state.lock() {
            state.error.get_or_insert(error);
        }
        self.arrived.notify_all();
    }

    /// Waits up to `timeout` for a frame and returns the newest, shared.
    pub(crate) fn wait(&self, timeout: Duration) -> Result<Arc<Yuv420Frame>, String> {
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

    /// The newest frame is returned, and an error ends waiting.
    #[test]
    fn keeps_the_newest_frame() {
        let latest = LatestFrame::default();
        assert!(latest.wait(Duration::from_millis(1)).is_err());
        latest.store(Yuv420Frame::black(2, 2));
        latest.store(Yuv420Frame::black(4, 2));
        assert_eq!(latest.wait(Duration::ZERO).unwrap().width(), 4);
        latest.fail("stopped".to_owned());
        assert_eq!(latest.wait(Duration::ZERO), Err("stopped".to_owned()));
    }
}
