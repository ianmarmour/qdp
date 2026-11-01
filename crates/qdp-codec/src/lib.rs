//! AV1 encoding and decoding for the Quick Desktop Protocol (QDP).
//!
//! QDP streams are AV1 only (Section 6.4). This crate provides:
//!
//! - [`open_encoder`], which opens the platform's hardware encoder when it has
//!   one ([`VaapiEncoder`] on Linux) and falls back to software otherwise.
//! - [`Av1Encoder`], the software encoder every platform can fall back on,
//!   built on rav1e in low-latency mode.
//! - [`open_decoder`], which opens the platform's hardware decoder when it has
//!   one (VideoToolbox on macOS) and falls back to software otherwise.
//! - [`Av1Decoder`], which decodes with rav1d, returning one picture per
//!   temporal unit.
//! - [`ProcessDecoder`], which decodes in a separate process running
//!   [`serve_decoder_on_stdio`], isolating the decoder from the client.
//! - [`parse_obus`] and [`to_qdp_temporal_unit`], which check and convert AV1
//!   bitstreams.
//!
//! Every encoder produces temporal units in the form QDP sends: no temporal
//! delimiters, a sequence header on every key frame, the minimum tile counts
//! of Section 6.4.4 and the colour description of standard dynamic range
//! streams.

mod audio;
mod backend;
// Only the VA-API encoder writes headers itself.
#[cfg(target_os = "linux")]
mod bits;
mod decoder;
#[cfg(target_os = "linux")]
mod dma_buf;
mod encoder;
mod frame;
#[cfg(target_os = "linux")]
mod headers;
mod level;
mod obu;
mod picture;
mod process_decoder;
mod sequence;
#[cfg(target_os = "linux")]
mod vaapi;
#[cfg(target_os = "macos")]
mod videotoolbox;

use thiserror::Error;

pub use audio::{
    AudioContent, AudioDecoder, AudioEncoder, AudioFormat, OPUS_MAX_CHANNELS, OPUS_SAMPLE_RATE,
    PCM_MAX_CHANNELS, open_audio_decoder, open_audio_encoder,
};
#[cfg(target_os = "linux")]
pub use backend::encoder_takes_dma_bufs;
pub use backend::{CodecPreference, VideoDecoder, VideoEncoder, open_decoder, open_encoder};
pub use decoder::Av1Decoder;
#[cfg(target_os = "linux")]
pub use dma_buf::{BufferIdentity, DRM_FORMAT_MOD_LINEAR, DmaBufFrame, DmaBufLayout};
pub use encoder::{
    Av1Encoder, EncodedFrame, EncoderSettings, FASTEST_SPEED, KeyFrameRequest, tile_count,
};
pub use frame::{Rgb32Order, Yuv420Frame};
pub use level::{NO_LEVEL, TileCount, frame_rate_within, level_for};
pub use obu::{Obu, ObuType, parse_obus, to_qdp_temporal_unit};
pub use picture::DecodedPicture;
pub use process_decoder::{DecoderCommand, ProcessDecoder, serve_decoder, serve_decoder_on_stdio};
pub use sequence::{ColorDescription, SequenceHeader};
#[cfg(target_os = "linux")]
pub use vaapi::VaapiEncoder;
#[cfg(target_os = "macos")]
pub use videotoolbox::PixelBufferPicture;

/// An error from encoding or decoding.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CodecError {
    /// A bitstream does not meet the requirements of Section 6.4.1.
    #[error("malformed AV1 bitstream: {0}")]
    MalformedBitstream(&'static str),
    /// The encoder rejected its configuration or a frame.
    #[error("encoder error: {0}")]
    Encoder(String),
    /// A frame's size differs from the encoder's.
    #[error("the frame size does not match the encoder")]
    FrameSize,
    /// The decoder reported an error.
    #[error("decoder error in {what}: {code}")]
    Decoder {
        /// The operation that failed.
        what: &'static str,
        /// The negative errno value rav1d returned.
        code: i32,
    },
    /// The temporal unit produced no picture.
    #[error("the temporal unit produced no picture")]
    NoPicture,
    /// The decoded picture is not 8-bit 4:2:0.
    #[error("unsupported picture format")]
    UnsupportedFormat,
    /// The encoder cannot read a frame the GPU holds, so frames must come
    /// in memory instead.
    #[error("the encoder cannot read frames the GPU holds: {0}")]
    DmaBufUnsupported(String),
    /// A hardware codec cannot handle the stream, so software must.
    #[error("the hardware codec cannot be used: {0}")]
    HardwareUnavailable(String),
    /// The decoder process failed, or its decoder reported an error.
    #[error("decoder process: {0}")]
    DecoderProcess(String),
}
