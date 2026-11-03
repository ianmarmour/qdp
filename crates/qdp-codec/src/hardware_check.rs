//! The check a hardware decoder passes before it is used: a short stream it
//! must decode exactly as rav1d does. AV1 decoding is exact, so any
//! difference is a fault, here or in the driver, and the stream is better
//! decoded in software.

use tracing::info;

use crate::{
    Av1Decoder, Av1Encoder, CodecError, EncoderSettings, FASTEST_SPEED, KeyFrameRequest,
    VideoDecoder, Yuv420Frame,
};

/// The frames of the stream [`verify`] decodes.
const VERIFY_FRAMES: usize = 4;

/// The size of that stream: wide enough for two tiles.
const VERIFY_SIZE: (usize, usize) = (512, 256);

/// Decodes a short stream with the hardware decoder `open` opens and with
/// rav1d, and returns why the hardware decoder cannot be used if their
/// pictures differ at all.
pub(crate) fn verify<D: VideoDecoder>(
    open: impl FnOnce() -> Result<D, CodecError>,
) -> Result<(), String> {
    let (width, height) = VERIFY_SIZE;
    let mut encoder = Av1Encoder::new(EncoderSettings {
        width,
        height,
        frame_rate: 60,
        speed: FASTEST_SPEED,
        quantizer: 100,
        threads: 1,
    })
    .map_err(|error| error.to_string())?;
    let mut hardware = open().map_err(|error| error.to_string())?;
    let mut software = Av1Decoder::new(1).map_err(|error| error.to_string())?;
    for index in 0..VERIFY_FRAMES {
        let frame = verification_frame(width, height, index);
        let request = if index == 0 {
            KeyFrameRequest::Force
        } else {
            KeyFrameRequest::Auto
        };
        let units = encoder
            .encode(&frame, request)
            .map_err(|error| error.to_string())?;
        for unit in units {
            let expected = software
                .decode(&unit.temporal_unit)
                .map_err(|error| error.to_string())?;
            let decoded = hardware
                .decode(&unit.temporal_unit)
                .map_err(|error| format!("hardware AV1 decoding failed: {error}"))?;
            if decoded != expected {
                let psnr = decoded.luma_psnr(&expected).unwrap_or(0.0);
                return Err(format!(
                    "hardware AV1 decoding differs from rav1d's ({psnr:.1} dB on frame {index})"
                ));
            }
        }
    }
    info!("hardware AV1 decoding matches rav1d's");
    Ok(())
}

/// Returns frame `index` of the verification stream: a scrolling gradient
/// with a moving square and coloured chroma, for prediction and filters to
/// act on.
fn verification_frame(width: usize, height: usize, index: usize) -> Yuv420Frame {
    let square = 32 + 8 * index;
    let y = (0..height)
        .flat_map(|row| (0..width).map(move |column| (row, column)))
        .map(|(row, column)| {
            if (square..square + 64).contains(&row) && (square..square + 64).contains(&column) {
                235
            } else {
                ((column + row + 6 * index) % 200) as u8 + 16
            }
        })
        .collect();
    let chroma = |offset: usize| {
        (0..width * height / 4)
            .map(|i| (64 + (i % (width / 2) + offset + 2 * index) % 128) as u8)
            .collect()
    };
    Yuv420Frame::from_planes(width, height, y, chroma(0), chroma(40))
        .expect("the planes fit the frame")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The verification frames differ from one to the next, so that inter
    /// frames have motion to code.
    #[test]
    fn verification_frames_move() {
        let (width, height) = VERIFY_SIZE;
        let first = verification_frame(width, height, 0);
        let second = verification_frame(width, height, 1);
        assert_eq!((first.width(), first.height()), VERIFY_SIZE);
        assert_ne!(first, second);
    }

    /// A decoder that decodes as rav1d does passes, and one that fails or
    /// differs does not.
    #[test]
    fn passes_only_exact_decoders() {
        assert_eq!(verify(|| Av1Decoder::new(1)), Ok(()));
        let failing = verify(|| -> Result<Av1Decoder, CodecError> {
            Err(CodecError::HardwareUnavailable("no GPU".to_owned()))
        });
        assert!(failing.unwrap_err().contains("no GPU"));
        assert!(verify(|| Ok(Grey)).unwrap_err().contains("differs"));
    }

    /// A decoder whose pictures are all grey.
    struct Grey;

    impl VideoDecoder for Grey {
        fn decode(&mut self, _temporal_unit: &[u8]) -> Result<Yuv420Frame, CodecError> {
            let (width, height) = VERIFY_SIZE;
            Ok(Yuv420Frame::from_planes(
                width,
                height,
                vec![128; width * height],
                vec![128; width * height / 4],
                vec![128; width * height / 4],
            )
            .expect("the planes fit the frame"))
        }

        fn description(&self) -> &str {
            "grey"
        }
    }
}
