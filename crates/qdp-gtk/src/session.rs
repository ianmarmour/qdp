//! A session with a host: `qdp connect --events`, which pairs if asked,
//! connects, and shows the stream in a window of its own, reporting its
//! progress as lines.

use std::cell::Cell;
use std::ffi::OsStr;
use std::path::Path;
use std::rc::Rc;

use gtk::{gio, glib};

use crate::command;
use crate::rows::{Authentication, Target};
use crate::session_events::SessionEvent;

/// The environment variable `qdp connect` reads a token from, which keeps
/// it out of the command line.
const TOKEN_VARIABLE: &str = "QDP_TOKEN";

/// What a session reports when `qdp` exits without saying how it ended.
const UNEXPLAINED_EXIT: &str = "The qdp command stopped without saying why; its log says more";

/// A running `qdp connect`.
#[derive(Debug)]
pub struct Session {
    /// The process.
    process: gio::Subprocess,
    /// Whether the app cancelled it.
    cancelled: Rc<Cell<bool>>,
}

impl Session {
    /// Starts a session with `target`, as the client whose identity is in
    /// `identity`, telling `report` of its progress. Its last report is
    /// always that it ended or failed.
    pub fn start(
        target: &Target,
        identity: &Path,
        options: &[String],
        report: impl Fn(SessionEvent) + 'static,
    ) -> Result<Self, glib::Error> {
        let mut arguments = vec![
            OsStr::new("connect"),
            OsStr::new(&target.address),
            OsStr::new("--identity"),
            identity.as_os_str(),
            OsStr::new("--events"),
        ];
        let mut environment = Vec::new();
        match &target.authentication {
            Authentication::Paired(pin) => {
                arguments.extend([OsStr::new("--pin"), OsStr::new(pin.as_str())]);
            }
            Authentication::Token { pin, token } => {
                arguments.extend([OsStr::new("--pin"), OsStr::new(pin.as_str())]);
                environment.push((TOKEN_VARIABLE, token.as_str()));
            }
            Authentication::Pair => arguments.push(OsStr::new("--pair")),
        }
        arguments.extend(options.iter().map(OsStr::new));
        let process = command::start_with_environment(&arguments, &environment)?;
        let cancelled = Rc::new(Cell::new(false));
        let finished = Rc::new(Cell::new(false));
        let report = Rc::new(report);
        let (line_report, line_finished) = (Rc::clone(&report), Rc::clone(&finished));
        let exit_cancelled = Rc::clone(&cancelled);
        command::read_lines(
            &process,
            move |line| {
                if let Some(event) = SessionEvent::parse(line) {
                    if matches!(event, SessionEvent::Ended | SessionEvent::Failed(_)) {
                        line_finished.set(true);
                    }
                    line_report(event);
                }
            },
            move || {
                if !finished.get() {
                    report(if exit_cancelled.get() {
                        SessionEvent::Ended
                    } else {
                        SessionEvent::Failed(UNEXPLAINED_EXIT.to_owned())
                    });
                }
            },
        );
        Ok(Self { process, cancelled })
    }

    /// Answers a pairing host's request for its code.
    pub fn send_code(&self, code: &str) {
        command::write_line(&self.process, code);
    }

    /// Ends the session, as when the user declines to pair.
    pub fn cancel(&self) {
        self.cancelled.set(true);
        self.process.force_exit();
    }
}
