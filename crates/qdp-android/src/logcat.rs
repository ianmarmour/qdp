//! Logging to Android's log, which `adb logcat` shows, under the tag `qdp`.
//! Android's log is a C interface, so this module uses `unsafe`.

#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int};
use std::io::{self, Write};

use tracing::{Level, Metadata};
use tracing_subscriber::fmt::MakeWriter;

/// The tag the core's log lines carry.
const TAG: &CStr = c"qdp";

/// Android's log priorities.
const ANDROID_LOG_VERBOSE: c_int = 2;
/// Debug messages.
const ANDROID_LOG_DEBUG: c_int = 3;
/// Information.
const ANDROID_LOG_INFO: c_int = 4;
/// Warnings.
const ANDROID_LOG_WARN: c_int = 5;
/// Errors.
const ANDROID_LOG_ERROR: c_int = 6;

#[link(name = "log")]
unsafe extern "C" {
    /// Writes `text` to the log with `priority` and `tag`.
    fn __android_log_write(priority: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

/// Sends the core's log to Android's, at information and above, and logs
/// panics there too.
pub fn init() {
    let installed = tracing_subscriber::fmt()
        .with_writer(Logcat)
        .with_max_level(Level::INFO)
        .with_ansi(false)
        .with_level(false)
        .without_time()
        .try_init();
    if installed.is_ok() {
        std::panic::set_hook(Box::new(|panic| tracing::error!("{panic}")));
    }
}

/// Makes a writer for each log line.
struct Logcat;

impl<'a> MakeWriter<'a> for Logcat {
    type Writer = Line;

    fn make_writer(&'a self) -> Line {
        Line::new(ANDROID_LOG_INFO)
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Line {
        Line::new(match *meta.level() {
            Level::TRACE => ANDROID_LOG_VERBOSE,
            Level::DEBUG => ANDROID_LOG_DEBUG,
            Level::INFO => ANDROID_LOG_INFO,
            Level::WARN => ANDROID_LOG_WARN,
            Level::ERROR => ANDROID_LOG_ERROR,
        })
    }
}

/// One log line, written to Android's log when dropped.
struct Line {
    /// Its priority.
    priority: c_int,
    /// Its text so far.
    text: Vec<u8>,
}

impl Line {
    /// Returns an empty line of `priority`.
    fn new(priority: c_int) -> Self {
        Self {
            priority,
            text: Vec::new(),
        }
    }
}

impl Write for Line {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.text.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        let mut text = std::mem::take(&mut self.text);
        text.retain(|byte| *byte != 0);
        while text.last() == Some(&b'\n') {
            text.pop();
        }
        if let Ok(text) = CString::new(text) {
            // SAFETY: the tag and text are NUL-terminated strings that
            // outlive the call.
            unsafe { __android_log_write(self.priority, TAG.as_ptr(), text.as_ptr()) };
        }
    }
}
