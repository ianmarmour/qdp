//! `qdp connect --events`: the session's progress as the lines the apps'
//! cores report (see [`qdp_app::events`]), on standard output, for an app
//! that runs `qdp connect` for its stream window, as the Linux app does. The
//! code a pairing host shows is read from standard input, a line for each
//! `pair` line. The log stays on standard error.

use std::io::{BufRead, Write};

use qdp_app::events::{CodeRequest, Ending, Event};

use crate::args::EventOutput;

/// Reports the session's progress as lines, if asked to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventLines {
    /// Whether lines are written.
    output: EventOutput,
}

impl EventLines {
    /// Returns a reporter that writes lines if `output` asks for them.
    pub fn new(output: EventOutput) -> Self {
        Self { output }
    }

    /// Returns whether lines are written.
    pub fn enabled(self) -> bool {
        self.output == EventOutput::Lines
    }

    /// Writes `event`'s line, if lines are asked for.
    pub fn send(self, event: &Event) {
        if !self.enabled() {
            return;
        }
        let mut output = std::io::stdout().lock();
        // An app that stopped reading only misses progress it no longer
        // wants.
        let _ = writeln!(output, "{event}");
        let _ = output.flush();
    }

    /// Writes how the session ended.
    pub fn ended(self, result: &Result<(), String>) {
        let ending = match result {
            Ok(()) => Ending::Normal,
            Err(error) => Ending::Failed(error.clone()),
        };
        self.send(&Event::Ended(ending));
    }

    /// Asks the app for the code the host shows, `request` saying whether
    /// an earlier one did not work, and returns the line it answers, or
    /// `None` once its input closes.
    pub fn ask_code(self, request: CodeRequest) -> Option<String> {
        self.send(&Event::PairingCode(request));
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim().to_owned()),
        }
    }
}
