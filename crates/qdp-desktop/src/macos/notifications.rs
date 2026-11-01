//! Notifications on macOS, in Notification Center, which a program without
//! an app bundle, as `qdp serve` is, reaches through `osascript`'s `display
//! notification`. Notification Center shows each as the user's settings
//! say, as a banner that goes away or an alert that stays, and keeps it
//! until the user clears it: notifications cannot be taken away early.

use std::process::{Command, Stdio};
use std::thread;

/// The program that runs AppleScript, which every Mac has.
const OSASCRIPT: &str = "/usr/bin/osascript";

/// A notification shown in Notification Center.
#[derive(Debug)]
pub(crate) struct Notification;

impl Notification {
    /// Does nothing: Notification Center keeps the notification until the
    /// user clears it.
    pub(crate) fn close(self) {}
}

/// Returns `text` as an AppleScript string literal.
fn quoted(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Returns the AppleScript that shows a notification titled `summary`
/// saying `body`.
fn script(summary: &str, body: &str) -> String {
    format!(
        "display notification {} with title {}",
        quoted(body),
        quoted(summary)
    )
}

/// Shows a notification titled `summary` saying `body` in Notification
/// Center.
///
/// # Errors
///
/// Fails if `osascript` cannot be started.
pub(crate) fn notify(summary: &str, body: &str) -> Result<Notification, String> {
    let mut child = Command::new(OSASCRIPT)
        .arg("-e")
        .arg(script(summary, body))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot run {OSASCRIPT}: {error}"))?;
    // Reaped on a thread of its own, so that showing never waits for it.
    thread::spawn(move || child.wait());
    Ok(Notification)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_what_it_shows() {
        assert_eq!(
            script("Pair \"Phone\"?", r"Type 1234 on C:\Phone."),
            r#"display notification "Type 1234 on C:\\Phone." with title "Pair \"Phone\"?""#
        );
    }
}
