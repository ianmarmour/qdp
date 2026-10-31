//! Choosing between the platform's hardware encoder and the software one.

use tracing::info;

use crate::{Av1Encoder, CodecError, EncodedFrame, EncoderSettings, KeyFrameRequest, Yuv420Frame};

/// An AV1 encoder producing temporal units in the form QDP sends.
pub trait VideoEncoder {
    /// Passes one frame to the encoder and returns the frames whose encoding
    /// has completed, oldest first.
    fn encode(
        &mut self,
        frame: &Yuv420Frame,
        request: KeyFrameRequest,
    ) -> Result<Vec<EncodedFrame>, CodecError>;

    /// Returns which encoder this is, for logs.
    fn description(&self) -> &str;
}

/// Which kind of encoder to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum EncoderPreference {
    /// The platform's hardware encoder if it has one that works, otherwise
    /// the software encoder.
    #[default]
    Auto,
    /// The platform's hardware encoder, failing without one.
    Hardware,
    /// The software encoder, rav1e.
    Software,
}

/// Opens an encoder as `preference` asks.
///
/// With [`EncoderPreference::Auto`], a hardware encoder that cannot be opened
/// is logged and replaced by the software encoder.
pub fn open_encoder(
    settings: EncoderSettings,
    preference: EncoderPreference,
) -> Result<Box<dyn VideoEncoder>, CodecError> {
    if preference != EncoderPreference::Software {
        match open_hardware_encoder(settings) {
            Ok(encoder) => return Ok(encoder),
            Err(error) if preference == EncoderPreference::Auto => {
                info!(%error, "no hardware AV1 encoder; encoding in software");
            }
            Err(error) => return Err(error),
        }
    }
    Ok(Box::new(Av1Encoder::new(settings)?))
}

/// Opens the platform's hardware encoder: VA-API on Linux.
#[cfg(target_os = "linux")]
fn open_hardware_encoder(settings: EncoderSettings) -> Result<Box<dyn VideoEncoder>, CodecError> {
    Ok(Box::new(crate::VaapiEncoder::new(settings)?))
}

/// Opens the platform's hardware encoder; this platform has none yet.
#[cfg(not(target_os = "linux"))]
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

    /// Asking for software gives rav1e, and asking for either always gives an
    /// encoder that works, whatever the machine has.
    #[test]
    fn opens_an_encoder() {
        let software = open_encoder(settings(), EncoderPreference::Software).unwrap();
        assert_eq!(software.description(), "rav1e (software)");
        let mut any = open_encoder(settings(), EncoderPreference::Auto).unwrap();
        let frame = Yuv420Frame::black(128, 64);
        let encoded = any.encode(&frame, KeyFrameRequest::Force).unwrap();
        assert!(encoded.iter().any(|frame| frame.is_key_frame));
    }
}
