//! The client's touchpad, sent raw for the host's own gestures when the user
//! asks for them (Section 8.4.6).
//!
//! Only Linux clients read their touchpad raw yet, through evdev.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use qdp_runtime::InputHandle;

/// Sends the touchpad's contacts until dropped.
#[derive(Debug)]
pub struct TouchpadCapture {
    /// Whether the window has focus, so that the touchpad is grabbed and its
    /// contacts sent.
    focused: Arc<AtomicBool>,
    /// Raised to stop the thread.
    stop: Arc<AtomicBool>,
    /// The thread reading the touchpad.
    thread: Option<JoinHandle<()>>,
}

impl TouchpadCapture {
    /// Attaches the touchpad and starts sending its contacts to `input`.
    pub fn start(input: &InputHandle) -> Result<Self, String> {
        let focused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "linux")]
        let thread = crate::linux::start_touchpad(input, &focused, &stop)?;
        #[cfg(not(target_os = "linux"))]
        let thread = {
            let _ = input;
            return Err("this client cannot read its touchpad raw".to_owned());
        };
        Ok(Self {
            focused,
            stop,
            thread: Some(thread),
        })
    }

    /// Tells the capture whether the window has focus: the touchpad is
    /// grabbed and its contacts sent only while it has.
    pub fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::Relaxed);
    }
}

impl Drop for TouchpadCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
