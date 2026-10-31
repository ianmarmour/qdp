//! AV1 Open Bitstream Units, as QDP carries them (Section 6.4.1).

use crate::CodecError;

/// The bit of an OBU header that is always zero.
const FORBIDDEN_BIT: u8 = 0x80;

/// The shift of the OBU type field in the header byte.
const TYPE_SHIFT: u8 = 3;

/// The mask of the OBU type field after shifting.
const TYPE_MASK: u8 = 0x0f;

/// The flag of an OBU header that says an extension byte follows.
const EXTENSION_FLAG: u8 = 0x04;

/// The flag of an OBU header that says a size field follows.
const HAS_SIZE_FLAG: u8 = 0x02;

/// The continuation bit of a LEB128 byte.
const LEB128_CONTINUE: u8 = 0x80;

/// The value bits of a LEB128 byte.
const LEB128_VALUE: u8 = 0x7f;

/// The largest number of bytes in an AV1 LEB128 value.
const LEB128_MAX_BYTES: usize = 8;

/// The type of an OBU (AV1 specification, Section 6.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObuType {
    /// A sequence header.
    SequenceHeader,
    /// A temporal delimiter, which QDP temporal units must not contain.
    TemporalDelimiter,
    /// A frame header.
    FrameHeader,
    /// A tile group.
    TileGroup,
    /// Metadata.
    Metadata,
    /// A frame header and tile group together.
    Frame,
    /// A redundant frame header.
    RedundantFrameHeader,
    /// A tile list, which QDP temporal units must not contain.
    TileList,
    /// Padding.
    Padding,
    /// A reserved type.
    Reserved(u8),
}

impl ObuType {
    /// Returns the type of a header's type field.
    fn from_field(field: u8) -> Self {
        match field {
            1 => Self::SequenceHeader,
            2 => Self::TemporalDelimiter,
            3 => Self::FrameHeader,
            4 => Self::TileGroup,
            5 => Self::Metadata,
            6 => Self::Frame,
            7 => Self::RedundantFrameHeader,
            8 => Self::TileList,
            15 => Self::Padding,
            other => Self::Reserved(other),
        }
    }
}

/// One OBU of a temporal unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Obu<'a> {
    /// The OBU's type.
    pub obu_type: ObuType,
    /// The whole OBU, header included.
    pub bytes: &'a [u8],
}

/// Reads an unsigned LEB128 value, returning it and its length.
fn read_leb128(bytes: &[u8]) -> Result<(u64, usize), CodecError> {
    let mut value = 0u64;
    for (index, &byte) in bytes.iter().take(LEB128_MAX_BYTES).enumerate() {
        value |= u64::from(byte & LEB128_VALUE) << (7 * index);
        if byte & LEB128_CONTINUE == 0 {
            return Ok((value, index + 1));
        }
    }
    Err(CodecError::MalformedBitstream("truncated LEB128 value"))
}

/// Splits a temporal unit in the low overhead bitstream format into OBUs.
///
/// Every OBU must have a size field and no extension header, as QDP requires.
pub fn parse_obus(temporal_unit: &[u8]) -> Result<Vec<Obu<'_>>, CodecError> {
    let mut obus = Vec::new();
    let mut rest = temporal_unit;
    while let Some(&header) = rest.first() {
        if header & FORBIDDEN_BIT != 0 {
            return Err(CodecError::MalformedBitstream("forbidden bit set"));
        }
        if header & EXTENSION_FLAG != 0 {
            return Err(CodecError::MalformedBitstream(
                "OBU extension headers are not allowed",
            ));
        }
        if header & HAS_SIZE_FLAG == 0 {
            return Err(CodecError::MalformedBitstream(
                "every OBU needs a size field",
            ));
        }
        let (size, size_len) = read_leb128(&rest[1..])?;
        let total = usize::try_from(size)
            .ok()
            .and_then(|size| size.checked_add(1 + size_len))
            .filter(|&total| total <= rest.len())
            .ok_or(CodecError::MalformedBitstream(
                "OBU longer than the temporal unit",
            ))?;
        let (bytes, tail) = rest.split_at(total);
        obus.push(Obu {
            obu_type: ObuType::from_field((header >> TYPE_SHIFT) & TYPE_MASK),
            bytes,
        });
        rest = tail;
    }
    Ok(obus)
}

/// Converts an encoder's temporal unit into the form QDP sends: temporal
/// delimiters and padding removed, and, for key frames, `sequence_header`
/// prepended if the unit does not already start with one (Section 6.4.1).
pub fn to_qdp_temporal_unit(
    packet: &[u8],
    is_key_frame: bool,
    sequence_header: &[u8],
) -> Result<Vec<u8>, CodecError> {
    let obus = parse_obus(packet)?;
    let kept: Vec<&Obu<'_>> = obus
        .iter()
        .filter(|obu| !matches!(obu.obu_type, ObuType::TemporalDelimiter | ObuType::Padding))
        .collect();
    if kept.iter().any(|obu| obu.obu_type == ObuType::TileList) {
        return Err(CodecError::MalformedBitstream(
            "tile list OBUs are not allowed",
        ));
    }
    let mut temporal_unit = Vec::with_capacity(packet.len() + sequence_header.len());
    let starts_with_sequence_header = kept
        .first()
        .is_some_and(|obu| obu.obu_type == ObuType::SequenceHeader);
    if is_key_frame && !starts_with_sequence_header {
        temporal_unit.extend_from_slice(sequence_header);
    }
    for obu in kept {
        temporal_unit.extend_from_slice(obu.bytes);
    }
    Ok(temporal_unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns an OBU of `field` type with `payload`.
    fn obu(field: u8, payload: &[u8]) -> Vec<u8> {
        let mut bytes = vec![(field << TYPE_SHIFT) | HAS_SIZE_FLAG, payload.len() as u8];
        bytes.extend_from_slice(payload);
        bytes
    }

    /// OBUs are split by their size fields.
    #[test]
    fn parses_obus() {
        let unit = [obu(2, &[]), obu(1, &[1, 2, 3]), obu(6, &[4; 5])].concat();
        let obus = parse_obus(&unit).unwrap();
        let types: Vec<ObuType> = obus.iter().map(|obu| obu.obu_type).collect();
        assert_eq!(
            types,
            [
                ObuType::TemporalDelimiter,
                ObuType::SequenceHeader,
                ObuType::Frame
            ]
        );
        assert_eq!(obus[1].bytes, [0x0a, 3, 1, 2, 3]);
    }

    /// Temporal delimiters and padding are removed, and a sequence header is
    /// added to key frames that lack one.
    #[test]
    fn converts_to_qdp_form() {
        let sequence_header = obu(1, &[9, 9]);
        let packet = [obu(2, &[]), obu(6, &[4; 3]), obu(15, &[0; 2])].concat();
        let key = to_qdp_temporal_unit(&packet, true, &sequence_header).unwrap();
        assert_eq!(key, [sequence_header.clone(), obu(6, &[4; 3])].concat());
        let delta = to_qdp_temporal_unit(&packet, false, &sequence_header).unwrap();
        assert_eq!(delta, obu(6, &[4; 3]));
        let with_header = [obu(2, &[]), sequence_header.clone(), obu(6, &[1])].concat();
        assert_eq!(
            to_qdp_temporal_unit(&with_header, true, &sequence_header).unwrap(),
            [sequence_header, obu(6, &[1])].concat()
        );
    }

    /// OBUs without size fields, with extensions or overrunning the unit are
    /// rejected.
    #[test]
    fn rejects_malformed_obus() {
        assert!(parse_obus(&[6 << TYPE_SHIFT]).is_err());
        assert!(parse_obus(&[(6 << TYPE_SHIFT) | HAS_SIZE_FLAG | EXTENSION_FLAG, 0]).is_err());
        assert!(parse_obus(&[(6 << TYPE_SHIFT) | HAS_SIZE_FLAG, 5, 0]).is_err());
        assert!(parse_obus(&[FORBIDDEN_BIT | HAS_SIZE_FLAG, 0]).is_err());
    }

    /// Multi-byte sizes are read as LEB128.
    #[test]
    fn leb128() {
        assert_eq!(read_leb128(&[0x05]).unwrap(), (5, 1));
        assert_eq!(read_leb128(&[0x80, 0x01]).unwrap(), (128, 2));
        assert!(read_leb128(&[0x80]).is_err());
    }
}
