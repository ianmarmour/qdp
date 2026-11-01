//! `film_grain_params()` (AV1 specification, Section 5.9.30): the grain a
//! decoder adds to the frames it shows.

use crate::CodecError;
use crate::bits::BitReader;
use crate::sequence::SequenceHeader;

use super::header::{FrameHeader, FrameType};
use super::{NUM_REF_FRAMES, ReferenceSlot};

/// The most luma scaling points.
pub(crate) const MAX_Y_POINTS: usize = 14;

/// The most scaling points of each chroma plane.
pub(crate) const MAX_CHROMA_POINTS: usize = 10;

/// The most luma auto-regression coefficients: `2 * lag * (lag + 1)` at the
/// longest lag, 3.
pub(crate) const MAX_Y_COEFFS: usize = 24;

/// The most auto-regression coefficients of each chroma plane: one more than
/// luma's, for the luma value.
pub(crate) const MAX_CHROMA_COEFFS: usize = MAX_Y_COEFFS + 1;

/// A scaling function's point: a sample value and its scaling.
pub(crate) type ScalingPoint = [u8; 2];

/// The film grain of a frame, all zero when it has none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FilmGrain {
    /// `apply_grain`.
    pub(crate) apply_grain: bool,
    /// `grain_seed`.
    pub(crate) grain_seed: u16,
    /// `update_grain`: whether the parameters were coded rather than loaded
    /// from a reference.
    pub(crate) update_grain: bool,
    /// `point_y_value` and `point_y_scaling`.
    pub(crate) y_points: Vec<ScalingPoint>,
    /// `chroma_scaling_from_luma`.
    pub(crate) chroma_scaling_from_luma: bool,
    /// `point_cb_value` and `point_cb_scaling`.
    pub(crate) cb_points: Vec<ScalingPoint>,
    /// `point_cr_value` and `point_cr_scaling`.
    pub(crate) cr_points: Vec<ScalingPoint>,
    /// `grain_scaling_minus_8`.
    pub(crate) scaling_shift_minus_8: u8,
    /// `ar_coeff_lag`.
    pub(crate) ar_coeff_lag: u8,
    /// `ar_coeffs_y_plus_128`.
    pub(crate) ar_coeffs_y: [u8; MAX_Y_COEFFS],
    /// `ar_coeffs_cb_plus_128`.
    pub(crate) ar_coeffs_cb: [u8; MAX_CHROMA_COEFFS],
    /// `ar_coeffs_cr_plus_128`.
    pub(crate) ar_coeffs_cr: [u8; MAX_CHROMA_COEFFS],
    /// `ar_coeff_shift_minus_6`.
    pub(crate) ar_coeff_shift_minus_6: u8,
    /// `grain_scale_shift`.
    pub(crate) grain_scale_shift: u8,
    /// `cb_mult`.
    pub(crate) cb_mult: u8,
    /// `cb_luma_mult`.
    pub(crate) cb_luma_mult: u8,
    /// `cb_offset`.
    pub(crate) cb_offset: u16,
    /// `cr_mult`.
    pub(crate) cr_mult: u8,
    /// `cr_luma_mult`.
    pub(crate) cr_luma_mult: u8,
    /// `cr_offset`.
    pub(crate) cr_offset: u16,
    /// `overlap_flag`.
    pub(crate) overlap: bool,
    /// `clip_to_restricted_range`.
    pub(crate) clip_to_restricted_range: bool,
}

/// Reads `film_grain_params()` for `header`, loading the parameters a frame
/// reuses from `refs`.
pub(crate) fn film_grain_params(
    reader: &mut BitReader<'_>,
    sequence: &SequenceHeader,
    header: &FrameHeader,
    refs: &[ReferenceSlot; NUM_REF_FRAMES],
) -> Result<FilmGrain, CodecError> {
    if !sequence.film_grain || (!header.show_frame && !header.showable_frame) {
        return Ok(FilmGrain::default());
    }
    if !reader.bit()? {
        return Ok(FilmGrain::default());
    }
    let grain_seed = reader.bits(16)? as u16;
    let update_grain = header.frame_type != FrameType::Inter || reader.bit()?;
    if !update_grain {
        let slot = reader.bits(3)? as usize;
        if !header.ref_frame_idx.contains(&(slot as u8)) {
            return Err(CodecError::MalformedBitstream(
                "film_grain_params_ref_idx names no reference",
            ));
        }
        // load_grain_params(), keeping the frame's own seed.
        return Ok(FilmGrain {
            grain_seed,
            ..refs[slot].film_grain.clone()
        });
    }
    let mut grain = FilmGrain {
        apply_grain: true,
        grain_seed,
        update_grain,
        ..FilmGrain::default()
    };
    grain.y_points = scaling_points(reader, MAX_Y_POINTS)?;
    grain.chroma_scaling_from_luma = !sequence.monochrome && reader.bit()?;
    let no_chroma_points = sequence.monochrome
        || grain.chroma_scaling_from_luma
        || (sequence.subsampling_x && sequence.subsampling_y && grain.y_points.is_empty());
    if !no_chroma_points {
        grain.cb_points = scaling_points(reader, MAX_CHROMA_POINTS)?;
        grain.cr_points = scaling_points(reader, MAX_CHROMA_POINTS)?;
    }
    grain.scaling_shift_minus_8 = reader.bits(2)? as u8;
    grain.ar_coeff_lag = reader.bits(2)? as u8;
    let lag = usize::from(grain.ar_coeff_lag);
    let luma_positions = 2 * lag * (lag + 1);
    let chroma_positions = if grain.y_points.is_empty() {
        luma_positions
    } else {
        for coefficient in &mut grain.ar_coeffs_y[..luma_positions] {
            *coefficient = reader.bits(8)? as u8;
        }
        luma_positions + 1
    };
    if grain.chroma_scaling_from_luma || !grain.cb_points.is_empty() {
        for coefficient in &mut grain.ar_coeffs_cb[..chroma_positions] {
            *coefficient = reader.bits(8)? as u8;
        }
    }
    if grain.chroma_scaling_from_luma || !grain.cr_points.is_empty() {
        for coefficient in &mut grain.ar_coeffs_cr[..chroma_positions] {
            *coefficient = reader.bits(8)? as u8;
        }
    }
    grain.ar_coeff_shift_minus_6 = reader.bits(2)? as u8;
    grain.grain_scale_shift = reader.bits(2)? as u8;
    if !grain.cb_points.is_empty() {
        grain.cb_mult = reader.bits(8)? as u8;
        grain.cb_luma_mult = reader.bits(8)? as u8;
        grain.cb_offset = reader.bits(9)? as u16;
    }
    if !grain.cr_points.is_empty() {
        grain.cr_mult = reader.bits(8)? as u8;
        grain.cr_luma_mult = reader.bits(8)? as u8;
        grain.cr_offset = reader.bits(9)? as u16;
    }
    grain.overlap = reader.bit()?;
    grain.clip_to_restricted_range = reader.bit()?;
    Ok(grain)
}

/// Reads a scaling function: a count of at most `max` points, then each
/// point's value and scaling.
fn scaling_points(reader: &mut BitReader<'_>, max: usize) -> Result<Vec<ScalingPoint>, CodecError> {
    let count = reader.bits(4)? as usize;
    if count > max {
        return Err(CodecError::MalformedBitstream("too many film grain points"));
    }
    (0..count)
        .map(|_| Ok([reader.bits(8)? as u8, reader.bits(8)? as u8]))
        .collect()
}
