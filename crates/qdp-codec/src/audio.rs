//! Audio codecs (Section 7.5): Opus, through the pure-Rust `rusty-opus`, and
//! PCM L16 and L24.
//!
//! Samples are 32-bit floats from -1 to 1, interleaved in the channel order of
//! RFC 7845's mapping family 1, as QDP orders channels for every codec.

use qdp::message::AudioCodec;
use rusty_opus::{Application, OpusDecoder, OpusEncoder};

use crate::CodecError;

/// The sample rate Opus streams use (Section 7.3).
pub const OPUS_SAMPLE_RATE: u32 = 48_000;

/// The most channels this implementation's Opus streams carry: one stream,
/// mono or stereo.
pub const OPUS_MAX_CHANNELS: u16 = 2;

/// The most channels PCM streams carry (Section 7.3).
pub const PCM_MAX_CHANNELS: u16 = 8;

/// The largest value of a 16-bit sample.
const L16_SCALE: f32 = 32_767.0;

/// The largest value of a 24-bit sample.
const L24_SCALE: f32 = 8_388_607.0;

/// The bytes of one L16 sample.
const L16_BYTES: usize = 2;

/// The bytes of one L24 sample.
const L24_BYTES: usize = 3;

/// The encoder's complexity, from 0 (fastest) to 10 (best).
const OPUS_COMPLEXITY: i32 = 9;

/// What an audio stream carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AudioContent {
    /// General audio such as a desktop's sound, coded for fidelity with the
    /// least delay.
    General,
    /// Speech from a microphone, coded for intelligibility with forward
    /// error correction (Section 7.5.1).
    Speech,
}

/// The parameters of one audio stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioFormat {
    /// The codec.
    pub codec: AudioCodec,
    /// The sample rate in hertz.
    pub sample_rate: u32,
    /// The number of channels.
    pub channels: u16,
    /// The samples per channel in each packet.
    pub frame_samples: usize,
}

impl AudioFormat {
    /// Returns the samples in one packet's frame, across all channels.
    pub const fn frame_len(&self) -> usize {
        self.frame_samples * self.channels as usize
    }

    /// Returns the bytes one PCM packet takes, or `None` for Opus, whose
    /// packets vary.
    pub const fn pcm_packet_bytes(&self) -> Option<usize> {
        match pcm_sample_bytes(self.codec) {
            Some(bytes) => Some(self.frame_len() * bytes),
            None => None,
        }
    }
}

/// Returns the bytes of one sample of a PCM codec, or `None` for others.
const fn pcm_sample_bytes(codec: AudioCodec) -> Option<usize> {
    if codec.0.into_inner() == AudioCodec::PCM_L16.0.into_inner() {
        Some(L16_BYTES)
    } else if codec.0.into_inner() == AudioCodec::PCM_L24.0.into_inner() {
        Some(L24_BYTES)
    } else {
        None
    }
}

/// Encodes audio frames into packets.
pub trait AudioEncoder: Send {
    /// Encodes one frame of [`AudioFormat::frame_len`] samples into a packet
    /// of at most `max_bytes`.
    fn encode(&mut self, frame: &[f32], max_bytes: usize) -> Result<Vec<u8>, CodecError>;

    /// Tells the encoder how many packets the receiver loses, in percent,
    /// so that it can protect them.
    fn set_expected_loss(&mut self, percent: u8);
}

/// Decodes packets into audio frames.
pub trait AudioDecoder: Send {
    /// Decodes a packet, appending one frame of samples to `output`.
    fn decode(&mut self, packet: &[u8], output: &mut Vec<f32>) -> Result<(), CodecError>;

    /// Appends one frame standing in for a lost packet: recovered from the
    /// forward error correction in `next`, the packet after it, if that is
    /// given and carries any, and concealed otherwise.
    fn conceal(&mut self, next: Option<&[u8]>, output: &mut Vec<f32>) -> Result<(), CodecError>;
}

/// Returns an encoder for `format`, tuned for `content`.
pub fn open_audio_encoder(
    format: AudioFormat,
    content: AudioContent,
) -> Result<Box<dyn AudioEncoder>, CodecError> {
    match pcm_sample_bytes(format.codec) {
        Some(_) => Ok(Box::new(PcmCodec { format })),
        None if format.codec == AudioCodec::OPUS => Ok(Box::new(Opus::encoder(format, content)?)),
        None => Err(CodecError::Encoder("unsupported audio codec".to_owned())),
    }
}

/// Returns a decoder for `format`.
pub fn open_audio_decoder(format: AudioFormat) -> Result<Box<dyn AudioDecoder>, CodecError> {
    match pcm_sample_bytes(format.codec) {
        Some(_) => Ok(Box::new(PcmCodec { format })),
        None if format.codec == AudioCodec::OPUS => Ok(Box::new(OpusDecoderState::new(format)?)),
        None => Err(CodecError::Encoder("unsupported audio codec".to_owned())),
    }
}

/// Returns an error for a failure of the Opus codec.
fn opus_error(error: rusty_opus::Error) -> CodecError {
    CodecError::Encoder(format!("Opus: {error:?}"))
}

/// The Opus encoder of one stream.
struct Opus {
    /// The encoder.
    encoder: OpusEncoder,
    /// The stream's format.
    format: AudioFormat,
}

impl Opus {
    /// Returns an encoder for `format`, tuned for `content`.
    fn encoder(format: AudioFormat, content: AudioContent) -> Result<Self, CodecError> {
        if format.sample_rate != OPUS_SAMPLE_RATE
            || !(1..=OPUS_MAX_CHANNELS).contains(&format.channels)
        {
            return Err(CodecError::Encoder("unsupported Opus format".to_owned()));
        }
        let application = match content {
            AudioContent::General => Application::RestrictedLowDelay,
            AudioContent::Speech => Application::Voip,
        };
        let mut encoder = OpusEncoder::new(
            OPUS_SAMPLE_RATE as i32,
            usize::from(format.channels),
            application,
        )
        .map_err(opus_error)?;
        encoder.complexity = OPUS_COMPLEXITY;
        encoder.bitrate_bps = match content {
            AudioContent::General => 64_000 * i32::from(format.channels),
            AudioContent::Speech => 32_000 * i32::from(format.channels),
        };
        encoder.use_inband_fec = content == AudioContent::Speech;
        Ok(Self { encoder, format })
    }
}

impl AudioEncoder for Opus {
    fn encode(&mut self, frame: &[f32], max_bytes: usize) -> Result<Vec<u8>, CodecError> {
        if frame.len() != self.format.frame_len() {
            return Err(CodecError::FrameSize);
        }
        let mut packet = vec![0; max_bytes];
        let length = self
            .encoder
            .encode(frame, self.format.frame_samples, &mut packet)
            .map_err(opus_error)?;
        packet.truncate(length);
        Ok(packet)
    }

    fn set_expected_loss(&mut self, percent: u8) {
        self.encoder.packet_loss_perc = i32::from(percent.min(100));
    }
}

/// The Opus decoder of one stream.
struct OpusDecoderState {
    /// The decoder.
    decoder: OpusDecoder,
    /// The stream's format.
    format: AudioFormat,
}

impl OpusDecoderState {
    /// Returns a decoder for `format`.
    fn new(format: AudioFormat) -> Result<Self, CodecError> {
        if format.sample_rate != OPUS_SAMPLE_RATE
            || !(1..=OPUS_MAX_CHANNELS).contains(&format.channels)
        {
            return Err(CodecError::Encoder("unsupported Opus format".to_owned()));
        }
        let decoder = OpusDecoder::new(OPUS_SAMPLE_RATE as i32, usize::from(format.channels))
            .map_err(opus_error)?;
        Ok(Self { decoder, format })
    }

    /// Appends the samples `decode` writes into a frame buffer.
    fn append(
        &mut self,
        output: &mut Vec<f32>,
        decode: impl FnOnce(&mut OpusDecoder, usize, &mut [f32]) -> Result<usize, rusty_opus::Error>,
    ) -> Result<(), CodecError> {
        let start = output.len();
        output.resize(start + self.format.frame_len(), 0.0);
        let samples = decode(
            &mut self.decoder,
            self.format.frame_samples,
            &mut output[start..],
        )
        .map_err(opus_error)?;
        output.truncate(start + samples * usize::from(self.format.channels));
        Ok(())
    }
}

impl AudioDecoder for OpusDecoderState {
    fn decode(&mut self, packet: &[u8], output: &mut Vec<f32>) -> Result<(), CodecError> {
        if packet.is_empty() {
            return Err(CodecError::Encoder("empty Opus packet".to_owned()));
        }
        self.append(output, |decoder, samples, frame| {
            decoder.decode(packet, samples, frame)
        })
    }

    fn conceal(&mut self, next: Option<&[u8]>, output: &mut Vec<f32>) -> Result<(), CodecError> {
        let next = next.unwrap_or_default();
        self.append(output, |decoder, samples, frame| {
            decoder.decode_fec(next, samples, frame)
        })
    }
}

/// The PCM codec of one stream: signed big-endian samples (Section 7.5.2).
struct PcmCodec {
    /// The stream's format.
    format: AudioFormat,
}

impl AudioEncoder for PcmCodec {
    fn encode(&mut self, frame: &[f32], max_bytes: usize) -> Result<Vec<u8>, CodecError> {
        if frame.len() != self.format.frame_len() {
            return Err(CodecError::FrameSize);
        }
        let bytes = pcm_sample_bytes(self.format.codec).unwrap_or(L16_BYTES);
        if frame.len() * bytes > max_bytes {
            return Err(CodecError::Encoder(
                "the PCM packet is too large".to_owned(),
            ));
        }
        let mut packet = Vec::with_capacity(frame.len() * bytes);
        for &sample in frame {
            let sample = sample.clamp(-1.0, 1.0);
            if bytes == L16_BYTES {
                packet.extend_from_slice(&((sample * L16_SCALE).round() as i16).to_be_bytes());
            } else {
                let value = (sample * L24_SCALE).round() as i32;
                packet.extend_from_slice(&value.to_be_bytes()[1..]);
            }
        }
        Ok(packet)
    }

    fn set_expected_loss(&mut self, _percent: u8) {}
}

impl AudioDecoder for PcmCodec {
    fn decode(&mut self, packet: &[u8], output: &mut Vec<f32>) -> Result<(), CodecError> {
        let bytes = pcm_sample_bytes(self.format.codec).unwrap_or(L16_BYTES);
        if packet.len() != self.format.frame_len() * bytes {
            return Err(CodecError::FrameSize);
        }
        output.extend(packet.chunks_exact(bytes).map(|sample| match sample {
            [high, low] => f32::from(i16::from_be_bytes([*high, *low])) / L16_SCALE,
            [high, middle, low] => {
                // Sign-extend the 24-bit value through the top byte.
                let value = i32::from_be_bytes([*high, *middle, *low, 0]) >> 8;
                value as f32 / L24_SCALE
            }
            _ => 0.0,
        }));
        Ok(())
    }

    fn conceal(&mut self, _next: Option<&[u8]>, output: &mut Vec<f32>) -> Result<(), CodecError> {
        // PCM has no concealment of its own (Section 7.5.2): silence.
        output.resize(output.len() + self.format.frame_len(), 0.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use super::*;

    /// Returns `frames` frames of a stereo tone, interleaved.
    fn tone(format: AudioFormat, frames: usize) -> Vec<f32> {
        (0..format.frame_samples * frames)
            .flat_map(|index| {
                let time = index as f32 / format.sample_rate as f32;
                let left = 0.5 * (TAU * 440.0 * time).sin();
                let right = 0.25 * (TAU * 660.0 * time).sin();
                [left, right].into_iter().take(usize::from(format.channels))
            })
            .collect()
    }

    /// PCM round-trips within one step of its resolution.
    #[test]
    fn pcm_round_trips() {
        for (codec, step) in [
            (AudioCodec::PCM_L16, 1.0 / L16_SCALE),
            (AudioCodec::PCM_L24, 1.0 / L24_SCALE),
        ] {
            let format = AudioFormat {
                codec,
                sample_rate: 48_000,
                channels: 2,
                frame_samples: 240,
            };
            let frame = tone(format, 1);
            let mut encoder = open_audio_encoder(format, AudioContent::General).unwrap();
            let packet = encoder.encode(&frame, 4000).unwrap();
            assert_eq!(Some(packet.len()), format.pcm_packet_bytes());
            let mut decoder = open_audio_decoder(format).unwrap();
            let mut decoded = Vec::new();
            decoder.decode(&packet, &mut decoded).unwrap();
            for (original, round) in frame.iter().zip(&decoded) {
                assert!(
                    (original - round).abs() <= step,
                    "{original} became {round}"
                );
            }
            assert!(
                encoder.encode(&frame, 100).is_err(),
                "too large for the space"
            );
            decoder.conceal(None, &mut decoded).unwrap();
            assert_eq!(decoded.len(), 2 * format.frame_len());
        }
    }

    /// PCM samples are signed and big-endian.
    #[test]
    fn pcm_is_big_endian() {
        let format = AudioFormat {
            codec: AudioCodec::PCM_L24,
            sample_rate: 48_000,
            channels: 1,
            frame_samples: 2,
        };
        let mut encoder = open_audio_encoder(format, AudioContent::General).unwrap();
        let packet = encoder.encode(&[1.0, -1.0], 6).unwrap();
        assert_eq!(packet, [0x7f, 0xff, 0xff, 0x80, 0x00, 0x01]);
    }

    /// Opus frames decode to the tone, after the codec's delay, and a lost
    /// frame is concealed with a whole frame.
    #[test]
    fn opus_round_trips() {
        let format = AudioFormat {
            codec: AudioCodec::OPUS,
            sample_rate: OPUS_SAMPLE_RATE,
            channels: 2,
            frame_samples: 480,
        };
        let input = tone(format, 50);
        let mut encoder = open_audio_encoder(format, AudioContent::General).unwrap();
        let mut decoder = open_audio_decoder(format).unwrap();
        let mut decoded = Vec::new();
        for frame in input.chunks_exact(format.frame_len()) {
            let packet = encoder.encode(frame, 1200).unwrap();
            assert!(packet.len() <= 1200);
            decoder.decode(&packet, &mut decoded).unwrap();
        }
        assert_eq!(decoded.len(), input.len());
        // Compare the energy of the second half: the codec delays the signal
        // by a few milliseconds and alters its waveform.
        let energy = |samples: &[f32]| samples.iter().map(|sample| sample * sample).sum::<f32>();
        let half = input.len() / 2;
        let ratio = energy(&decoded[half..]) / energy(&input[half..]);
        assert!((0.8..1.25).contains(&ratio), "energy ratio {ratio}");
        let before = decoded.len();
        decoder.conceal(None, &mut decoded).unwrap();
        assert_eq!(decoded.len() - before, format.frame_len());
    }

    /// Speech streams carry forward error correction the decoder recovers a
    /// lost frame from.
    #[test]
    fn opus_speech_recovers_from_loss() {
        let format = AudioFormat {
            codec: AudioCodec::OPUS,
            sample_rate: OPUS_SAMPLE_RATE,
            channels: 1,
            frame_samples: 480,
        };
        let input = tone(format, 20);
        let mut encoder = open_audio_encoder(format, AudioContent::Speech).unwrap();
        encoder.set_expected_loss(20);
        let packets: Vec<Vec<u8>> = input
            .chunks_exact(format.frame_len())
            .map(|frame| encoder.encode(frame, 1200).unwrap())
            .collect();
        let mut decoder = open_audio_decoder(format).unwrap();
        let mut decoded = Vec::new();
        for (index, packet) in packets.iter().enumerate() {
            if index == 10 {
                decoder
                    .conceal(packets.get(11).map(Vec::as_slice), &mut decoded)
                    .unwrap();
            } else {
                decoder.decode(packet, &mut decoded).unwrap();
            }
        }
        assert_eq!(decoded.len(), input.len());
        assert!(
            open_audio_encoder(
                AudioFormat {
                    channels: 3,
                    ..format
                },
                AudioContent::Speech
            )
            .is_err()
        );
    }
}
