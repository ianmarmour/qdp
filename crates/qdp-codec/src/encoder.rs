//! Low-latency AV1 encoding with rav1e (Sections 6.4.1, 6.4.2 and 6.4.4).

use rav1e::Context;
use rav1e::color::{
    ChromaSampling, ColorDescription, ColorPrimaries, MatrixCoefficients, PixelRange,
    TransferCharacteristics,
};
use rav1e::config::{Config, EncoderConfig, SpeedSettings};
use rav1e::data::{EncoderStatus, FrameType, Rational};
use rav1e::prelude::{FrameParameters, FrameTypeOverride, SceneDetectionSpeed};

use crate::{
    CodecError, SequenceHeader, TileCount, VideoEncoder, Yuv420Frame, level_for,
    to_qdp_temporal_unit,
};

/// The bit depth of the frames this encoder accepts.
const BIT_DEPTH: usize = 8;

/// The number of luma samples up to which one tile suffices, with two
/// recommended (Section 6.4.4).
const ONE_TILE_SAMPLES: usize = 1920 * 1080;

/// The number of luma samples up to which four tiles suffice (Section 6.4.4).
const FOUR_TILE_SAMPLES: usize = 2560 * 1600;

/// The longest key frame interval rav1e accepts, about four months at 60 frames
/// per second. QDP sends key frames only when a stream starts or a client needs
/// one, so the encoder never chooses one on its own.
const MAX_KEY_FRAME_INTERVAL: u64 = 715_827_882;

/// How far the wanted quantizer must be from a context's before a new
/// context takes it without waiting for a key frame.
const REOPEN_STEP: u8 = 24;

/// The frames a context encodes before a new one may take a new quantizer
/// without waiting for a key frame: two seconds at 60 frames per second.
const REOPEN_FRAMES: u64 = 120;

/// How [`Av1Encoder`] describes itself.
const DESCRIPTION: &str = "rav1e (software)";

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
    /// The frames per second, which sets the level hardware encoders signal.
    pub frame_rate: u32,
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
    /// The sequence header OBU of the first key frame, added to later key
    /// frames that lack one.
    sequence_header: Vec<u8>,
    /// The settings contexts are made with.
    settings: EncoderSettings,
    /// The quantizer of the current context.
    quantizer: u8,
    /// The quantizer asked for, which a new context takes.
    wanted_quantizer: u8,
    /// The frames passed to the current context.
    frames_in_context: u64,
    /// The frames passed to earlier contexts, which input indices count.
    index_offset: u64,
}

impl std::fmt::Debug for Av1Encoder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Av1Encoder")
            .field("settings", &self.settings)
            .field("quantizer", &self.quantizer)
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
        Ok(Self {
            context: open_context(&settings, settings.quantizer)?,
            sequence_header: Vec::new(),
            settings,
            quantizer: settings.quantizer,
            wanted_quantizer: settings.quantizer,
            frames_in_context: 0,
            index_offset: 0,
        })
    }

    /// Sets the quantizer of the frames encoded from now on.
    ///
    /// rav1e fixes the quantizer when its context is made, so a new context
    /// takes it, whose first frame is a key frame. That happens when a key
    /// frame is due anyway, or when the quantizer moved far from the
    /// context's and the context has encoded for a while.
    pub fn set_quantizer(&mut self, quantizer: u8) {
        self.wanted_quantizer = quantizer;
    }

    /// Returns whether a new context should take the wanted quantizer before
    /// a frame encoded as `request` asks.
    fn should_reopen(&self, request: KeyFrameRequest) -> bool {
        self.wanted_quantizer != self.quantizer
            && (request == KeyFrameRequest::Force
                || (self.wanted_quantizer.abs_diff(self.quantizer) >= REOPEN_STEP
                    && self.frames_in_context >= REOPEN_FRAMES))
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
        if frame.width() != self.settings.width || frame.height() != self.settings.height {
            return Err(CodecError::FrameSize);
        }
        let mut frames = Vec::new();
        if self.should_reopen(request) {
            // The old context's last frames come out first.
            self.context.flush();
            frames = self.receive_packets()?;
            self.context = open_context(&self.settings, self.wanted_quantizer)?;
            self.quantizer = self.wanted_quantizer;
            self.index_offset += self.frames_in_context;
            self.frames_in_context = 0;
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
        self.frames_in_context += 1;
        frames.extend(self.receive_packets()?);
        Ok(frames)
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
                    if is_key_frame {
                        if let Some(header) = SequenceHeader::find(&packet.data)? {
                            self.sequence_header = header.obu().to_vec();
                        }
                        if self.sequence_header.is_empty() {
                            return Err(CodecError::Encoder(
                                "a key frame has no sequence header".to_owned(),
                            ));
                        }
                    }
                    let temporal_unit =
                        to_qdp_temporal_unit(&packet.data, is_key_frame, &self.sequence_header)?;
                    frames.push(EncodedFrame {
                        input_index: self.index_offset + packet.input_frameno,
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

impl VideoEncoder for Av1Encoder {
    fn encode(
        &mut self,
        frame: &Yuv420Frame,
        request: KeyFrameRequest,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        Av1Encoder::encode(self, frame, request)
    }

    fn set_quantizer(&mut self, quantizer: u8) {
        Av1Encoder::set_quantizer(self, quantizer);
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }
}

/// Makes a rav1e context with `settings`, at `quantizer`.
///
/// Frames are never reordered and each one is emitted as soon as it is
/// encoded. Streams signal BT.709 primaries, sRGB transfer and the BT.709
/// matrix in limited range, as QDP requires for standard dynamic range.
fn open_context(settings: &EncoderSettings, quantizer: u8) -> Result<Context<u8>, CodecError> {
    let tiles = tile_count(settings.width, settings.height);
    // rav1e chooses the tile layout itself, so the level allows for the
    // tiles lying in one row.
    let level = level_for(
        settings.width as u32,
        settings.height as u32,
        settings.frame_rate,
        TileCount::at_most_columns(tiles as u32),
    );
    let mut speed_settings = SpeedSettings::from_preset(settings.speed.min(FASTEST_SPEED));
    speed_settings.rdo_lookahead_frames = 0;
    speed_settings.scene_detection_mode = SceneDetectionSpeed::None;
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
        quantizer: usize::from(quantizer),
        bitrate: 0,
        tiles,
        level_idx: Some(level),
        speed_settings,
        ..EncoderConfig::default()
    };
    Config::new()
        .with_encoder_config(encoder)
        .with_threads(settings.threads)
        .new_context()
        .map_err(|error| CodecError::Encoder(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bits of a configuration record's second byte that hold the level.
    const LEVEL_MASK: u8 = 0x1f;

    /// The sequence header parser builds the same configuration record as
    /// rav1e does for its own stream.
    #[test]
    fn configuration_record_matches_rav1e() {
        let mut encoder = Av1Encoder::new(EncoderSettings {
            width: 320,
            height: 192,
            frame_rate: 60,
            speed: FASTEST_SPEED,
            quantizer: 100,
            threads: 1,
        })
        .unwrap();
        let key = Av1Encoder::encode(
            &mut encoder,
            &Yuv420Frame::black(320, 192),
            KeyFrameRequest::Force,
        )
        .unwrap()
        .remove(0);
        let header = SequenceHeader::find(&key.temporal_unit).unwrap().unwrap();
        assert_eq!((header.max_width, header.max_height), (320, 192));
        assert_eq!(header.level, 0);
        let description = header.color_description.unwrap();
        assert_eq!(
            (
                description.primaries,
                description.transfer,
                description.matrix
            ),
            (1, 13, 1)
        );
        assert!(!header.full_range && !header.superres && !header.film_grain);
        // rav1e's record carries only the four fixed bytes, not the OBU, and
        // always says level 31 whatever level the stream signals.
        let record = header.codec_configuration_record();
        let mut expected = encoder.context.container_sequence_header();
        expected[1] = (expected[1] & !LEVEL_MASK) | header.level;
        assert_eq!(record[..4], expected);
        assert_eq!(record[4..], *header.obu());
        assert_eq!(encoder.sequence_header, header.obu());
    }

    /// A new quantizer waits for a key frame or a large change, and the
    /// stream keeps its sequence header and input indices across the new
    /// context.
    #[test]
    fn takes_new_quantizers() {
        let mut encoder = Av1Encoder::new(EncoderSettings {
            width: 64,
            height: 64,
            frame_rate: 60,
            speed: FASTEST_SPEED,
            quantizer: 100,
            threads: 1,
        })
        .unwrap();
        let black = Yuv420Frame::black(64, 64);
        let mut frames = encoder.encode(&black, KeyFrameRequest::Force).unwrap();
        encoder.set_quantizer(110);
        assert!(!encoder.should_reopen(KeyFrameRequest::Auto));
        encoder.set_quantizer(100 + REOPEN_STEP);
        assert!(!encoder.should_reopen(KeyFrameRequest::Auto));
        for _ in 1..REOPEN_FRAMES {
            frames.extend(encoder.encode(&black, KeyFrameRequest::Auto).unwrap());
        }
        assert!(encoder.should_reopen(KeyFrameRequest::Auto));
        frames.extend(encoder.encode(&black, KeyFrameRequest::Auto).unwrap());
        assert_eq!(encoder.quantizer, 100 + REOPEN_STEP);
        frames.extend(encoder.flush().unwrap());
        let indices: Vec<u64> = frames.iter().map(|frame| frame.input_index).collect();
        assert_eq!(indices, (0..=REOPEN_FRAMES).collect::<Vec<_>>());
        let keys: Vec<u64> = frames
            .iter()
            .filter(|frame| frame.is_key_frame)
            .map(|frame| frame.input_index)
            .collect();
        assert_eq!(keys, [0, REOPEN_FRAMES]);
        let headers: Vec<SequenceHeader> = frames
            .iter()
            .filter_map(|frame| SequenceHeader::find(&frame.temporal_unit).unwrap())
            .collect();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].obu(), headers[1].obu());
    }

    /// Tile counts follow the table of Section 6.4.4.
    #[test]
    fn tile_counts() {
        assert_eq!(tile_count(1280, 720), 2);
        assert_eq!(tile_count(1920, 1080), 2);
        assert_eq!(tile_count(2560, 1440), 4);
        assert_eq!(tile_count(3840, 2160), 8);
    }
}
