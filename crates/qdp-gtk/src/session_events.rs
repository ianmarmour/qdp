//! What `qdp connect --events` reports as a session progresses, a line at a
//! time: the lines of the qdp-app crate's `events` module the app acts on.
//! The others, as the video's size and statistics, are for the stream
//! window, which `qdp connect` draws itself.

use crate::key_pin::KeyPin;

/// Whether something is asked for the first time, or again after an answer
/// that did not work: a pairing code, or a gateway's credentials or code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeRequest {
    /// The first time: for a pairing code, the host shows one.
    First,
    /// Again: for a pairing code, the host shows a new one.
    Retry,
}

impl CodeRequest {
    /// Reads a line's `first` or `retry`.
    fn parse(word: &str) -> Option<Self> {
        match word {
            "first" => Some(Self::First),
            "retry" => Some(Self::Retry),
            _ => None,
        }
    }
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
    /// A gateway needs the user's name and password, which the app writes
    /// back.
    GatewaySignIn {
        /// Whether an earlier attempt failed.
        request: CodeRequest,
        /// The gateway's name for users.
        gateway: String,
    },
    /// A gateway needs a code from the user's authenticator app, which the
    /// app writes back.
    GatewayCode {
        /// Whether an earlier code did not work.
        request: CodeRequest,
        /// The `otpauth://` URI to show as a QR code first, for an app that
        /// does not have the gateway's secret yet.
        enrollment: Option<String>,
        /// The gateway's name for users.
        gateway: String,
    },
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
            "pair" => CodeRequest::parse(rest).map(Self::PairingCode),
            "signin" => {
                let (request, gateway) = rest.split_once(' ')?;
                Some(Self::GatewaySignIn {
                    request: CodeRequest::parse(request)?,
                    gateway: gateway.to_owned(),
                })
            }
            "totp" => {
                let mut fields = rest.splitn(3, ' ');
                let request = CodeRequest::parse(fields.next()?)?;
                let enrollment = match fields.next()? {
                    "-" => None,
                    uri => Some(uri.to_owned()),
                };
                Some(Self::GatewayCode {
                    request,
                    enrollment,
                    gateway: fields.next()?.to_owned(),
                })
            }
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
            (
                "signin retry Example Corp",
                SessionEvent::GatewaySignIn {
                    request: CodeRequest::Retry,
                    gateway: "Example Corp".to_owned(),
                },
            ),
            (
                "totp first otpauth://totp/Example%20Corp:alice?secret=AB Example Corp",
                SessionEvent::GatewayCode {
                    request: CodeRequest::First,
                    enrollment: Some("otpauth://totp/Example%20Corp:alice?secret=AB".to_owned()),
                    gateway: "Example Corp".to_owned(),
                },
            ),
            (
                "totp retry - Example Corp",
                SessionEvent::GatewayCode {
                    request: CodeRequest::Retry,
                    enrollment: None,
                    gateway: "Example Corp".to_owned(),
                },
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
            "signin later Example Corp",
            "totp first",
        ] {
            assert_eq!(SessionEvent::parse(line), None, "{line}");
        }
    }
}
