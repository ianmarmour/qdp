//! `qdp connect --events`: the session's progress as the lines the apps'
//! cores report (see [`qdp_app::events`]), on standard output, for an app
//! that runs `qdp connect` for its stream window, as the Linux app does. What
//! the app answers is read from standard input: the code a pairing host
//! shows, a line for each `pair` line; a gateway's user name and password,
//! two lines for each `signin` line; and a TOTP code, a line for each `totp`
//! line. The log stays on standard error.

use std::io::{BufRead, Write};

use qdp_app::events::{CodeRequest, Ending, Event};
use qdp_runtime::{Credentials, SignInAnswer, SignInPrompt, SignInRequest};

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
        read_answer()
    }

    /// Returns the prompt that asks the app what a gateway's sign-in needs.
    pub fn sign_in_prompt(self) -> SignInPrompt {
        SignInPrompt::new(move |request| {
            Box::pin(async move {
                tokio::task::spawn_blocking(move || self.ask_sign_in(&request))
                    .await
                    .ok()
                    .flatten()
            })
        })
    }

    /// Asks the app what `request` asks for, and returns its answer, or
    /// `None` once its input closes.
    fn ask_sign_in(self, request: &SignInRequest) -> Option<SignInAnswer> {
        let credentials = matches!(request, SignInRequest::Credentials { .. });
        self.send(&Event::sign_in(request.clone()));
        if credentials {
            let user = read_answer()?;
            let password = read_raw_answer()?;
            Some(SignInAnswer::Credentials(Credentials { user, password }))
        } else {
            read_answer().map(SignInAnswer::Code)
        }
    }
}

/// Reads the app's next answer, trimmed, or `None` once its input closes.
fn read_answer() -> Option<String> {
    read_raw_answer().map(|answer| answer.trim().to_owned())
}

/// Reads the app's next answer as it is, but for its line ending, as a
/// password, whose spaces count, or `None` once its input closes.
fn read_raw_answer() -> Option<String> {
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_owned()),
    }
}
