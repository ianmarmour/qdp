//! Decoding of QDP fields from a byte slice.

use crate::wire::{SignedVarInt, VarInt, WireError};

/// The bits of the first byte of a variable-length integer that hold the value.
const VARINT_VALUE_MASK: u8 = 0x3f;

/// A cursor that reads QDP fields from the front of a byte slice.
///
/// Every read either consumes the whole field or fails with
/// [`WireError::UnexpectedEnd`] and leaves the reader unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reader<'a> {
    /// The bytes not yet read.
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Creates a reader over `bytes`.
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Returns the number of bytes not yet read.
    pub const fn remaining(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether every byte has been read.
    pub const fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Reads exactly `len` bytes.
    pub fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        let (head, tail) = self
            .bytes
            .split_at_checked(len)
            .ok_or(WireError::UnexpectedEnd)?;
        self.bytes = tail;
        Ok(head)
    }

    /// Reads every remaining byte, for fields that run to the end of a structure.
    pub fn read_rest(&mut self) -> &'a [u8] {
        std::mem::take(&mut self.bytes)
    }

    /// Reads a fixed-size array of bytes.
    fn read_array<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let (head, tail) = self
            .bytes
            .split_first_chunk::<N>()
            .ok_or(WireError::UnexpectedEnd)?;
        self.bytes = tail;
        Ok(*head)
    }

    /// Reads an unsigned 8-bit integer.
    pub fn read_u8(&mut self) -> Result<u8, WireError> {
        self.read_array().map(u8::from_be_bytes)
    }

    /// Reads an unsigned 16-bit integer in network byte order.
    pub fn read_u16(&mut self) -> Result<u16, WireError> {
        self.read_array().map(u16::from_be_bytes)
    }

    /// Reads an unsigned 32-bit integer in network byte order.
    pub fn read_u32(&mut self) -> Result<u32, WireError> {
        self.read_array().map(u32::from_be_bytes)
    }

    /// Reads an unsigned 64-bit integer in network byte order.
    pub fn read_u64(&mut self) -> Result<u64, WireError> {
        self.read_array().map(u64::from_be_bytes)
    }

    /// Reads a signed 8-bit integer in two's complement.
    pub fn read_i8(&mut self) -> Result<i8, WireError> {
        self.read_array().map(i8::from_be_bytes)
    }

    /// Reads a signed 16-bit integer in two's complement and network byte order.
    pub fn read_i16(&mut self) -> Result<i16, WireError> {
        self.read_array().map(i16::from_be_bytes)
    }

    /// Reads a signed 32-bit integer in two's complement and network byte order.
    pub fn read_i32(&mut self) -> Result<i32, WireError> {
        self.read_array().map(i32::from_be_bytes)
    }

    /// Reads a variable-length integer, accepting encodings that are longer than
    /// necessary (Section 1.5.1).
    pub fn read_varint(&mut self) -> Result<VarInt, WireError> {
        let first = *self.bytes.first().ok_or(WireError::UnexpectedEnd)?;
        let encoding = self.read_bytes(VarInt::encoded_len_from_first_byte(first))?;
        let value = encoding[1..]
            .iter()
            .fold(u64::from(first & VARINT_VALUE_MASK), |value, &byte| {
                (value << u8::BITS) | u64::from(byte)
            });
        Ok(VarInt::new(value).expect("62 bits of payload always fit a variable-length integer"))
    }

    /// Reads a signed variable-length integer (Section 1.5.2).
    pub fn read_signed_varint(&mut self) -> Result<SignedVarInt, WireError> {
        self.read_varint().map(SignedVarInt::from_zigzag)
    }

    /// Reads a variable-length integer and returns it as a length within this reader.
    ///
    /// A length longer than the remaining data cannot be satisfied, so it fails with
    /// [`WireError::UnexpectedEnd`].
    fn read_length(&mut self) -> Result<usize, WireError> {
        let length = usize::try_from(self.read_varint()?.into_inner())
            .map_err(|_| WireError::UnexpectedEnd)?;
        if length > self.remaining() {
            return Err(WireError::UnexpectedEnd);
        }
        Ok(length)
    }

    /// Reads an Octets field: a variable-length integer length followed by that many
    /// bytes (Section 1.5.4).
    pub fn read_octets(&mut self) -> Result<&'a [u8], WireError> {
        let original = *self;
        let result = self
            .read_length()
            .and_then(|length| self.read_bytes(length));
        if result.is_err() {
            *self = original;
        }
        result
    }

    /// Reads a String field: an Octets field whose value is valid UTF-8
    /// (Section 1.5.4).
    pub fn read_string(&mut self) -> Result<&'a str, WireError> {
        let original = *self;
        let result = self
            .read_octets()
            .and_then(|bytes| std::str::from_utf8(bytes).map_err(|_| WireError::InvalidUtf8));
        if result.is_err() {
            *self = original;
        }
        result
    }

    /// Reads an Octets field whose value may be at most `limit` bytes long.
    pub fn read_octets_at_most(
        &mut self,
        field: &'static str,
        limit: usize,
    ) -> Result<&'a [u8], WireError> {
        let original = *self;
        let bytes = self.read_octets()?;
        if bytes.len() > limit {
            *self = original;
            return Err(WireError::LimitExceeded {
                field,
                limit: limit as u64,
            });
        }
        Ok(bytes)
    }

    /// Reads a String field whose value may be at most `limit` bytes long.
    pub fn read_string_at_most(
        &mut self,
        field: &'static str,
        limit: usize,
    ) -> Result<&'a str, WireError> {
        let original = *self;
        let value = self.read_string()?;
        if value.len() > limit {
            *self = original;
            return Err(WireError::LimitExceeded {
                field,
                limit: limit as u64,
            });
        }
        Ok(value)
    }

    /// Reads the count of a list of entries, which may be at most `limit`.
    ///
    /// Every entry takes at least one byte, so a count larger than the remaining data
    /// fails with [`WireError::UnexpectedEnd`]. The returned count is therefore safe
    /// to preallocate for.
    pub fn read_count(&mut self, field: &'static str, limit: u64) -> Result<usize, WireError> {
        let original = *self;
        let count = self.read_varint()?.into_inner();
        if count > limit {
            *self = original;
            return Err(WireError::LimitExceeded { field, limit });
        }
        match usize::try_from(count) {
            Ok(count) if count <= self.remaining() => Ok(count),
            _ => {
                *self = original;
                Err(WireError::UnexpectedEnd)
            }
        }
    }

    /// Reads a length-prefixed structure, such as an entry with an Entry Length, and
    /// returns a reader over its contents.
    ///
    /// The caller reads the fields it knows from the returned reader and ignores any
    /// that remain, as extensible structures require (Section 1.6, rule 3).
    pub fn read_length_prefixed(&mut self) -> Result<Reader<'a>, WireError> {
        self.read_octets().map(Reader::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The examples of RFC 9000, Appendix A.1, decode to the values it gives.
    #[test]
    fn rfc9000_varint_examples() {
        let cases: [(&[u8], u64); 5] = [
            (
                &[0xc2, 0x19, 0x7c, 0x5e, 0xff, 0x14, 0xe8, 0x8c],
                151_288_809_941_952_652,
            ),
            (&[0x9d, 0x7f, 0x3e, 0x7d], 494_878_333),
            (&[0x7b, 0xbd], 15_293),
            (&[0x25], 37),
            (&[0x40, 0x25], 37),
        ];
        for (bytes, value) in cases {
            let mut reader = Reader::new(bytes);
            assert_eq!(reader.read_varint().unwrap().into_inner(), value);
            assert!(reader.is_empty());
        }
    }

    /// A truncated variable-length integer fails without consuming anything.
    #[test]
    fn truncated_varint() {
        let mut reader = Reader::new(&[0x9d, 0x7f]);
        assert_eq!(reader.read_varint(), Err(WireError::UnexpectedEnd));
        assert_eq!(reader.remaining(), 2);
        assert_eq!(
            Reader::new(&[]).read_varint(),
            Err(WireError::UnexpectedEnd)
        );
    }

    /// Fixed-width integers are big-endian, and signed ones use two's complement.
    #[test]
    fn fixed_width_integers() {
        let bytes = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0xff, 0xfe, 0xff, 0xff, 0xff, 0xfd,
        ];
        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_u8().unwrap(), 0x01);
        assert_eq!(reader.read_u16().unwrap(), 0x0203);
        assert_eq!(reader.read_u32().unwrap(), 0x0405_0607);
        assert_eq!(reader.read_i16().unwrap(), -2);
        assert_eq!(reader.read_i32().unwrap(), -3);
        assert!(reader.is_empty());
        assert_eq!(reader.read_u8(), Err(WireError::UnexpectedEnd));
    }

    /// Signed variable-length integers use the zigzag mapping.
    #[test]
    fn signed_varint() {
        let mut reader = Reader::new(&[0x00, 0x01, 0x02, 0x03]);
        let values: Vec<i64> = (0..4)
            .map(|_| reader.read_signed_varint().unwrap().into_inner())
            .collect();
        assert_eq!(values, [0, -1, 1, -2]);
    }

    /// Octets and Strings read their length and then their value.
    #[test]
    fn octets_and_strings() {
        let mut reader = Reader::new(&[0x02, 0xaa, 0xbb, 0x03, b'q', b'd', b'p', 0x00]);
        assert_eq!(reader.read_octets().unwrap(), [0xaa, 0xbb]);
        assert_eq!(reader.read_string().unwrap(), "qdp");
        assert_eq!(reader.read_string().unwrap(), "");
        assert!(reader.is_empty());
    }

    /// A length beyond the end of the data fails without consuming anything.
    #[test]
    fn octets_longer_than_data() {
        let mut reader = Reader::new(&[0x05, 0x01, 0x02]);
        assert_eq!(reader.read_octets(), Err(WireError::UnexpectedEnd));
        assert_eq!(reader.remaining(), 3);
    }

    /// A String that is not valid UTF-8 fails without consuming anything.
    #[test]
    fn invalid_utf8() {
        let mut reader = Reader::new(&[0x02, 0xc3, 0x28]);
        assert_eq!(reader.read_string(), Err(WireError::InvalidUtf8));
        assert_eq!(reader.remaining(), 3);
    }

    /// A length-prefixed structure is read as a whole, so that its unknown trailing
    /// fields are skipped.
    #[test]
    fn length_prefixed_structure() {
        let mut reader = Reader::new(&[0x03, 0x07, 0xee, 0xee, 0x09]);
        let mut entry = reader.read_length_prefixed().unwrap();
        assert_eq!(entry.read_u8().unwrap(), 0x07);
        assert_eq!(entry.remaining(), 2);
        assert_eq!(reader.read_u8().unwrap(), 0x09);
    }

    /// Bounded fields reject values above their limit without consuming anything.
    #[test]
    fn bounded_fields() {
        let mut reader = Reader::new(&[0x03, b'q', b'd', b'p']);
        assert_eq!(
            reader.read_string_at_most("Name", 2),
            Err(WireError::LimitExceeded {
                field: "Name",
                limit: 2
            })
        );
        assert_eq!(
            reader.read_octets_at_most("Data", 2),
            Err(WireError::LimitExceeded {
                field: "Data",
                limit: 2
            })
        );
        assert_eq!(reader.remaining(), 4);
        assert_eq!(reader.read_string_at_most("Name", 3).unwrap(), "qdp");
    }

    /// Counts above their limit, or above the remaining data, are rejected.
    #[test]
    fn counts() {
        assert_eq!(
            Reader::new(&[0x05, 0, 0, 0, 0, 0]).read_count("Count", 4),
            Err(WireError::LimitExceeded {
                field: "Count",
                limit: 4
            })
        );
        assert_eq!(
            Reader::new(&[0x03, 0, 0]).read_count("Count", 4),
            Err(WireError::UnexpectedEnd)
        );
        let mut reader = Reader::new(&[0x02, 0, 0]);
        assert_eq!(reader.read_count("Count", 4), Ok(2));
        assert_eq!(reader.remaining(), 2);
    }

    /// Fields that run to the end of a structure take every remaining byte.
    #[test]
    fn read_rest() {
        let mut reader = Reader::new(&[0x01, 0x02, 0x03]);
        assert_eq!(reader.read_u8().unwrap(), 0x01);
        assert_eq!(reader.read_rest(), [0x02, 0x03]);
        assert!(reader.is_empty());
    }
}
