//! Encoding of QDP fields into a byte buffer.

use crate::wire::{SignedVarInt, VarInt};

/// The two length bits of the first byte of a two-byte variable-length integer.
const TWO_BYTE_PREFIX: u64 = 0b01 << 14;

/// The two length bits of the first byte of a four-byte variable-length integer.
const FOUR_BYTE_PREFIX: u64 = 0b10 << 30;

/// The two length bits of the first byte of an eight-byte variable-length integer.
const EIGHT_BYTE_PREFIX: u64 = 0b11 << 62;

/// A buffer that QDP fields are appended to.
///
/// Writing cannot fail: every value a field can hold has an encoding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Writer {
    /// The bytes written so far.
    bytes: Vec<u8>,
}

impl Writer {
    /// Creates an empty writer.
    pub const fn new() -> Self {
        Self { bytes: Vec::new() }
    }

    /// Creates an empty writer with room for `capacity` bytes.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
        }
    }

    /// Returns the number of bytes written.
    pub const fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether nothing has been written.
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Returns the bytes written so far.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the bytes written, consuming the writer.
    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// Appends raw bytes, for fields that run to the end of a structure.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    /// Appends an unsigned 8-bit integer.
    pub fn write_u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// Appends an unsigned 16-bit integer in network byte order.
    pub fn write_u16(&mut self, value: u16) {
        self.write_bytes(&value.to_be_bytes());
    }

    /// Appends an unsigned 32-bit integer in network byte order.
    pub fn write_u32(&mut self, value: u32) {
        self.write_bytes(&value.to_be_bytes());
    }

    /// Appends an unsigned 64-bit integer in network byte order.
    pub fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_be_bytes());
    }

    /// Appends a signed 8-bit integer in two's complement.
    pub fn write_i8(&mut self, value: i8) {
        self.write_bytes(&value.to_be_bytes());
    }

    /// Appends a signed 16-bit integer in two's complement and network byte order.
    pub fn write_i16(&mut self, value: i16) {
        self.write_bytes(&value.to_be_bytes());
    }

    /// Appends a signed 32-bit integer in two's complement and network byte order.
    pub fn write_i32(&mut self, value: i32) {
        self.write_bytes(&value.to_be_bytes());
    }

    /// Appends a variable-length integer in its shortest encoding (Section 1.5.1).
    pub fn write_varint(&mut self, value: VarInt) {
        let encoded_len = value.encoded_len();
        let value = value.into_inner();
        match encoded_len {
            1 => self.write_u8(value as u8),
            2 => self.write_u16((TWO_BYTE_PREFIX | value) as u16),
            4 => self.write_u32((FOUR_BYTE_PREFIX | value) as u32),
            _ => self.write_u64(EIGHT_BYTE_PREFIX | value),
        }
    }

    /// Appends a signed variable-length integer (Section 1.5.2).
    pub fn write_signed_varint(&mut self, value: SignedVarInt) {
        self.write_varint(value.to_zigzag());
    }

    /// Appends a variable-length integer holding a length.
    ///
    /// # Panics
    ///
    /// Panics if `length` exceeds [`VarInt::MAX`], which no buffer in memory can
    /// reach.
    fn write_length(&mut self, length: usize) {
        let length =
            VarInt::try_from(length).expect("no buffer is longer than a variable-length integer");
        self.write_varint(length);
    }

    /// Appends the count of a list of entries.
    pub fn write_count(&mut self, count: usize) {
        self.write_length(count);
    }

    /// Appends an Octets field: the length of `bytes` followed by the bytes
    /// (Section 1.5.4).
    pub fn write_octets(&mut self, bytes: &[u8]) {
        self.write_length(bytes.len());
        self.write_bytes(bytes);
    }

    /// Appends a String field (Section 1.5.4).
    ///
    /// The caller ensures that `value` contains no byte order mark or NUL character,
    /// which the specification forbids.
    pub fn write_string(&mut self, value: &str) {
        self.write_octets(value.as_bytes());
    }

    /// Appends a String field that may be at most `limit` bytes long:
    /// `value`, or its longest beginning that fits and ends on a character
    /// boundary. It is for names and reasons, which receivers refuse when
    /// longer than their limit, and which a sender takes from places it
    /// does not control, such as the operating system.
    pub fn write_string_at_most(&mut self, value: &str, limit: usize) {
        let mut end = value.len().min(limit);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        self.write_string(&value[..end]);
    }

    /// Appends a length-prefixed structure, such as an entry with an Entry Length,
    /// whose contents `write_contents` writes.
    pub fn write_length_prefixed(&mut self, write_contents: impl FnOnce(&mut Writer)) {
        let mut contents = Writer::new();
        write_contents(&mut contents);
        self.write_octets(contents.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::Reader;

    /// The examples of RFC 9000, Appendix A.1, encode to the bytes it gives.
    #[test]
    fn rfc9000_varint_examples() {
        let cases: [(u64, &[u8]); 4] = [
            (
                151_288_809_941_952_652,
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c],
            ),
            (494_878_333, &[0x9d, 0x7f, 0x3e, 0x7d]),
            (15_293, &[0x7b, 0xbd]),
            (37, &[0x25]),
        ];
        for (value, bytes) in cases {
            let mut writer = Writer::new();
            writer.write_varint(VarInt::new(value).unwrap());
            assert_eq!(writer.as_bytes(), bytes);
        }
    }

    /// Every encoding length round-trips through the reader at its boundaries.
    #[test]
    fn varint_round_trip() {
        let values = [
            0,
            63,
            64,
            16_383,
            16_384,
            1_073_741_823,
            1_073_741_824,
            VarInt::MAX.into_inner(),
        ];
        for value in values {
            let value = VarInt::new(value).unwrap();
            let mut writer = Writer::new();
            writer.write_varint(value);
            assert_eq!(writer.len(), value.encoded_len());
            let mut reader = Reader::new(writer.as_bytes());
            assert_eq!(reader.read_varint().unwrap(), value);
            assert!(reader.is_empty());
        }
    }

    /// Signed variable-length integers round-trip through the reader.
    #[test]
    fn signed_varint_round_trip() {
        for value in [
            SignedVarInt::MIN,
            SignedVarInt::from(-256),
            SignedVarInt::ZERO,
            SignedVarInt::MAX,
        ] {
            let mut writer = Writer::new();
            writer.write_signed_varint(value);
            let mut reader = Reader::new(writer.as_bytes());
            assert_eq!(reader.read_signed_varint().unwrap(), value);
        }
    }

    /// Fixed-width integers are written big-endian.
    #[test]
    fn fixed_width_integers() {
        let mut writer = Writer::new();
        writer.write_u8(0x01);
        writer.write_u16(0x0203);
        writer.write_u32(0x0405_0607);
        writer.write_i16(-2);
        writer.write_i32(-3);
        assert_eq!(
            writer.as_bytes(),
            [
                0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0xff, 0xfe, 0xff, 0xff, 0xff, 0xfd
            ]
        );
    }

    /// A string longer than its limit is cut at the last character
    /// boundary that fits.
    #[test]
    fn bounded_strings() {
        let mut writer = Writer::new();
        writer.write_string_at_most("qdp", 8);
        // "é" takes two bytes, so four bytes hold "aé" and one more "a".
        writer.write_string_at_most("aéa", 4);
        writer.write_string_at_most("aéa", 2);
        assert_eq!(
            writer.as_bytes(),
            [3, b'q', b'd', b'p', 4, b'a', 0xc3, 0xa9, b'a', 1, b'a']
        );
    }

    /// Octets, Strings and length-prefixed structures carry their length first.
    #[test]
    fn length_prefixed_fields() {
        let mut writer = Writer::new();
        writer.write_octets(&[0xaa, 0xbb]);
        writer.write_string("qdp");
        writer.write_length_prefixed(|entry| {
            entry.write_u8(0x07);
            entry.write_u16(0xeeee);
        });
        assert_eq!(
            writer.into_bytes(),
            [
                0x02, 0xaa, 0xbb, 0x03, b'q', b'd', b'p', 0x03, 0x07, 0xee, 0xee
            ]
        );
    }
}
