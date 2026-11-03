//! Choosing between the platform's hardware codecs and the software ones.

use tracing::{info, warn};

#[cfg(target_os = "linux")]
use crate::DmaBufFrame;
use crate::decoder::SOFTWARE_DECODER;
use crate::{
    Av1Decoder, Av1Encoder, CodecError, DecodedPicture, EncodedFrame, EncoderSettings,
    KeyFrameRequest, Yuv420Frame,
};

/// An AV1 encoder producing temporal units in the form QDP sends.
pub trait VideoEncoder {
    /// Passes one frame to the encoder and returns the frames whose encoding
    /// has completed, oldest first.
    fn encode(
        &mut self,
        frame: &Yuv420Frame,
        request: KeyFrameRequest,
    ) -> Result<Vec<EncodedFrame>, CodecError>;

    /// Returns whether [`VideoEncoder::encode_dma_buf`] can encode frames
    /// the GPU holds.
    #[cfg(target_os = "linux")]
    fn takes_dma_bufs(&self) -> bool {
        false
    }

    /// Passes one frame the GPU holds to the encoder, scaling it to the
    /// encoder's size, and returns the frames whose encoding has completed,
    /// oldest first. Encoders that cannot read such frames, and frames they
    /// cannot read, fail with [`CodecError::DmaBufUnsupported`], after
    /// which the caller encodes frames in memory instead.
    #[cfg(target_os = "linux")]
    fn encode_dma_buf(
        &mut self,
        frame: &DmaBufFrame,
        request: KeyFrameRequest,
    ) -> Result<Vec<EncodedFrame>, CodecError> {
        let _ = (frame, request);
        Err(CodecError::DmaBufUnsupported(format!(
            "{} reads frames in memory only",
            self.description()
        )))
    }

    /// Sets the quantizer, from 0 (best) to 255, of the frames encoded from
    /// now on, as rate control asks (Section 5.5). An encoder that cannot
    /// change it at every frame takes it when it can.
    fn set_quantizer(&mut self, quantizer: u8);

    /// Returns which encoder this is, for logs.
    fn description(&self) -> &str;
}

/// An AV1 decoder for QDP streams, in which each temporal unit holds one shown
/// frame and so gives one picture.
pub trait VideoDecoder: Send {
    /// Decodes one temporal unit into memory.
    fn decode(&mut self, temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError>;

    /// Decodes one temporal unit, leaving the picture where the decoder
    /// put it: in GPU memory, for a hardware decoder whose pictures a
    /// window can draw from there, or in memory.
    fn decode_picture(&mut self, temporal_unit: &[u8]) -> Result<DecodedPicture, CodecError> {
        self.decode(temporal_unit).map(DecodedPicture::Memory)
    }

    /// Returns which decoder this is, for logs.
    fn description(&self) -> &str;
}

/// Which kind of encoder or decoder to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CodecPreference {
    /// The platform's hardware encoder if it has one that works, otherwise
    /// the software encoder.
    #[default]
    Auto,
    /// The platform's hardware encoder, failing without one.
    Hardware,
    /// The software codec: rav1e to encode, rav1d to decode.
    Software,
}

/// Opens an encoder as `preference` asks.
///
/// With [`CodecPreference::Auto`], a hardware encoder that cannot be opened
/// is logged and replaced by the software encoder.
pub fn open_encoder(
    settings: EncoderSettings,
    preference: CodecPreference,
) -> Result<Box<dyn VideoEncoder>, CodecError> {
    if preference != CodecPreference::Software {
        match open_hardware_encoder(settings) {
            Ok(encoder) => return Ok(encoder),
            Err(error) if preference == CodecPreference::Auto => {
                info!(%error, "no hardware AV1 encoder; encoding in software");
            }
            Err(error) => return Err(error),
        }
    }
    Ok(Box::new(Av1Encoder::new(settings)?))
}

/// The size of the encoder opened to find whether it takes frames the GPU
/// holds: small, and one every hardware encoder supports.
#[cfg(target_os = "linux")]
const PROBE_SIZE: (usize, usize) = (1280, 720);

/// Returns whether the encoder `preference` opens can encode frames the GPU
/// holds, which a capture can then hand over as DMA-BUFs rather than copy
/// into memory. The software encoder cannot; whether the hardware encoder
/// can is found once, by opening it.
#[cfg(target_os = "linux")]
pub fn encoder_takes_dma_bufs(preference: CodecPreference) -> bool {
    /// Whether the hardware encoder takes them, once found.
    static HARDWARE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if preference == CodecPreference::Software {
        return false;
    }
    *HARDWARE.get_or_init(|| {
        let settings = EncoderSettings {
            width: PROBE_SIZE.0,
            height: PROBE_SIZE.1,
            frame_rate: 60,
            speed: crate::FASTEST_SPEED,
            quantizer: 100,
            threads: 1,
        };
        match open_hardware_encoder(settings) {
            Ok(encoder) => encoder.takes_dma_bufs(),
            Err(error) => {
                tracing::debug!(%error, "no hardware encoder to take frames the GPU holds");
                false
            }
        }
    })
}

/// Opens a decoder as `preference` asks, using `threads` threads for software
/// decoding, or 0 to choose automatically.
///
/// With [`CodecPreference::Auto`], software decoding takes over if the
/// platform has no hardware decoder, or if the hardware decoder later finds it
/// cannot handle the stream.
pub fn open_decoder(
    threads: u32,
    preference: CodecPreference,
) -> Result<Box<dyn VideoDecoder>, CodecError> {
    match preference {
        CodecPreference::Software => Ok(Box::new(Av1Decoder::new(threads)?)),
        CodecPreference::Hardware => open_hardware_decoder(),
        CodecPreference::Auto => match open_hardware_decoder() {
            Ok(hardware) => Ok(Box::new(FallbackDecoder {
                hardware: Some(hardware),
                software: None,
                threads,
            })),
            Err(error) => {
                info!(%error, "no hardware AV1 decoder; decoding in software");
                Ok(Box::new(Av1Decoder::new(threads)?))
            }
        },
    }
}

/// A hardware decoder that hands over to software for good once it reports
/// that it cannot handle the stream.
struct FallbackDecoder {
    /// The hardware decoder, until it gives up.
    hardware: Option<Box<dyn VideoDecoder>>,
    /// The software decoder, once needed.
    software: Option<Av1Decoder>,
    /// The software decoder's thread count.
    threads: u32,
}

impl VideoDecoder for FallbackDecoder {
    fn decode(&mut self, temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
        self.decode_picture(temporal_unit)?.into_memory()
    }

    fn decode_picture(&mut self, temporal_unit: &[u8]) -> Result<DecodedPicture, CodecError> {
        if let Some(hardware) = &mut self.hardware {
            match hardware.decode_picture(temporal_unit) {
                Err(CodecError::HardwareUnavailable(reason)) => {
                    warn!(reason, "decoding in software instead of hardware");
                    self.hardware = None;
                }
                result => return result,
            }
        }
        let software = match &mut self.software {
            Some(software) => software,
            None => self.software.insert(Av1Decoder::new(self.threads)?),
        };
        software.decode(temporal_unit).map(DecodedPicture::Memory)
    }

    fn description(&self) -> &str {
        self.hardware
            .as_ref()
            .map_or(SOFTWARE_DECODER, |hardware| hardware.description())
    }
}

/// Opens the platform's hardware decoder: VideoToolbox on macOS and iOS.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn open_hardware_decoder() -> Result<Box<dyn VideoDecoder>, CodecError> {
    Ok(Box::new(crate::videotoolbox::VideoToolboxDecoder::new()?))
}

/// Opens the platform's hardware decoder: MediaCodec on Android, rendering
/// into the app's video surface.
#[cfg(target_os = "android")]
fn open_hardware_decoder() -> Result<Box<dyn VideoDecoder>, CodecError> {
    Ok(Box::new(crate::MediaCodecDecoder::new()?))
}

/// Opens the platform's hardware decoder: D3D11 video on Windows.
#[cfg(windows)]
fn open_hardware_decoder() -> Result<Box<dyn VideoDecoder>, CodecError> {
    Ok(Box::new(crate::D3d11Decoder::new()?))
}

/// Opens the platform's hardware decoder: VA-API on Linux.
#[cfg(target_os = "linux")]
fn open_hardware_decoder() -> Result<Box<dyn VideoDecoder>, CodecError> {
    Ok(Box::new(crate::VaapiDecoder::new()?))
}

/// Opens the platform's hardware decoder; this platform has none yet.
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "android",
    windows,
    target_os = "linux"
)))]
fn open_hardware_decoder() -> Result<Box<dyn VideoDecoder>, CodecError> {
    Err(CodecError::HardwareUnavailable(
        "hardware decoding is not supported on this platform yet".to_owned(),
    ))
}

/// Opens the platform's hardware encoder: VA-API on Linux.
#[cfg(target_os = "linux")]
fn open_hardware_encoder(settings: EncoderSettings) -> Result<Box<dyn VideoEncoder>, CodecError> {
    Ok(Box::new(crate::VaapiEncoder::new(settings)?))
}

/// Opens the platform's hardware encoder: on Windows, NVENC on NVIDIA GPUs
/// that encode AV1, and otherwise the AV1 encoder a GPU's driver registers
/// with Media Foundation, as AMD's and Intel's do.
#[cfg(windows)]
fn open_hardware_encoder(settings: EncoderSettings) -> Result<Box<dyn VideoEncoder>, CodecError> {
    let nvenc = match crate::nvenc::NvencEncoder::new(settings) {
        Ok(encoder) => return Ok(Box::new(encoder)),
        Err(error) => error,
    };
    match crate::mediafoundation::MediaFoundationEncoder::new(settings) {
        Ok(encoder) => Ok(Box::new(encoder)),
        Err(error) => Err(CodecError::HardwareUnavailable(format!(
            "NVENC: {nvenc}; Media Foundation: {error}"
        ))),
    }
}

/// Opens the platform's hardware encoder; this platform has none yet.
#[cfg(not(any(target_os = "linux", windows)))]
fn open_hardware_encoder(_settings: EncoderSettings) -> Result<Box<dyn VideoEncoder>, CodecError> {
    Err(CodecError::Encoder(
        "hardware encoding is not supported on this platform yet".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings for a small stream.
    fn settings() -> EncoderSettings {
        EncoderSettings {
            width: 128,
            height: 64,
            frame_rate: 60,
            speed: 10,
            quantizer: 100,
            threads: 1,
        }
    }

    /// A decoder that gives up on the stream.
    struct GivesUp;

    impl VideoDecoder for GivesUp {
        fn decode(&mut self, _temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
            Err(CodecError::HardwareUnavailable("test".to_owned()))
        }

        fn description(&self) -> &str {
            "gives up"
        }
    }

    /// The fallback decoder decodes in software once hardware gives up, and
    /// decoders of every preference decode a key frame.
    #[test]
    fn falls_back_to_software() {
        let mut encoder = open_encoder(settings(), CodecPreference::Software).unwrap();
        let frame = Yuv420Frame::black(128, 64);
        let key = encoder
            .encode(&frame, KeyFrameRequest::Force)
            .unwrap()
            .remove(0);
        let mut fallback = FallbackDecoder {
            hardware: Some(Box::new(GivesUp)),
            software: None,
            threads: 1,
        };
        assert_eq!(fallback.description(), "gives up");
        let decoded = fallback.decode(&key.temporal_unit).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (128, 64));
        assert_eq!(fallback.description(), SOFTWARE_DECODER);
        for preference in [CodecPreference::Auto, CodecPreference::Software] {
            let mut decoder = open_decoder(1, preference).unwrap();
            assert_eq!(decoder.decode(&key.temporal_unit).unwrap().width(), 128);
        }
    }

    /// Asking for software gives rav1e, and asking for either always gives an
    /// encoder that works, whatever the machine has.
    #[test]
    fn opens_an_encoder() {
        let software = open_encoder(settings(), CodecPreference::Software).unwrap();
        assert_eq!(software.description(), "rav1e (software)");
        let mut any = open_encoder(settings(), CodecPreference::Auto).unwrap();
        let frame = Yuv420Frame::black(128, 64);
        let encoded = any.encode(&frame, KeyFrameRequest::Force).unwrap();
        assert!(encoded.iter().any(|frame| frame.is_key_frame));
    }
}
