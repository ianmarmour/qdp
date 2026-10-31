//! Sans-I/O session state machines for the control stream (Section 4).
//!
//! [`ClientSession`] and [`HostSession`] consume the bytes received on the control
//! stream and produce the bytes to send on it, enforcing the session phases, the
//! table of when each message may be sent, pipelining and the handshake timers of
//! Section 4.2. They perform no I/O and read no clock: the caller passes the
//! current time and drives the transport. Authentication decisions and the content
//! of configuration messages are left to the caller, which receives events and
//! answers through methods.

mod client;
mod host;
mod negotiation;

use std::time::Duration;

use thiserror::Error;

pub use client::{ClientEvent, ClientSession};
pub use host::{AuthResponse, HostConfig, HostEvent, HostSession};
pub use negotiation::{ClipboardLimit, Negotiated};

use crate::registry::{ErrorCode, MessageType};
use crate::wire::WireError;

/// How long the host waits for `CLIENT_HELLO` after the QUIC handshake
/// (Section 4.2).
pub const CLIENT_HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the host waits for authentication to succeed after the QUIC handshake
/// (Section 4.2).
pub const AUTHENTICATION_TIMEOUT: Duration = Duration::from_secs(120);

/// How long the host waits for `CLIENT_READY` after a successful `AUTH_RESULT`
/// (Section 4.2).
pub const CLIENT_READY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the client waits for `SESSION_START` after sending `CLIENT_READY`
/// (Section 4.2).
pub const SESSION_START_TIMEOUT: Duration = Duration::from_secs(30);

/// The number of failed authentication attempts after which the host closes the
/// connection (Section 4.4.1).
pub const MAX_FAILED_ATTEMPTS: u32 = 3;

/// The endpoint that sends a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// The endpoint that connects and views the desktop.
    Client,
    /// The endpoint that serves the desktop.
    Host,
}

/// The phase of a session (Section 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    /// Waiting for the peer's hello message.
    Hello,
    /// The client is proving its identity.
    Authenticating,
    /// The client describes its displays, audio and devices.
    Configuring,
    /// Media, input, cursor and clipboard data flow.
    Active,
    /// The connection is closing or closed.
    Closed,
}

/// An error that ends the session: the connection is closed with `code`
/// (Section 3.10).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{code}: {reason}")]
pub struct SessionError {
    /// The QDP error code for the CONNECTION_CLOSE frame.
    pub code: ErrorCode,
    /// A description of the cause, for the reason phrase and logs.
    pub reason: String,
}

impl SessionError {
    /// Creates an error with `code` and `reason`.
    pub fn new(code: ErrorCode, reason: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
        }
    }

    /// Creates a `QDP_PROTOCOL_VIOLATION` error.
    pub fn protocol_violation(reason: impl Into<String>) -> Self {
        Self::new(ErrorCode::PROTOCOL_VIOLATION, reason)
    }
}

impl From<WireError> for SessionError {
    fn from(error: WireError) -> Self {
        Self::new(error.error_code(), error.to_string())
    }
}

/// An error returned when the caller asks a session to do something the
/// specification does not allow at that point. The session is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum UsageError {
    /// The message may not be sent in the current phase.
    #[error("{message_type} may not be sent in the {phase:?} phase")]
    NotAllowed {
        /// The message's type.
        message_type: MessageType,
        /// The session's phase.
        phase: Phase,
    },
    /// The operation does not fit the state of the session.
    #[error("{0}")]
    InvalidState(&'static str),
    /// The message is larger than a control message may be.
    #[error("message exceeds the control message size limit")]
    TooLarge,
}

/// Whether a received or outgoing message is allowed in the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// The message may be processed now.
    Allowed,
    /// The message is allowed only in a later phase that the client may reach by
    /// pipelining; the host processes it once that phase is reached
    /// (Section 4.4.6).
    Pipelined,
    /// The message breaks the rules of Section 4.2.
    Violation,
}

/// When a message may be sent, as in the table of Section 4.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Window {
    /// Only as the first message on the control stream.
    Hello,
    /// In `AUTHENTICATING`.
    Authenticating,
    /// In `CONFIGURING`.
    Configuring,
    /// In `CONFIGURING` or `ACTIVE`.
    ConfiguringOrActive,
    /// In `CONFIGURING` after `CLIENT_READY`, or in `ACTIVE`.
    AfterClientReady,
    /// In `CONFIGURING` after `CLIENT_READY` only.
    BeforeActive,
    /// In `ACTIVE`.
    Active,
}

/// Which endpoints may send a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Senders {
    /// Only the client.
    Client,
    /// Only the host.
    Host,
    /// Either endpoint.
    Both,
}

impl Senders {
    /// Returns whether `role` may send.
    fn includes(self, role: Role) -> bool {
        matches!(
            (self, role),
            (Self::Both, _) | (Self::Client, Role::Client) | (Self::Host, Role::Host)
        )
    }
}

/// Returns who may send `message_type` and when, or `None` for unknown types,
/// which are always ignored.
fn rule(message_type: MessageType) -> Option<(Senders, Window)> {
    let rule = match message_type {
        MessageType::CLIENT_HELLO => (Senders::Client, Window::Hello),
        MessageType::HOST_HELLO => (Senders::Host, Window::Hello),
        MessageType::AUTH_START => (Senders::Client, Window::Authenticating),
        MessageType::AUTH_CONTINUE => (Senders::Both, Window::Authenticating),
        MessageType::AUTH_RESULT => (Senders::Host, Window::Authenticating),
        MessageType::DISPLAY_REQUEST
        | MessageType::AUDIO_REQUEST
        | MessageType::KEYBOARD_INFO
        | MessageType::DEVICE_ATTACH
        | MessageType::DEVICE_DETACH
        | MessageType::HID_FEATURE_RESPONSE => (Senders::Client, Window::ConfiguringOrActive),
        MessageType::CLIENT_READY => (Senders::Client, Window::Configuring),
        MessageType::DEVICE_STATUS | MessageType::HID_FEATURE_REQUEST => {
            (Senders::Host, Window::ConfiguringOrActive)
        }
        MessageType::DISPLAY_CONFIG
        | MessageType::AUDIO_CONFIG
        | MessageType::POINTER_MODE
        | MessageType::CURSOR_SET
        | MessageType::CURSOR_WARP => (Senders::Host, Window::AfterClientReady),
        MessageType::SESSION_START => (Senders::Host, Window::BeforeActive),
        MessageType::RESUMPTION_TOKEN => (Senders::Host, Window::Active),
        MessageType::KEYFRAME_REQUEST => (Senders::Client, Window::Active),
        MessageType::CLIPBOARD_OFFER
        | MessageType::CLIPBOARD_REQUEST
        | MessageType::CLIPBOARD_REJECT => (Senders::Both, Window::Active),
        _ => return None,
    };
    Some(rule)
}

/// Decides whether `sender` may send `message_type` in `phase`, given whether
/// `CLIENT_READY` has been sent in the current configuration.
///
/// The caller enforces that `CLIENT_READY` and `SESSION_START` are sent exactly
/// once, and handles the hello messages directly.
fn check(sender: Role, phase: Phase, client_ready: bool, message_type: MessageType) -> Verdict {
    let Some((senders, window)) = rule(message_type) else {
        return Verdict::Allowed;
    };
    if !senders.includes(sender) {
        return Verdict::Violation;
    }
    let allowed = match window {
        Window::Hello => false,
        Window::Authenticating => phase == Phase::Authenticating,
        Window::Configuring => phase == Phase::Configuring,
        Window::ConfiguringOrActive => matches!(phase, Phase::Configuring | Phase::Active),
        Window::AfterClientReady => {
            (phase == Phase::Configuring && client_ready) || phase == Phase::Active
        }
        Window::BeforeActive => phase == Phase::Configuring && client_ready,
        Window::Active => phase == Phase::Active,
    };
    let reachable_by_pipelining = sender == Role::Client
        && phase == Phase::Authenticating
        && matches!(window, Window::Configuring | Window::ConfiguringOrActive);
    if allowed {
        Verdict::Allowed
    } else if reachable_by_pipelining {
        Verdict::Pipelined
    } else {
        Verdict::Violation
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::VarInt;

    /// Spot checks of the table of Section 4.2.
    #[test]
    fn message_table() {
        use Phase::*;
        let cases = [
            (
                Role::Client,
                Authenticating,
                false,
                MessageType::AUTH_START,
                Verdict::Allowed,
            ),
            (
                Role::Host,
                Authenticating,
                false,
                MessageType::AUTH_START,
                Verdict::Violation,
            ),
            (
                Role::Client,
                Configuring,
                false,
                MessageType::AUTH_START,
                Verdict::Violation,
            ),
            (
                Role::Client,
                Authenticating,
                false,
                MessageType::DISPLAY_REQUEST,
                Verdict::Pipelined,
            ),
            (
                Role::Client,
                Authenticating,
                false,
                MessageType::CLIENT_READY,
                Verdict::Pipelined,
            ),
            (
                Role::Client,
                Configuring,
                false,
                MessageType::CLIENT_READY,
                Verdict::Allowed,
            ),
            (
                Role::Client,
                Active,
                true,
                MessageType::CLIENT_READY,
                Verdict::Violation,
            ),
            (
                Role::Host,
                Configuring,
                false,
                MessageType::DISPLAY_CONFIG,
                Verdict::Violation,
            ),
            (
                Role::Host,
                Configuring,
                true,
                MessageType::DISPLAY_CONFIG,
                Verdict::Allowed,
            ),
            (
                Role::Host,
                Active,
                true,
                MessageType::CURSOR_WARP,
                Verdict::Allowed,
            ),
            (
                Role::Host,
                Configuring,
                false,
                MessageType::DEVICE_STATUS,
                Verdict::Allowed,
            ),
            (
                Role::Host,
                Configuring,
                true,
                MessageType::SESSION_START,
                Verdict::Allowed,
            ),
            (
                Role::Host,
                Active,
                true,
                MessageType::SESSION_START,
                Verdict::Violation,
            ),
            (
                Role::Host,
                Configuring,
                true,
                MessageType::RESUMPTION_TOKEN,
                Verdict::Violation,
            ),
            (
                Role::Client,
                Configuring,
                true,
                MessageType::KEYFRAME_REQUEST,
                Verdict::Violation,
            ),
            (
                Role::Client,
                Active,
                true,
                MessageType::KEYFRAME_REQUEST,
                Verdict::Allowed,
            ),
            (
                Role::Host,
                Active,
                true,
                MessageType::CLIPBOARD_OFFER,
                Verdict::Allowed,
            ),
            (
                Role::Client,
                Authenticating,
                false,
                MessageType::CLIPBOARD_OFFER,
                Verdict::Violation,
            ),
            (
                Role::Client,
                Hello,
                false,
                MessageType(VarInt::from_u32(0x21)),
                Verdict::Allowed,
            ),
        ];
        for (sender, phase, ready, message_type, verdict) in cases {
            assert_eq!(
                check(sender, phase, ready, message_type),
                verdict,
                "{sender:?} {phase:?} {ready} {message_type}"
            );
        }
    }
}
