//! Low-latency AV1 encoding with rav1e (Sections 6.4.1, 6.4.2 and 6.4.4).

use rav1e::Context;
use rav1e::color::{
    ChromaSampling, ColorDescription, ColorPrimaries, MatrixCoefficients, PixelRange,
    TransferCharacteristics,
};
use rav1e::config::{Config, EncoderConfig, SpeedSettings};
use rav1e::data::{EncoderStatus, FrameType, Rational};
use rav1e::prelude::{FrameParameters, FrameTypeOverride};

use crate::{CodecError, Yuv420Frame, to_qdp_temporal_unit};

/// The bit depth of the frames this encoder accepts.
const BIT_DEPTH: usize = 8;

/// The size of the AV1 configuration record header that precedes the sequence
/// header OBU in rav1e's container sequence header.
const AV1C_HEADER_LENGTH: usize = 4;

/// The number of luma samples up to which one tile suffices, with two
/// recommended (Section 6.4.4).
const ONE_TILE_SAMPLES: usize = 1920 * 1080;

/// The number of luma samples up to which four tiles suffice (Section 6.4.4).
const FOUR_TILE_SAMPLES: usize = 2560 * 1600;

/// The longest key frame interval rav1e accepts, about four months at 60 frames
/// per second. QDP sends key frames only when a stream starts or a client needs
/// one, so the encoder never chooses one on its own.
const MAX_KEY_FRAME_INTERVAL: u64 = 715_827_882;

/// The fastest rav1e speed preset.
pub const FASTEST_SPEED: u8 = 10;

/// The time base the encoder is configured with. QDP frames carry their own
/// Capture Timestamps, so it only affects rate control.
const TIME_BASE: Rational = Rational { num: 1, den: 60 };

/// Whether the next frame must be a key frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyFrameRequest {
    /// Let the encoder decide; it produces key frames only when asked.
    Auto,
    /// Force a key frame, as recovery from a loss requires.
    Force,
}

/// The settings of an encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderSettings {
    /// The frame width in pixels.
    pub width: usize,
    /// The frame height in pixels.
    pub height: usize,
    /// rav1e's speed preset, from 0 (slowest) to [`FASTEST_SPEED`].
    pub speed: u8,
    /// The quantizer, from 0 (best) to 255.
    pub quantizer: u8,
    /// The number of encoder threads, or 0 to choose automatically.
    pub threads: usize,
}

/// One encoded frame, ready to be sent as a QDP frame payload's temporal unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// The index of the input frame this encodes, counting from 0 in the order
    /// frames were passed to [`Av1Encoder::encode`].
    pub input_index: u64,
    /// The temporal unit, without temporal delimiters, beginning with a
    /// sequence header if it is a key frame.
    pub temporal_unit: Vec<u8>,
    /// Whether the frame is a key frame.
    pub is_key_frame: bool,
}

/// Returns the minimum number of tiles for a frame of `width × height` luma
/// samples (Section 6.4.4), using the recommended two for frames that need one.
pub fn tile_count(width: usize, height: usize) -> usize {
    match width * height {
        samples if samples <= ONE_TILE_SAMPLES => 2,
        samples if samples <= FOUR_TILE_SAMPLES => 4,
        _ => 8,
    }
}

/// A low-latency AV1 encoder producing profile 0, 8-bit 4:2:0 streams.
pub struct Av1Encoder {
    /// The rav1e context.
    context: Context<u8>,
    /// The sequence header OBU, added to key frames that lack one.
    sequence_header: Vec<u8>,
    /// The frame width.
    width: usize,
    /// The frame height.
    height: usize,
}

impl std::fmt::Debug for Av1Encoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Av1Encoder")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Av1Encoder {
    /// Creates an encoder.
    ///
    /// Frames are never reordered and each one is emitted as soon as it is
    /// encoded. Streams signal BT.709 primaries, sRGB transfer and the BT.709
    /// matrix in limited range, as QDP requires for standard dynamic range.
    pub fn new(settings: EncoderSettings) -> Result<Self, CodecError> {
        let mut speed_settings = SpeedSettings::from_preset(settings.speed.min(FASTEST_SPEED));
        speed_settings.rdo_lookahead_frames = 1;
        let encoder = EncoderConfig {
            width: settings.width,
            height: settings.height,
            bit_depth: BIT_DEPTH,
            chroma_sampling: ChromaSampling::Cs420,
            pixel_range: PixelRange::Limited,
            color_description: Some(ColorDescription {
                color_primaries: ColorPrimaries::BT709,
                transfer_characteristics: TransferCharacteristics::SRGB,
                matrix_coefficients: MatrixCoefficients::BT709,
            }),
            time_base: TIME_BASE,
            low_latency: true,
            min_key_frame_interval: 0,
            max_key_frame_interval: MAX_KEY_FRAME_INTERVAL,
            reservoir_frame_delay: None,
            quantizer: usize::from(settings.quantizer),
            bitrate: 0,
            tiles: tile_count(settings.width, settings.height),
            speed_settings,
            ..EncoderConfig::default()
        };
        let context: Context<u8> = Config::new()
            .with_encoder_config(encoder)
            .with_threads(settings.threads)
            .new_context()
            .map_err(|error| CodecError::Encoder(error.to_string()))?;
        let container_header = context.container_sequence_header();
        let sequence_header = container_header
            .get(AV1C_HEADER_LENGTH..)
            .ok_or(CodecError::Encoder("missing sequence header".to_owned()))?
            .to_vec();
        Ok(Self {
            context,
            sequence_header,
            width: settings.width,
            height: settings.height,
        })
    }

    /// Passes one frame to the encoder and returns the frames whose encoding has
    /// completed, oldest first.
    ///
    /// rav1e 0.8 keeps a lookahead even in low-latency mode and returns each
    /// frame about four frames after receiving it, so the frames returned are
    /// earlier ones: [`EncodedFrame::input_index`] says which. That delay is why
    /// hardware encoders are preferred wherever they exist.
    pub fn encode(
        &mut self,
        frame: &Yuv420Frame,
        request: KeyFrameRequest,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        if frame.width() != self.width || frame.height() != self.height {
            return Err(CodecError::FrameSize);
        }
        let mut input = self.context.new_frame();
        input.planes[0].copy_from_raw_u8(frame.y(), frame.width(), 1);
        input.planes[1].copy_from_raw_u8(frame.u(), frame.chroma_width(), 1);
        input.planes[2].copy_from_raw_u8(frame.v(), frame.chroma_width(), 1);
        let parameters = FrameParameters {
            frame_type_override: match request {
                KeyFrameRequest::Auto => FrameTypeOverride::No,
                KeyFrameRequest::Force => FrameTypeOverride::Key,
            },
            ..FrameParameters::default()
        };
        self.context
            .send_frame((input, parameters))
            .map_err(|status| CodecError::Encoder(status.to_string()))?;
        self.receive_packets()
    }

    /// Ends the stream and returns every frame still in the encoder.
    pub fn flush(&mut self) -> Result<Vec<EncodedFrame>, CodecError> {
        self.context.flush();
        self.receive_packets()
    }

    /// Returns every packet the encoder has ready.
    fn receive_packets(&mut self) -> Result<Vec<EncodedFrame>, CodecError> {
        let mut frames = Vec::new();
        loop {
            match self.context.receive_packet() {
                Ok(packet) => {
                    let is_key_frame = packet.frame_type == FrameType::KEY;
                    let temporal_unit =
                        to_qdp_temporal_unit(&packet.data, is_key_frame, &self.sequence_header)?;
                    frames.push(EncodedFrame {
                        input_index: packet.input_frameno,
                        temporal_unit,
                        is_key_frame,
                    });
                }
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::NeedMoreData | EncoderStatus::LimitReached) => {
                    return Ok(frames);
                }
                Err(status) => return Err(CodecError::Encoder(status.to_string())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tile counts follow the table of Section 6.4.4.
    #[test]
    fn tile_counts() {
        assert_eq!(tile_count(1280, 720), 2);
        assert_eq!(tile_count(1920, 1080), 2);
        assert_eq!(tile_count(2560, 1440), 4);
        assert_eq!(tile_count(3840, 2160), 8);
    }
}
