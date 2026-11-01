//! Checks a hardware encoder's first key frame against what clients take
//! (Section 6.4), so that the host uses an encoder only once its stream is
//! known to suit them: profile 0, 8-bit 4:2:0, the colour description of
//! the other encoders in limited range, a level no higher than the one
//! planned, neither super-resolution nor film grain, one shown key frame,
//! and at least the tiles of Section 6.4.4.

use crate::frame_parser::{FrameParser, FrameType};
use crate::sequence::{ColorDescription, SequenceHeader};
use crate::tile_count;

/// The colour description clients expect, which the encoders write: BT.709
/// primaries and matrix, and the sRGB transfer.
pub(crate) const COLOR_DESCRIPTION: ColorDescription = ColorDescription {
    primaries: 1,
    transfer: 13,
    matrix: 1,
};

/// Checks `temporal_unit`, in the form QDP sends, as the first key frame of
/// a stream of `width × height` frames planned at `level`, returning its
/// sequence header OBU, or what does not suit clients.
pub(crate) fn check_first_key_frame(
    temporal_unit: &[u8],
    width: usize,
    height: usize,
    level: u8,
) -> Result<Vec<u8>, String> {
    let header = SequenceHeader::find(temporal_unit)
        .map_err(|error| format!("its sequence header is malformed: {error}"))?
        .ok_or("its key frame has no sequence header")?;
    let frames = FrameParser::new()
        .parse(temporal_unit)
        .map_err(|error| format!("its key frame is malformed: {error}"))?;
    let tiles = tile_count(width, height);
    let one_key_frame = matches!(
        frames.as_slice(),
        [frame] if frame.header.frame_type == FrameType::Key
            && frame.header.show_frame
            && !frame.header.show_existing_frame
    );
    let checks = [
        ("frames", one_key_frame),
        ("profile", header.profile == 0),
        ("bit depth", header.bit_depth == 8),
        (
            "chroma subsampling",
            !header.monochrome && header.subsampling_x && header.subsampling_y,
        ),
        (
            "color description",
            header.color_description == Some(COLOR_DESCRIPTION),
        ),
        ("color range", !header.full_range),
        ("level", header.level <= level),
        ("super-resolution", !header.superres),
        ("film grain", !header.film_grain),
        (
            "tiles",
            frames.iter().all(|frame| frame.tiles.len() >= tiles),
        ),
    ];
    match checks.into_iter().find(|(_, suits)| !suits) {
        Some((field, _)) => Err(format!("its {field} do not suit clients")),
        None => Ok(header.obu().to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Av1Encoder, EncoderSettings, KeyFrameRequest, TileCount, Yuv420Frame, level_for};

    /// Returns the first key frame rav1e encodes of `width × height`, and
    /// the level it signals.
    fn key_frame(width: usize, height: usize) -> (Vec<u8>, u8) {
        let mut encoder = Av1Encoder::new(EncoderSettings {
            width,
            height,
            frame_rate: 30,
            speed: crate::FASTEST_SPEED,
            quantizer: 120,
            threads: 1,
        })
        .unwrap();
        let frame = Yuv420Frame::black(width, height);
        let encoded = encoder.encode(&frame, KeyFrameRequest::Force).unwrap();
        let unit = encoded.into_iter().next().unwrap().temporal_unit;
        let level = SequenceHeader::find(&unit).unwrap().unwrap().level;
        (unit, level)
    }

    /// The software encoder's key frames suit clients, as they must.
    #[test]
    fn takes_streams_that_suit_clients() {
        let (unit, level) = key_frame(640, 360);
        let header = check_first_key_frame(&unit, 640, 360, level).unwrap();
        assert_eq!(header, SequenceHeader::find(&unit).unwrap().unwrap().obu());
    }

    /// A stream above its planned level, or with too few tiles for frames
    /// of its size, does not.
    #[test]
    fn refuses_streams_that_do_not() {
        let (unit, level) = key_frame(640, 360);
        if let Some(lower) = level.checked_sub(1) {
            assert!(check_first_key_frame(&unit, 640, 360, lower).is_err());
        }
        // Frames as large as 4K need eight tiles; this one has fewer.
        let planned = level_for(3840, 2160, 30, TileCount::at_most_columns(8));
        let error = check_first_key_frame(&unit, 3840, 2160, planned).unwrap_err();
        assert!(error.contains("tiles"), "{error}");
        assert!(check_first_key_frame(&unit[..unit.len() / 2], 640, 360, level).is_err());
    }
}
