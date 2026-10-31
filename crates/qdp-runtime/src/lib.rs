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

mod audio;
mod audio_receiver;
mod auth;
mod client;
mod client_cursor;
mod client_input;
mod client_passthrough;
mod client_touch;
mod clipboard;
mod encode_thread;
mod host;
mod host_cursor;
mod host_input;
mod host_output;
mod host_passthrough;
mod media;
mod send_stats;

use qdp::auth::scram::ScramError;
use qdp::session::UsageError;
use qdp::video::SendFrameError;
use qdp_codec::CodecError;
use qdp_quic::TransportError;
use thiserror::Error;

pub use audio::{
    ActiveDevice, AudioInput, AudioOutput, CaptureSink, ClientAudio, HostAudio, Playout,
    SAMPLE_RATE,
};
pub use auth::{ClientCredentials, HostAuthenticator};
pub use client::{ClientOptions, ClientSummary, DisplaySize, run};
pub use client_input::{
    GamepadFeedback, InputHandle, InputQueue, PassthroughFeedback, PenInput, UserInput,
    input_channel,
};
pub use clipboard::{Clipboard, ClipboardDirections, SharedClipboard};
pub use host::{EncoderTuning, HostOptions, HostSummary, serve};
pub use host_input::{InputAction, InputInjector, NoInput, PointerButton, TouchPhase, TouchPoint};
pub use host_passthrough::{FeatureRequest, PassthroughDevice, PassthroughPolicy};
pub use media::{
    CursorShape, CursorState, CursorWarp, FrameInfo, FrameSink, FrameSource, SinkControl,
    TestPattern, WarpKind, changed_blocks,
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
    /// The system clipboard could not be shared.
    #[error("clipboard failed: {0}")]
    Clipboard(String),
    /// An audio stream could not be started.
    #[error("audio failed: {0}")]
    Audio(String),
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
