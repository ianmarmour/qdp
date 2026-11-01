//! What `qdp connect --events` reports as a session progresses, a line at a
//! time: the lines of the qdp-app crate's `events` module the app acts on.
//! The others, as the video's size and statistics, are for the stream
//! window, which `qdp connect` draws itself.

use crate::key_pin::KeyPin;

/// Why a code is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRequest {
    /// The host shows a code for the first time.
    First,
    /// The last code did not work, and the host shows a new one.
    Retry,
}

/// Something a session reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    /// Connecting to the host.
    Connecting,
    /// Connected; the stream is starting.
    Connected,
    /// The connection was lost, and is being made again.
    Reconnecting {
        /// Why it was lost.
        error: String,
    },
    /// Pairing needs the code the host shows, which the app writes back.
    PairingCode(CodeRequest),
    /// Pairing succeeded with the host whose key this is; `qdp` keeps it.
    Paired(KeyPin),
    /// The session ended normally.
    Ended,
    /// The session failed.
    Failed(String),
}

impl SessionEvent {
    /// Reads `line`, or returns `None` for a line the app has no use for.
    pub fn parse(line: &str) -> Option<Self> {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            "connecting" => Some(Self::Connecting),
            "connected" => Some(Self::Connected),
            "reconnecting" => {
                // The delay before the next attempt comes first.
                let (_, error) = rest.split_once(' ').unwrap_or((rest, ""));
                Some(Self::Reconnecting {
                    error: error.to_owned(),
                })
            }
            "pair" => match rest {
                "first" => Some(Self::PairingCode(CodeRequest::First)),
                "retry" => Some(Self::PairingCode(CodeRequest::Retry)),
                _ => None,
            },
            "paired" => KeyPin::parse(rest).map(Self::Paired),
            "ended" => Some(Self::Ended),
            "failed" => Some(Self::Failed(rest.to_owned())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_lines_the_app_acts_on() {
        let pin = "ab".repeat(32);
        let cases = [
            ("connecting", SessionEvent::Connecting),
            ("connected", SessionEvent::Connected),
            (
                "reconnecting 500 timed out",
                SessionEvent::Reconnecting {
                    error: "timed out".to_owned(),
                },
            ),
            ("pair first", SessionEvent::PairingCode(CodeRequest::First)),
            ("pair retry", SessionEvent::PairingCode(CodeRequest::Retry)),
            (
                &format!("paired {pin}"),
                SessionEvent::Paired(KeyPin::parse(&pin).unwrap()),
            ),
            ("ended", SessionEvent::Ended),
            (
                "failed closed by peer",
                SessionEvent::Failed("closed by peer".to_owned()),
            ),
        ];
        for (line, event) in cases {
            assert_eq!(SessionEvent::parse(line), Some(event), "{line}");
        }
    }

    #[test]
    fn skips_the_stream_windows_lines() {
        for line in [
            "video 1920 1080",
            "stats 120 4.2 1.3 - - - - -",
            "cursor",
            "pair maybe",
            "paired 12",
        ] {
            assert_eq!(SessionEvent::parse(line), None, "{line}");
        }
    }
}
