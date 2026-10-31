//! Writing AV1 syntax elements bit by bit (AV1 specification, Section 4.10).

/// The number of bits in a byte.
const BYTE_BITS: usize = 8;

/// The bits of a LEB128 byte that carry the value.
const LEB128_VALUE_MASK: u64 = 0x7f;

/// The bit of a LEB128 byte that says another byte follows.
const LEB128_CONTINUE: u8 = 0x80;

/// Writes values most significant bit first, as AV1's `f(n)` descriptor reads
/// them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BitWriter {
    /// The bytes written so far, the last one possibly partial.
    bytes: Vec<u8>,
    /// The number of bits written.
    length: usize,
}

impl BitWriter {
    /// Creates an empty writer.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Writes the low `count` bits of `value`, most significant first: AV1's
    /// `f(count)`.
    pub(crate) fn bits(&mut self, value: u64, count: u32) {
        debug_assert!(count <= u64::BITS);
        debug_assert!(
            count == u64::BITS || value >> count == 0,
            "{value} needs more than {count} bits"
        );
        for shift in (0..count).rev() {
            self.bit((value >> shift) & 1 == 1);
        }
    }

    /// Writes one bit: AV1's `f(1)`.
    pub(crate) fn bit(&mut self, set: bool) {
        if self.length.is_multiple_of(BYTE_BITS) {
            self.bytes.push(0);
        }
        if set {
            let last = self.bytes.len() - 1;
            self.bytes[last] |= 0x80 >> (self.length % BYTE_BITS);
        }
        self.length += 1;
    }

    /// Writes `trailing_bits()`: a one bit, then zero bits up to the next byte
    /// boundary.
    pub(crate) fn trailing_bits(&mut self) {
        self.bit(true);
        while !self.length.is_multiple_of(BYTE_BITS) {
            self.bit(false);
        }
    }

    /// Returns the bytes written. The last byte is padded with zero bits.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

/// Appends `value` as an unsigned LEB128 number with the fewest bytes: AV1's
/// `leb128()`.
pub(crate) fn write_leb128(output: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & LEB128_VALUE_MASK) as u8;
        value >>= 7;
        if value == 0 {
            output.push(byte);
            return;
        }
        output.push(byte | LEB128_CONTINUE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bits are packed most significant first, across byte boundaries.
    #[test]
    fn packs_bits_most_significant_first() {
        let mut writer = BitWriter::new();
        writer.bits(0b101, 3);
        writer.bits(0b1_1110_0001, 9);
        writer.bit(true);
        assert_eq!(writer.into_bytes(), [0b1011_1110, 0b0001_1000]);
    }

    /// Trailing bits add a one and pad to a byte, even when already aligned.
    #[test]
    fn writes_trailing_bits() {
        let mut writer = BitWriter::new();
        writer.bits(0b11, 2);
        writer.trailing_bits();
        assert_eq!(writer.into_bytes(), [0b1110_0000]);
        let mut aligned = BitWriter::new();
        aligned.bits(0xab, 8);
        aligned.trailing_bits();
        assert_eq!(aligned.into_bytes(), [0xab, 0x80]);
    }

    /// LEB128 values use seven bits per byte, least significant group first.
    #[test]
    fn writes_leb128() {
        let mut output = Vec::new();
        write_leb128(&mut output, 5);
        write_leb128(&mut output, 300);
        assert_eq!(output, [5, 0xac, 0x02]);
    }
}
