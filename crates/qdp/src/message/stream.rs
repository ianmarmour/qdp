//! Headers and framing of unidirectional streams (Sections 3.5.2, 8.8.2, 9.2 and
//! 10.4).

use crate::message::{ClipboardRequestId, DeviceId};
use crate::registry::StreamType;
use crate::wire::{Reader, WireError, Writer};

/// The longest HID output report this implementation accepts, in bytes, including
/// its report ID byte. The specification sets no limit; this bounds the memory a
/// peer can make the receiver buffer (Section 11.7).
pub const MAX_HID_OUTPUT_REPORT_LENGTH: usize = 64 << 10;

/// The header at the start of a unidirectional stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamHeader {
    /// A `CURSOR_IMAGE` stream, whose data is a cursor image (Section 9.2).
    CursorImage,
    /// A `CLIPBOARD_DATA` stream answering a request (Section 10.4).
    ClipboardData {
        /// The request the data answers.
        request_id: ClipboardRequestId,
    },
    /// A `HID_OUTPUT` stream for one passthrough device (Section 8.8.2).
    HidOutput {
        /// The device the output reports are for.
        device_id: DeviceId,
    },
    /// A stream of a type this implementation does not know, which the receiver
    /// aborts or discards.
    Unknown(StreamType),
}

impl StreamHeader {
    /// Returns the Stream Type of this header.
    pub fn stream_type(&self) -> StreamType {
        match self {
            Self::CursorImage => StreamType::CURSOR_IMAGE,
            Self::ClipboardData { .. } => StreamType::CLIPBOARD_DATA,
            Self::HidOutput { .. } => StreamType::HID_OUTPUT,
            Self::Unknown(stream_type) => *stream_type,
        }
    }

    /// Returns the encoded header.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::new();
        writer.write_varint(self.stream_type().0);
        match self {
            Self::ClipboardData { request_id } => writer.write_varint(request_id.0),
            Self::HidOutput { device_id } => writer.write_varint(device_id.0),
            Self::CursorImage | Self::Unknown(_) => {}
        }
        writer.into_bytes()
    }

    /// Decodes the header at the start of a stream's data, returning it and its
    /// length in bytes, or `None` if the data does not yet hold the whole header.
    pub fn decode(prefix: &[u8]) -> Option<(Self, usize)> {
        let mut reader = Reader::new(prefix);
        let stream_type = StreamType(reader.read_varint().ok()?);
        let header = match stream_type {
            StreamType::CURSOR_IMAGE => Self::CursorImage,
            StreamType::CLIPBOARD_DATA => Self::ClipboardData {
                request_id: ClipboardRequestId(reader.read_varint().ok()?),
            },
            StreamType::HID_OUTPUT => Self::HidOutput {
                device_id: DeviceId(reader.read_varint().ok()?),
            },
            _ => Self::Unknown(stream_type),
        };
        Some((header, prefix.len() - reader.remaining()))
    }
}

/// Returns one HID Output Report, framed with its Report Length, for a
/// `HID_OUTPUT` stream (Section 8.8.2).
pub fn encode_hid_output_report(report: &[u8]) -> Vec<u8> {
    let mut writer = Writer::with_capacity(report.len() + 2);
    writer.write_octets(report);
    writer.into_bytes()
}

/// Splits the data of a `HID_OUTPUT` stream, after its header, into output
/// reports.
///
/// The caller passes each chunk read from the stream to
/// [`HidOutputDecoder::push`] and then calls [`HidOutputDecoder::next_report`]
/// until it returns `Ok(None)`.
#[derive(Debug, Clone, Default)]
pub struct HidOutputDecoder {
    /// Received bytes, of which those before `start` have been consumed.
    buffer: Vec<u8>,
    /// The offset in `buffer` of the first byte not yet consumed.
    start: usize,
}

impl HidOutputDecoder {
    /// Creates an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds bytes read from the stream.
    pub fn push(&mut self, data: &[u8]) {
        if self.start > 0 {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        self.buffer.extend_from_slice(data);
    }

    /// Returns the next complete output report, including its report ID byte, or
    /// `Ok(None)` if more bytes are needed.
    ///
    /// A report longer than [`MAX_HID_OUTPUT_REPORT_LENGTH`] is rejected as soon as
    /// its length arrives.
    pub fn next_report(&mut self) -> Result<Option<Vec<u8>>, WireError> {
        let pending = &self.buffer[self.start..];
        let mut reader = Reader::new(pending);
        let length = match reader.read_varint() {
            Ok(length) => length.into_inner(),
            Err(WireError::UnexpectedEnd) => return Ok(None),
            Err(error) => return Err(error),
        };
        if length > MAX_HID_OUTPUT_REPORT_LENGTH as u64 {
            return Err(WireError::LimitExceeded {
                field: "Report Length",
                limit: MAX_HID_OUTPUT_REPORT_LENGTH as u64,
            });
        }
        // The length fits in `usize` because it is below the limit checked above.
        let Ok(report) = reader.read_bytes(length as usize) else {
            return Ok(None);
        };
        let report = report.to_vec();
        self.start += pending.len() - reader.remaining();
        if self.start == self.buffer.len() {
            self.buffer.clear();
            self.start = 0;
        }
        Ok(Some(report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::VarInt;

    /// Every header round-trips, and an incomplete header asks for more data.
    #[test]
    fn stream_headers() {
        let headers = [
            StreamHeader::CursorImage,
            StreamHeader::ClipboardData {
                request_id: ClipboardRequestId(VarInt::from_u32(300)),
            },
            StreamHeader::HidOutput {
                device_id: DeviceId(VarInt::from_u32(4)),
            },
            StreamHeader::Unknown(StreamType(VarInt::from_u32(0x21))),
        ];
        for header in headers {
            let bytes = header.to_bytes();
            assert_eq!(StreamHeader::decode(&bytes), Some((header, bytes.len())));
            assert_eq!(StreamHeader::decode(&bytes[..bytes.len() - 1]), None);
        }
    }

    /// The header length excludes the stream data that follows it.
    #[test]
    fn header_followed_by_data() {
        let mut bytes = StreamHeader::ClipboardData {
            request_id: ClipboardRequestId(VarInt::from_u32(1)),
        }
        .to_bytes();
        bytes.extend_from_slice(b"hello");
        let (_, length) = StreamHeader::decode(&bytes).unwrap();
        assert_eq!(&bytes[length..], b"hello");
    }

    /// Output reports split across chunks are reassembled in order.
    #[test]
    fn hid_output_reports() {
        let mut bytes = encode_hid_output_report(&[0x00, 0x01]);
        bytes.extend(encode_hid_output_report(&[0x02; 100]));
        let mut decoder = HidOutputDecoder::new();
        let mut reports = Vec::new();
        for chunk in bytes.chunks(3) {
            decoder.push(chunk);
            while let Some(report) = decoder.next_report().unwrap() {
                reports.push(report);
            }
        }
        assert_eq!(reports, [vec![0x00, 0x01], vec![0x02; 100]]);
    }

    /// An oversized output report is rejected from its length alone.
    #[test]
    fn oversized_hid_output_report() {
        let mut writer = Writer::new();
        writer.write_varint(VarInt::from_u32(MAX_HID_OUTPUT_REPORT_LENGTH as u32 + 1));
        let mut decoder = HidOutputDecoder::new();
        decoder.push(writer.as_bytes());
        assert!(matches!(
            decoder.next_report(),
            Err(WireError::LimitExceeded { .. })
        ));
    }
}
