//! Base64 encoding with the standard alphabet and padding (RFC 4648, Section 4),
//! as SCRAM uses it.

/// The standard Base64 alphabet.
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// The padding character.
const PADDING: u8 = b'=';

/// The number of input bytes in one Base64 group.
const GROUP_BYTES: usize = 3;

/// The number of characters in one Base64 group.
const GROUP_CHARS: usize = 4;

/// The number of bits each Base64 character carries.
const BITS_PER_CHAR: u32 = 6;

/// The mask of the bits one character carries.
const CHAR_MASK: u32 = 0x3f;

/// Encodes `bytes` as padded Base64.
pub fn encode(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(GROUP_BYTES) * GROUP_CHARS);
    for group in bytes.chunks(GROUP_BYTES) {
        let mut buffer = [0u8; GROUP_BYTES];
        buffer[..group.len()].copy_from_slice(group);
        let bits = u32::from_be_bytes([0, buffer[0], buffer[1], buffer[2]]);
        // A group of n bytes produces n + 1 characters, padded to four.
        for index in 0..GROUP_CHARS {
            if index <= group.len() {
                let shift = BITS_PER_CHAR * (GROUP_CHARS - 1 - index) as u32;
                encoded.push(char::from(ALPHABET[((bits >> shift) & CHAR_MASK) as usize]));
            } else {
                encoded.push(char::from(PADDING));
            }
        }
    }
    encoded
}

/// Returns the 6-bit value of a Base64 character.
fn value(character: u8) -> Option<u32> {
    ALPHABET
        .iter()
        .position(|&candidate| candidate == character)
        .map(|position| position as u32)
}

/// Decodes padded Base64, returning `None` if `encoded` is not canonical Base64.
pub fn decode(encoded: &str) -> Option<Vec<u8>> {
    let encoded = encoded.as_bytes();
    if !encoded.len().is_multiple_of(GROUP_CHARS) {
        return None;
    }
    let mut decoded = Vec::with_capacity(encoded.len() / GROUP_CHARS * GROUP_BYTES);
    let group_count = encoded.len() / GROUP_CHARS;
    for (group_index, group) in encoded.chunks(GROUP_CHARS).enumerate() {
        let padding = group
            .iter()
            .rev()
            .take_while(|&&byte| byte == PADDING)
            .count();
        let is_last = group_index + 1 == group_count;
        if padding > 2 || (padding > 0 && !is_last) {
            return None;
        }
        let mut bits = 0u32;
        for &character in &group[..GROUP_CHARS - padding] {
            bits = (bits << BITS_PER_CHAR) | value(character)?;
        }
        bits <<= BITS_PER_CHAR * padding as u32;
        let bytes = bits.to_be_bytes();
        let byte_count = GROUP_BYTES - padding;
        // Canonical encodings leave the unused low bits zero.
        if bytes[1 + byte_count..].iter().any(|&byte| byte != 0) {
            return None;
        }
        decoded.extend_from_slice(&bytes[1..=byte_count]);
    }
    Some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test vectors of RFC 4648, Section 10.
    const VECTORS: [(&str, &str); 7] = [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ];

    /// Encoding matches RFC 4648.
    #[test]
    fn encodes_rfc4648_vectors() {
        for (plain, encoded) in VECTORS {
            assert_eq!(encode(plain.as_bytes()), encoded);
        }
    }

    /// Decoding matches RFC 4648.
    #[test]
    fn decodes_rfc4648_vectors() {
        for (plain, encoded) in VECTORS {
            assert_eq!(decode(encoded).unwrap(), plain.as_bytes());
        }
    }

    /// Every byte value round-trips.
    #[test]
    fn round_trips_all_bytes() {
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&bytes)).unwrap(), bytes);
    }

    /// Malformed input is rejected.
    #[test]
    fn rejects_malformed_input() {
        for encoded in ["Zg=", "Z===", "Zg==Zg==", "Zh==", "Zm9v!A==", "===="] {
            assert_eq!(decode(encoded), None, "{encoded}");
        }
    }
}
