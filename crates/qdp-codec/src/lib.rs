//! AV1 software encoding and decoding for the Quick Desktop Protocol (QDP).
//!
//! QDP streams are AV1 only (Section 6.4). This crate provides the software
//! codecs every platform can fall back on:
//!
//! - [`Av1Encoder`] encodes with rav1e in low-latency mode, producing temporal
//!   units in the form QDP sends: no temporal delimiters, a sequence header on
//!   every key frame, the minimum tile counts of Section 6.4.4 and the colour
//!   description of standard dynamic range streams.
//! - [`Av1Decoder`] decodes with rav1d, returning one picture per temporal unit.
//! - [`parse_obus`] and [`to_qdp_temporal_unit`] check and convert AV1 bitstreams.
//!
//! Hardware encoders and decoders reached through operating system interfaces
//! produce and consume the same temporal units.

mod decoder;
mod encoder;
mod frame;
mod obu;

use thiserror::Error;

pub use decoder::Av1Decoder;
pub use encoder::{
    Av1Encoder, EncodedFrame, EncoderSettings, FASTEST_SPEED, KeyFrameRequest, tile_count,
};
pub use frame::Yuv420Frame;
pub use obu::{Obu, ObuType, parse_obus, to_qdp_temporal_unit};

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
}
