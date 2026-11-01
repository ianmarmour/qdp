//! `global_motion_params()` (AV1 specification, Section 5.9.24): each
//! reference's warp, coded as a difference from the primary reference's,
//! and whether the warp's shear is valid (Section 7.11.3.6).

use crate::CodecError;
use crate::bits::BitReader;

use super::NUM_REF_FRAMES;

/// `WARPEDMODEL_PREC_BITS`: the fractional bits of warp parameters.
const WARPEDMODEL_PREC_BITS: u32 = 16;

/// `GM_ABS_TRANS_BITS`.
const GM_ABS_TRANS_BITS: u32 = 12;

/// `GM_TRANS_PREC_BITS`.
const GM_TRANS_PREC_BITS: u32 = 6;

/// `GM_ABS_TRANS_ONLY_BITS`.
const GM_ABS_TRANS_ONLY_BITS: u32 = 9;

/// `GM_TRANS_ONLY_PREC_BITS`.
const GM_TRANS_ONLY_PREC_BITS: u32 = 3;

/// `GM_ABS_ALPHA_BITS`.
const GM_ABS_ALPHA_BITS: u32 = 12;

/// `GM_ALPHA_PREC_BITS`.
const GM_ALPHA_PREC_BITS: u32 = 15;

/// `WARP_PARAM_REDUCE_BITS`.
const WARP_PARAM_REDUCE_BITS: u32 = 6;

/// `DIV_LUT_BITS`.
const DIV_LUT_BITS: u32 = 8;

/// `DIV_LUT_PREC_BITS`.
const DIV_LUT_PREC_BITS: u32 = 14;

/// `DIV_LUT_NUM`: the entries of [`DIV_LUT`].
const DIV_LUT_NUM: usize = (1 << DIV_LUT_BITS) + 1;

/// The `k` that `decode_subexp()` starts from.
const SUBEXP_START_BITS: u32 = 3;

/// `Div_Lut`: `2^14` divided by `1 + i / 256`, rounded, for each `i`, as
/// the specification tabulates it.
const DIV_LUT: [u16; DIV_LUT_NUM] = {
    let mut table = [0; DIV_LUT_NUM];
    let mut index = 0;
    while index < DIV_LUT_NUM {
        let divisor = (1 << DIV_LUT_BITS) + index as u32;
        let dividend = 1u32 << (DIV_LUT_BITS + DIV_LUT_PREC_BITS);
        table[index] = ((dividend + divisor / 2) / divisor) as u16;
        index += 1;
    }
    table
};

/// The identity warp: `gm_params` of a reference without global motion.
const IDENTITY: [i32; 6] = [
    0,
    0,
    1 << WARPEDMODEL_PREC_BITS,
    0,
    0,
    1 << WARPEDMODEL_PREC_BITS,
];

/// `PrevGmParams` when no reference provides them: the identity warp for
/// every reference.
pub(crate) const DEFAULT_PARAMS: [[i32; 6]; NUM_REF_FRAMES] = [IDENTITY; NUM_REF_FRAMES];

/// `GmType`: the kind of a reference's warp, each kind having the
/// parameters of those before it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum WarpModel {
    /// `IDENTITY`.
    #[default]
    Identity = 0,
    /// `TRANSLATION`.
    Translation = 1,
    /// `ROTZOOM`.
    RotZoom = 2,
    /// `AFFINE`.
    Affine = 3,
}

/// A reference's global motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GlobalMotion {
    /// `GmType`.
    pub(crate) model: WarpModel,
    /// `gm_params`.
    pub(crate) params: [i32; 6],
    /// Whether the warp's shear is invalid, so that blocks may not use it.
    pub(crate) invalid: bool,
}

impl Default for GlobalMotion {
    fn default() -> Self {
        Self {
            model: WarpModel::Identity,
            params: IDENTITY,
            invalid: false,
        }
    }
}

/// Reads `global_motion_params()`, each parameter coded relative to the
/// primary reference's, `previous`.
pub(crate) fn global_motion_params(
    reader: &mut BitReader<'_>,
    frame_is_intra: bool,
    allow_high_precision_mv: bool,
    previous: &[[i32; 6]; NUM_REF_FRAMES],
) -> Result<[GlobalMotion; NUM_REF_FRAMES], CodecError> {
    let mut motion = [GlobalMotion::default(); NUM_REF_FRAMES];
    if frame_is_intra {
        return Ok(motion);
    }
    for (reference, motion) in motion.iter_mut().enumerate().skip(1) {
        let model = if !reader.bit()? {
            WarpModel::Identity
        } else if reader.bit()? {
            WarpModel::RotZoom
        } else if reader.bit()? {
            WarpModel::Translation
        } else {
            WarpModel::Affine
        };
        motion.model = model;
        let previous = &previous[reference];
        let mut read = |index: usize| {
            read_global_param(
                reader,
                model,
                allow_high_precision_mv,
                previous[index],
                index,
            )
        };
        if model >= WarpModel::RotZoom {
            motion.params[2] = read(2)?;
            motion.params[3] = read(3)?;
            if model == WarpModel::Affine {
                motion.params[4] = read(4)?;
                motion.params[5] = read(5)?;
            } else {
                motion.params[4] = -motion.params[3];
                motion.params[5] = motion.params[2];
            }
        }
        if model >= WarpModel::Translation {
            motion.params[0] = read(0)?;
            motion.params[1] = read(1)?;
        }
        motion.invalid = !shear_is_valid(&motion.params);
    }
    Ok(motion)
}

/// Reads `read_global_param()`: parameter `index` of a warp of `model`,
/// coded relative to `previous`.
fn read_global_param(
    reader: &mut BitReader<'_>,
    model: WarpModel,
    allow_high_precision_mv: bool,
    previous: i32,
    index: usize,
) -> Result<i32, CodecError> {
    let (abs_bits, prec_bits) = if index >= 2 {
        (GM_ABS_ALPHA_BITS, GM_ALPHA_PREC_BITS)
    } else if model == WarpModel::Translation {
        let low_precision = u32::from(!allow_high_precision_mv);
        (
            GM_ABS_TRANS_ONLY_BITS - low_precision,
            GM_TRANS_ONLY_PREC_BITS - low_precision,
        )
    } else {
        (GM_ABS_TRANS_BITS, GM_TRANS_PREC_BITS)
    };
    let prec_diff = WARPEDMODEL_PREC_BITS - prec_bits;
    let (round, sub) = if index % 3 == 2 {
        (1 << WARPEDMODEL_PREC_BITS, 1 << prec_bits)
    } else {
        (0, 0)
    };
    let mx = 1i32 << abs_bits;
    let reference = (previous >> prec_diff) - sub;
    let value = decode_signed_subexp_with_ref(reader, -mx, mx + 1, reference)?;
    Ok((value << prec_diff) + round)
}

/// `decode_signed_subexp_with_ref()`: a value in `low..high` coded relative
/// to `reference`.
fn decode_signed_subexp_with_ref(
    reader: &mut BitReader<'_>,
    low: i32,
    high: i32,
    reference: i32,
) -> Result<i32, CodecError> {
    let value = decode_unsigned_subexp_with_ref(reader, (high - low) as u32, reference - low)?;
    Ok(value as i32 + low)
}

/// `decode_unsigned_subexp_with_ref()`: a value below `mx` coded relative to
/// `reference`.
fn decode_unsigned_subexp_with_ref(
    reader: &mut BitReader<'_>,
    mx: u32,
    reference: i32,
) -> Result<u32, CodecError> {
    let value = decode_subexp(reader, mx)?;
    let reference = reference.clamp(0, mx as i32 - 1) as u32;
    Ok(if (reference << 1) <= mx {
        inverse_recenter(reference, value)
    } else {
        mx - 1 - inverse_recenter(mx - 1 - reference, value)
    })
}

/// `decode_subexp()`: a value below `symbols` in a sub-exponential code.
fn decode_subexp(reader: &mut BitReader<'_>, symbols: u32) -> Result<u32, CodecError> {
    let mut index = 0;
    let mut mk = 0;
    loop {
        let bits = if index == 0 {
            SUBEXP_START_BITS
        } else {
            SUBEXP_START_BITS + index - 1
        };
        let a = 1 << bits;
        if symbols <= mk + 3 * a {
            return Ok(reader.non_symmetric(symbols - mk)? + mk);
        }
        if !reader.bit()? {
            return Ok(reader.bits32(bits)? + mk);
        }
        index += 1;
        mk += a;
    }
}

/// `inverse_recenter()`: undoes the folding of values around `r`.
fn inverse_recenter(r: u32, v: u32) -> u32 {
    if v > 2 * r {
        v
    } else if v & 1 == 1 {
        r - v.div_ceil(2)
    } else {
        r + v / 2
    }
}

/// `Round2Signed(x, n)`.
fn round2_signed(x: i64, n: u32) -> i64 {
    if n == 0 {
        return x;
    }
    let rounding = 1i64 << (n - 1);
    if x >= 0 {
        (x + rounding) >> n
    } else {
        -((-x + rounding) >> n)
    }
}

/// `resolve_divisor()`: `1 / d` as a factor and a shift.
fn resolve_divisor(d: i64) -> (i64, u32) {
    let magnitude = d.unsigned_abs();
    let n = 63 - magnitude.leading_zeros();
    let e = magnitude - (1 << n);
    let f = if n > DIV_LUT_BITS {
        round2_signed(e as i64, n - DIV_LUT_BITS)
    } else {
        (e << (DIV_LUT_BITS - n)) as i64
    };
    let factor = i64::from(DIV_LUT[f as usize]);
    (if d < 0 { -factor } else { factor }, n + DIV_LUT_PREC_BITS)
}

/// Returns whether a warp's shear is valid: the setup shear process's
/// `warpValid` (Section 7.11.3.6).
fn shear_is_valid(params: &[i32; 6]) -> bool {
    if params[2] <= 0 {
        return false;
    }
    let clip = |value: i64| value.clamp(i64::from(i16::MIN), i64::from(i16::MAX));
    let one = 1i64 << WARPEDMODEL_PREC_BITS;
    let alpha0 = clip(i64::from(params[2]) - one);
    let beta0 = clip(i64::from(params[3]));
    let (factor, shift) = resolve_divisor(i64::from(params[2]));
    let v = i64::from(params[4]) << WARPEDMODEL_PREC_BITS;
    let gamma0 = clip(round2_signed(v * factor, shift));
    let w = i64::from(params[3]) * i64::from(params[4]);
    let delta0 = clip(i64::from(params[5]) - round2_signed(w * factor, shift) - one);
    let reduce =
        |value: i64| round2_signed(value, WARP_PARAM_REDUCE_BITS) << WARP_PARAM_REDUCE_BITS;
    let (alpha, beta, gamma, delta) = (
        reduce(alpha0),
        reduce(beta0),
        reduce(gamma0),
        reduce(delta0),
    );
    4 * alpha.abs() + 7 * beta.abs() < one && 4 * gamma.abs() + 4 * delta.abs() < one
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The computed table matches the specification's at its ends and in
    /// between.
    #[test]
    fn computes_the_division_table() {
        assert_eq!(
            DIV_LUT[..8],
            [16384, 16320, 16257, 16194, 16132, 16070, 16009, 15948]
        );
        assert_eq!(DIV_LUT[128], 10923);
        assert_eq!(DIV_LUT[250..], [8289, 8273, 8257, 8240, 8224, 8208, 8192]);
    }

    /// `inverse_recenter()` alternates around the reference, then counts on.
    #[test]
    fn recenters_values() {
        let unfolded: Vec<u32> = (0..8).map(|v| inverse_recenter(2, v)).collect();
        assert_eq!(unfolded, [2, 1, 3, 0, 4, 5, 6, 7]);
    }

    /// The identity warp and small rotations are valid; a shear too strong
    /// to warp with is not.
    #[test]
    fn checks_shear() {
        assert!(shear_is_valid(&IDENTITY));
        let one = 1 << WARPEDMODEL_PREC_BITS;
        assert!(shear_is_valid(&[0, 0, one, 1000, -1000, one]));
        assert!(!shear_is_valid(&[0, 0, one, one / 4, 0, one]));
        assert!(!shear_is_valid(&[0, 0, 0, 0, 0, one]));
    }

    /// A reference without global motion takes one bit, and leaves the
    /// identity warp; intra frames read nothing.
    #[test]
    fn reads_identity_motion() {
        let bytes = [0u8; 1];
        let mut reader = BitReader::new(&bytes);
        let motion = global_motion_params(&mut reader, false, true, &DEFAULT_PARAMS).unwrap();
        assert_eq!(reader.position(), 7);
        assert!(
            motion
                .iter()
                .all(|motion| *motion == GlobalMotion::default())
        );
        let mut reader = BitReader::new(&bytes);
        global_motion_params(&mut reader, true, true, &DEFAULT_PARAMS).unwrap();
        assert_eq!(reader.position(), 0);
    }

    /// A translation coded as no change from the reference keeps the
    /// reference's translation: `decode_subexp` of 0 recentres to it.
    #[test]
    fn reads_translation_relative_to_the_reference() {
        let mut previous = DEFAULT_PARAMS;
        // A translation of 5 in units of 1/8 sample, at 16 fractional bits.
        let precision = WARPEDMODEL_PREC_BITS - GM_TRANS_ONLY_PREC_BITS;
        previous[1][0] = 5 << precision;
        previous[1][1] = -(3 << precision);
        // LAST_FRAME: is_global 1, is_rot_zoom 0, is_translation 1; then
        // two parameters of value 0 (more bits 0 and three zero bits of
        // subexp_bits each, as 1 << 9 symbols exceed the first range);
        // the other six references have none.
        let bits = "101".to_owned() + "0000" + "0000" + "000000";
        let bytes: Vec<u8> = bits
            .as_bytes()
            .chunks(8)
            .map(|chunk| {
                let mut byte = 0u8;
                for (index, bit) in chunk.iter().enumerate() {
                    if *bit == b'1' {
                        byte |= 0x80 >> index;
                    }
                }
                byte
            })
            .collect();
        let mut reader = BitReader::new(&bytes);
        let motion = global_motion_params(&mut reader, false, true, &previous).unwrap();
        assert_eq!(motion[1].model, WarpModel::Translation);
        assert_eq!(motion[1].params[..2], [5 << precision, -(3 << precision)]);
        assert_eq!(reader.position(), bits.len());
    }
}
