//! Host and client media pipelines for the Quick Desktop Protocol (QDP).
//!
//! This crate ties the protocol logic of `qdp`, the codecs of `qdp-codec` and the
//! transport of `qdp-quic` together into working endpoints:
//!
//! - [`serve`] runs a host session: it authenticates the client with a
//!   [`HostAuthenticator`], configures one monitor, and streams frames from a
//!   [`FrameSource`], encoding them, sending their fragments, answering
//!   retransmission requests and recovering from loss with key frames. The
//!   pointer the source reports goes to the client as cursor images, shapes
//!   and warps, and the desktop's audio as Opus or PCM through a
//!   [`HostAudio`] input.
//! - [`run`] runs a client session: it authenticates with
//!   [`ClientCredentials`], configures its display and input devices, and decodes
//!   the host's video into a [`FrameSink`], sending feedback and synchronizing
//!   clocks. The sink also draws the host's cursor, and [`ClientAudio`]
//!   devices play the host's audio through an adaptive jitter buffer and send
//!   the user's microphone.
//!
//! [`TestPattern`] is a synthetic source for hosts without a screen capture
//! backend, and for tests.

mod admission;
mod audio;
mod audio_receiver;
mod auth;
mod client;
mod client_cursor;
mod client_display;
mod client_input;
mod client_passthrough;
mod client_touch;
mod clipboard;
mod decode_thread;
mod encode_thread;
mod host;
mod host_cursor;
mod host_display;
mod host_input;
mod host_output;
mod host_passthrough;
mod layout;
mod media;
mod rate_control;
mod send_stats;
mod stream_format;
mod stream_ids;

use qdp::auth::scram::ScramError;
use qdp::registry::ErrorCode;
use qdp::session::UsageError;
use qdp::video::SendFrameError;
use qdp_codec::CodecError;
use qdp_quic::TransportError;
use thiserror::Error;

pub use admission::{Admission, AdmissionLimits, AdmissionTicket, Refusal};
pub use audio::{
    ActiveDevice, AudioInput, AudioOutput, CaptureSink, ClientAudio, HostAudio, Microphone,
    Playout, SAMPLE_RATE,
};
pub use auth::{ClientCredentials, HostAuthenticator};
pub use client::{ClientOptions, ClientSummary, DisplaySize, run};
pub use client_input::{
    GamepadFeedback, InputHandle, InputQueue, PassthroughFeedback, PenInput, UserInput,
    input_channel,
};
pub use clipboard::{Clipboard, ClipboardDirections, Focus, SharedClipboard};
pub use host::{EncoderTuning, HostOptions, HostSummary, serve};
pub use host_input::{InputAction, InputInjector, NoInput, PointerButton, TouchPhase, TouchPoint};
pub use host_passthrough::{FeatureRequest, PassthroughDevice, PassthroughPolicy};
pub use media::{
    CursorShape, CursorState, CursorWarp, DisplayTiming, FrameInfo, FrameSink, FrameSource,
    PointerPosition, Presentation, PresentationTiming, RefreshMode, SinkControl, TestPattern,
    WarpKind, changed_blocks,
};

/// An error that ends a session.
#[derive(Debug, Error)]
pub enum RuntimeError {
    /// The transport or the session state machine failed.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// The session refused an operation; this indicates a bug.
    #[error(transparent)]
    Usage(#[from] UsageError),
    /// Encoding or decoding failed.
    #[error(transparent)]
    Codec(#[from] CodecError),
    /// A frame could not be fragmented.
    #[error(transparent)]
    SendFrame(#[from] SendFrameError),
    /// A SCRAM exchange failed.
    #[error(transparent)]
    Scram(#[from] ScramError),
    /// The host offered no method the client has credentials for.
    #[error("the host offers no authentication method the client can use")]
    NoCommonAuthMethod,
    /// The host sent an authentication step the method does not have.
    #[error("unexpected authentication step")]
    UnexpectedAuthStep,
    /// The host rejected the client's credentials.
    #[error("authentication failed: {0}")]
    AuthenticationFailed(String),
    /// A user name cannot be prepared with RFC 8265.
    #[error("the user name contains disallowed characters")]
    InvalidUsername,
    /// The system's random number generator failed.
    #[error("the random number generator failed")]
    Random,
    /// The frame source failed.
    #[error("capture failed: {0}")]
    Capture(String),
    /// The client's decoding stopped.
    #[error("decoding failed: {0}")]
    Decode(String),
    /// The system clipboard could not be shared.
    #[error("clipboard failed: {0}")]
    Clipboard(String),
    /// An audio stream could not be started.
    #[error("audio failed: {0}")]
    Audio(String),
    /// The session cannot run as configured.
    #[error("invalid configuration: {0}")]
    Config(&'static str),
    /// The peer broke a rule of the protocol.
    #[error("protocol violation: {0}")]
    ProtocolViolation(String),
}

impl RuntimeError {
    /// Returns the QDP error code a session that fails with this error closes
    /// its connection with (Section 3.10).
    pub fn close_code(&self) -> ErrorCode {
        match self {
            Self::Transport(TransportError::Session(error)) => error.code,
            Self::Scram(_)
            | Self::NoCommonAuthMethod
            | Self::UnexpectedAuthStep
            | Self::AuthenticationFailed(_)
            | Self::InvalidUsername => ErrorCode::AUTH_FAILED,
            Self::ProtocolViolation(_) => ErrorCode::PROTOCOL_VIOLATION,
            _ => ErrorCode::INTERNAL_ERROR,
        }
    }
}

/// Closes a session's connection with the code of the error that ended it,
/// unless the session ended normally, so that the peer learns why rather
/// than seeing the connection dropped with `QDP_NO_ERROR`.
pub(crate) fn close_on_error<T>(
    connection: &quinn::Connection,
    result: Result<T, RuntimeError>,
) -> Result<T, RuntimeError> {
    if let Err(error) = &result {
        let normal =
            matches!(error, RuntimeError::Transport(transport) if is_normal_close(transport));
        if !normal {
            let code = error.close_code();
            connection.close(
                quinn::VarInt::from_u64(code.0.into_inner()).unwrap_or(quinn::VarInt::MAX),
                error.to_string().as_bytes(),
            );
        }
    }
    result
}

/// Returns whether `error` is the peer closing the connection normally, with
/// `QDP_NO_ERROR`.
pub(crate) fn is_normal_close(error: &TransportError) -> bool {
    let connection_error = match error {
        TransportError::Connection(error) => error,
        TransportError::Read(quinn::ReadError::ConnectionLost(error))
        | TransportError::Write(quinn::WriteError::ConnectionLost(error)) => error,
        _ => return false,
    };
    matches!(
        connection_error,
        quinn::ConnectionError::ApplicationClosed(close) if close.error_code == quinn::VarInt::from_u32(0)
    ) || matches!(connection_error, quinn::ConnectionError::LocallyClosed)
}
