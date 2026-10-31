//! Variable-length integers (Sections 1.5.1 and 1.5.2).
//!
//! QDP uses the variable-length integer encoding of RFC 9000, Section 16. The two most
//! significant bits of the first byte give the length of the encoding and the
//! remaining bits hold the value in network byte order. Signed values are mapped to
//! unsigned ones with zigzag encoding before they are encoded.

use std::ops::{BitAnd, BitOr};

use thiserror::Error;

/// The largest value a one-byte encoding can hold.
const MAX_ONE_BYTE: u64 = (1 << 6) - 1;

/// The largest value a two-byte encoding can hold.
const MAX_TWO_BYTES: u64 = (1 << 14) - 1;

/// The largest value a four-byte encoding can hold.
const MAX_FOUR_BYTES: u64 = (1 << 30) - 1;

/// The largest value an eight-byte encoding can hold.
const MAX_EIGHT_BYTES: u64 = (1 << 62) - 1;

/// The shift that moves the two length bits of the first byte to the bottom.
const LENGTH_PREFIX_SHIFT: u32 = 6;

/// An error returned when a value is outside the range of a variable-length integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("value is outside the range of a variable-length integer")]
pub struct VarIntOutOfRange;

/// An unsigned variable-length integer, from 0 to 2^62 − 1.
///
/// The range is checked when the value is created, so encoding a `VarInt` cannot
/// fail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VarInt(u64);

impl VarInt {
    /// The value zero.
    pub const ZERO: Self = Self(0);

    /// The largest value a variable-length integer can hold, 2^62 − 1.
    pub const MAX: Self = Self(MAX_EIGHT_BYTES);

    /// Creates a variable-length integer, failing if `value` exceeds [`VarInt::MAX`].
    pub const fn new(value: u64) -> Result<Self, VarIntOutOfRange> {
        if value <= MAX_EIGHT_BYTES {
            Ok(Self(value))
        } else {
            Err(VarIntOutOfRange)
        }
    }

    /// Creates a variable-length integer from a value that always fits.
    pub const fn from_u32(value: u32) -> Self {
        Self(value as u64)
    }

    /// Returns the value as a `u64`.
    pub const fn into_inner(self) -> u64 {
        self.0
    }

    /// Returns the number of bytes of the shortest encoding of this value.
    pub const fn encoded_len(self) -> usize {
        if self.0 <= MAX_ONE_BYTE {
            1
        } else if self.0 <= MAX_TWO_BYTES {
            2
        } else if self.0 <= MAX_FOUR_BYTES {
            4
        } else {
            8
        }
    }

    /// Returns the length of the encoding that starts with `first_byte`.
    pub const fn encoded_len_from_first_byte(first_byte: u8) -> usize {
        1 << (first_byte >> LENGTH_PREFIX_SHIFT)
    }
}

/// Bitwise AND of two values, for flag masks sent as variable-length integers. The
/// result never exceeds either operand, so it is always in range.
impl BitAnd for VarInt {
    type Output = Self;

    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// Bitwise OR of two values, for flag masks sent as variable-length integers. Both
/// operands are below 2^62, so the result is too.
impl BitOr for VarInt {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl From<u8> for VarInt {
    fn from(value: u8) -> Self {
        Self(u64::from(value))
    }
}

impl From<u16> for VarInt {
    fn from(value: u16) -> Self {
        Self(u64::from(value))
    }
}

impl From<u32> for VarInt {
    fn from(value: u32) -> Self {
        Self::from_u32(value)
    }
}

impl TryFrom<u64> for VarInt {
    type Error = VarIntOutOfRange;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<usize> for VarInt {
    type Error = VarIntOutOfRange;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        u64::try_from(value)
            .map_err(|_| VarIntOutOfRange)
            .and_then(Self::new)
    }
}

impl From<VarInt> for u64 {
    fn from(value: VarInt) -> Self {
        value.0
    }
}

/// A signed variable-length integer, from −2^61 to 2^61 − 1.
///
/// On the wire, n ≥ 0 is sent as the variable-length integer 2n and n < 0 as −2n − 1,
/// so 0, −1, 1, −2, 2 are sent as 0, 1, 2, 3, 4. Fields of this type are written
/// `(si)` in the specification.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignedVarInt(i64);

impl SignedVarInt {
    /// The value zero.
    pub const ZERO: Self = Self(0);

    /// The smallest value a signed variable-length integer can hold, −2^61.
    pub const MIN: Self = Self(-(1 << 61));

    /// The largest value a signed variable-length integer can hold, 2^61 − 1.
    pub const MAX: Self = Self((1 << 61) - 1);

    /// Creates a signed variable-length integer, failing if `value` is outside
    /// [`SignedVarInt::MIN`] to [`SignedVarInt::MAX`].
    pub const fn new(value: i64) -> Result<Self, VarIntOutOfRange> {
        if value >= Self::MIN.0 && value <= Self::MAX.0 {
            Ok(Self(value))
        } else {
            Err(VarIntOutOfRange)
        }
    }

    /// Creates a signed variable-length integer from a value that always fits.
    pub const fn from_i32(value: i32) -> Self {
        Self(value as i64)
    }

    /// Returns the value as an `i64`.
    pub const fn into_inner(self) -> i64 {
        self.0
    }

    /// Maps the value to the unsigned variable-length integer sent on the wire.
    pub const fn to_zigzag(self) -> VarInt {
        let zigzag = if self.0 >= 0 {
            (self.0 as u64) << 1
        } else {
            (((-(self.0 + 1)) as u64) << 1) | 1
        };
        VarInt(zigzag)
    }

    /// Maps an unsigned variable-length integer received on the wire back to the
    /// signed value it encodes.
    pub const fn from_zigzag(value: VarInt) -> Self {
        let magnitude = (value.0 >> 1) as i64;
        if value.0 & 1 == 0 {
            Self(magnitude)
        } else {
            Self(-magnitude - 1)
        }
    }
}

impl From<i8> for SignedVarInt {
    fn from(value: i8) -> Self {
        Self(i64::from(value))
    }
}

impl From<i16> for SignedVarInt {
    fn from(value: i16) -> Self {
        Self(i64::from(value))
    }
}

impl From<i32> for SignedVarInt {
    fn from(value: i32) -> Self {
        Self::from_i32(value)
    }
}

impl TryFrom<i64> for SignedVarInt {
    type Error = VarIntOutOfRange;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<SignedVarInt> for i64 {
    fn from(value: SignedVarInt) -> Self {
        value.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values at the edge of each encoding length use the expected number of bytes.
    #[test]
    fn encoded_len_at_boundaries() {
        let cases = [
            (0, 1),
            (MAX_ONE_BYTE, 1),
            (MAX_ONE_BYTE + 1, 2),
            (MAX_TWO_BYTES, 2),
            (MAX_TWO_BYTES + 1, 4),
            (MAX_FOUR_BYTES, 4),
            (MAX_FOUR_BYTES + 1, 8),
            (MAX_EIGHT_BYTES, 8),
        ];
        for (value, len) in cases {
            assert_eq!(VarInt::new(value).unwrap().encoded_len(), len, "{value}");
        }
    }

    /// The first byte of each encoding length gives that length.
    #[test]
    fn encoded_len_from_first_byte() {
        assert_eq!(VarInt::encoded_len_from_first_byte(0x25), 1);
        assert_eq!(VarInt::encoded_len_from_first_byte(0x7b), 2);
        assert_eq!(VarInt::encoded_len_from_first_byte(0x9d), 4);
        assert_eq!(VarInt::encoded_len_from_first_byte(0xc2), 8);
    }

    /// Values above 2^62 − 1 are rejected.
    #[test]
    fn rejects_out_of_range() {
        assert_eq!(VarInt::new(MAX_EIGHT_BYTES + 1), Err(VarIntOutOfRange));
        assert_eq!(VarInt::try_from(u64::MAX), Err(VarIntOutOfRange));
        assert_eq!(VarInt::new(MAX_EIGHT_BYTES), Ok(VarInt::MAX));
    }

    /// Bitwise operators combine flag masks.
    #[test]
    fn bitwise_operators() {
        let a = VarInt::from_u32(0b0110);
        let b = VarInt::from_u32(0b0011);
        assert_eq!((a & b).into_inner(), 0b0010);
        assert_eq!((a | b).into_inner(), 0b0111);
        assert_eq!(VarInt::MAX | VarInt::MAX, VarInt::MAX);
    }

    /// The zigzag mapping matches the examples of Section 1.5.2.
    #[test]
    fn zigzag_examples() {
        let cases = [(0, 0), (-1, 1), (1, 2), (-2, 3), (2, 4)];
        for (signed, unsigned) in cases {
            let value = SignedVarInt::new(signed).unwrap();
            assert_eq!(value.to_zigzag().into_inner(), unsigned);
            assert_eq!(SignedVarInt::from_zigzag(value.to_zigzag()), value);
        }
    }

    /// The extremes of the signed range map to the extremes of the unsigned range.
    #[test]
    fn zigzag_extremes() {
        assert_eq!(SignedVarInt::MIN.to_zigzag(), VarInt::MAX);
        assert_eq!(
            SignedVarInt::MAX.to_zigzag().into_inner(),
            MAX_EIGHT_BYTES - 1
        );
        assert_eq!(SignedVarInt::from_zigzag(VarInt::MAX), SignedVarInt::MIN);
        assert_eq!(SignedVarInt::new(i64::MIN), Err(VarIntOutOfRange));
        assert_eq!(SignedVarInt::new(i64::MAX), Err(VarIntOutOfRange));
    }
}
