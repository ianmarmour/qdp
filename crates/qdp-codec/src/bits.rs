//! Reading and writing AV1 syntax elements bit by bit (AV1 specification,
//! Sections 4.10 and 8.1).

use crate::CodecError;

/// The number of bits in a byte.
const BYTE_BITS: usize = 8;

/// The bits of a LEB128 byte that carry the value.
#[cfg(target_os = "linux")]
const LEB128_VALUE_MASK: u64 = 0x7f;

/// The bit of a LEB128 byte that says another byte follows.
#[cfg(target_os = "linux")]
const LEB128_CONTINUE: u8 = 0x80;

/// The longest run of zero bits a `uvlc()` value may start with; longer
/// runs stand for the largest 32-bit value.
const UVLC_MAX_LEADING_ZEROS: u32 = 32;

/// The error for a header that ends before its last syntax element.
const TRUNCATED: CodecError = CodecError::MalformedBitstream("truncated header");

/// Reads values most significant bit first, as AV1's descriptors do.
#[derive(Debug, Clone)]
pub(crate) struct BitReader<'a> {
    /// The bytes being read.
    bytes: &'a [u8],
    /// The number of bits read.
    position: usize,
}

impl<'a> BitReader<'a> {
    /// Starts reading at the first bit of `bytes`.
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    /// Reads one bit: AV1's `f(1)`.
    pub(crate) fn bit(&mut self) -> Result<bool, CodecError> {
        let byte = self.bytes.get(self.position / BYTE_BITS).ok_or(TRUNCATED)?;
        let set = byte & (0x80 >> (self.position % BYTE_BITS)) != 0;
        self.position += 1;
        Ok(set)
    }

    /// Reads `count` bits, most significant first: AV1's `f(count)`.
    pub(crate) fn bits(&mut self, count: u32) -> Result<u64, CodecError> {
        debug_assert!(count <= u64::BITS);
        let mut value = 0;
        for _ in 0..count {
            value = (value << 1) | u64::from(self.bit()?);
        }
        Ok(value)
    }

    /// Reads `count` bits as a value that fits 32 bits.
    pub(crate) fn bits32(&mut self, count: u32) -> Result<u32, CodecError> {
        debug_assert!(count <= u32::BITS);
        Ok(self.bits(count)? as u32)
    }

    /// Reads a variable-length unsigned value: AV1's `uvlc()`.
    pub(crate) fn uvlc(&mut self) -> Result<u64, CodecError> {
        let mut leading_zeros = 0;
        while !self.bit()? {
            leading_zeros += 1;
            if leading_zeros >= UVLC_MAX_LEADING_ZEROS {
                return Ok(u64::from(u32::MAX));
            }
        }
        Ok(self.bits(leading_zeros)? + (1 << leading_zeros) - 1)
    }
}

// The frame header parser reads these too.
#[cfg(any(windows, target_os = "linux", test))]
impl BitReader<'_> {
    /// Returns the number of bits read: AV1's `get_position()`.
    pub(crate) fn position(&self) -> usize {
        self.position
    }

    /// Reads a signed value of `count` bits in two's complement: AV1's
    /// `su(count)`.
    pub(crate) fn signed(&mut self, count: u32) -> Result<i32, CodecError> {
        debug_assert!((1..u32::BITS).contains(&count));
        let value = self.bits(count)? as i64;
        let sign = 1i64 << (count - 1);
        Ok((if value & sign != 0 {
            value - 2 * sign
        } else {
            value
        }) as i32)
    }

    /// Reads a value below `n`, with fewer bits for the smaller values when
    /// `n` is not a power of two: AV1's `ns(n)`.
    pub(crate) fn non_symmetric(&mut self, n: u32) -> Result<u32, CodecError> {
        debug_assert!(n > 0);
        let width = u32::BITS - n.leading_zeros();
        let short = (1u32 << width) - n;
        let value = self.bits32(width - 1)?;
        if value < short {
            return Ok(value);
        }
        let extra = u32::from(self.bit()?);
        Ok((value << 1) - short + extra)
    }

    /// Skips to the next byte boundary: AV1's `byte_alignment()`.
    pub(crate) fn byte_alignment(&mut self) {
        self.position = self.position.next_multiple_of(BYTE_BITS);
    }
}

/// Writes values most significant bit first, as AV1's `f(n)` descriptor reads
/// them.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BitWriter {
    /// The bytes written so far, the last one possibly partial.
    bytes: Vec<u8>,
    /// The number of bits written.
    length: usize,
}

#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
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

    /// Values are read most significant bit first, across byte boundaries.
    #[test]
    fn reads_bits_most_significant_first() {
        let mut reader = BitReader::new(&[0b1011_1110, 0b0001_1000]);
        assert_eq!(reader.bits(3).unwrap(), 0b101);
        assert_eq!(reader.bits(9).unwrap(), 0b1_1110_0001);
        assert!(reader.bit().unwrap());
        assert_eq!(reader.position(), 13);
        assert_eq!(reader.bits(3).unwrap(), 0);
        assert!(reader.bit().is_err());
    }

    /// `su(n)` reads two's complement values.
    #[test]
    fn reads_signed_values() {
        // 7 bits: 0b111_1111 is -1, 0b100_0000 is -64, 0b011_1111 is 63.
        let mut reader = BitReader::new(&[0b1111_1111, 0b0000_0001, 0b1111_1000]);
        assert_eq!(reader.signed(7).unwrap(), -1);
        assert_eq!(reader.signed(7).unwrap(), -64);
        assert_eq!(reader.signed(7).unwrap(), 63);
    }

    /// `ns(n)` reads the smaller values with one bit fewer: below 5, 0 to 2
    /// take two bits and 3 and 4 take three.
    #[test]
    fn reads_non_symmetric_values() {
        // 00 → 0, 10 → 2, 110 → 3, 111 → 4.
        let mut reader = BitReader::new(&[0b0010_1101, 0b1100_0000]);
        assert_eq!(reader.non_symmetric(5).unwrap(), 0);
        assert_eq!(reader.non_symmetric(5).unwrap(), 2);
        assert_eq!(reader.non_symmetric(5).unwrap(), 3);
        assert_eq!(reader.non_symmetric(5).unwrap(), 4);
        // Powers of two take their whole width; 1 takes no bits.
        let mut reader = BitReader::new(&[0b1100_0000]);
        assert_eq!(reader.non_symmetric(4).unwrap(), 3);
        assert_eq!(reader.non_symmetric(1).unwrap(), 0);
        assert_eq!(reader.position(), 2);
    }

    /// `byte_alignment()` skips to the next byte, unless already there.
    #[test]
    fn aligns_to_bytes() {
        let mut reader = BitReader::new(&[0b1000_0000, 0x34]);
        assert!(reader.bit().unwrap());
        reader.byte_alignment();
        assert_eq!(reader.position(), 8);
        reader.byte_alignment();
        assert_eq!(reader.position(), 8);
        assert_eq!(reader.bits(8).unwrap(), 0x34);
    }

    /// `uvlc()` counts leading zeros, then reads as many bits.
    #[test]
    fn reads_uvlc_values() {
        // 1 → 0; 010 → 1; 00111 → 6.
        let mut reader = BitReader::new(&[0b1010_0011, 0b1000_0000]);
        assert_eq!(reader.uvlc().unwrap(), 0);
        assert_eq!(reader.uvlc().unwrap(), 1);
        assert_eq!(reader.uvlc().unwrap(), 6);
    }

    /// Bits are packed most significant first, across byte boundaries.
    #[cfg(target_os = "linux")]
    #[test]
    fn packs_bits_most_significant_first() {
        let mut writer = BitWriter::new();
        writer.bits(0b101, 3);
        writer.bits(0b1_1110_0001, 9);
        writer.bit(true);
        assert_eq!(writer.into_bytes(), [0b1011_1110, 0b0001_1000]);
    }

    /// Trailing bits add a one and pad to a byte, even when already aligned.
    #[cfg(target_os = "linux")]
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
    #[cfg(target_os = "linux")]
    #[test]
    fn writes_leb128() {
        let mut output = Vec::new();
        write_leb128(&mut output, 5);
        write_leb128(&mut output, 300);
        assert_eq!(output, [5, 0xac, 0x02]);
    }
}
