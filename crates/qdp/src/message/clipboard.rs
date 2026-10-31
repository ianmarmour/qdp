//! Clipboard offers, requests and rejections (Section 10).

use crate::message::{ClipboardRequestId, MAX_NAME_LENGTH, MessagePayload, OfferId};
use crate::registry::MessageType;
use crate::wire::{Reader, VarInt, WireError, Writer};

/// The largest number of formats in one offer (Section 10.2).
pub const MAX_CLIPBOARD_FORMATS: u64 = 32;

/// The format every endpoint that supports the clipboard must support
/// (Section 10.1).
pub const PLAIN_TEXT_FORMAT: &str = "text/plain;charset=utf-8";

/// A `CLIPBOARD_OFFER` message (Section 10.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardOffer {
    /// Numbered from 1, increasing with each offer the sender makes.
    pub offer_id: OfferId,
    /// The highest Offer ID the sender had received from its peer, or
    /// [`OfferId::NONE`].
    pub peer_offer_id: OfferId,
    /// The media types the sender can provide, at most 32 of at most 255 bytes.
    /// Empty when the sender's clipboard was cleared.
    pub formats: Vec<String>,
}

impl MessagePayload for ClipboardOffer {
    const MESSAGE_TYPE: MessageType = MessageType::CLIPBOARD_OFFER;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.offer_id.0);
        writer.write_varint(self.peer_offer_id.0);
        writer.write_count(self.formats.len());
        for format in &self.formats {
            writer.write_string_at_most(format, MAX_NAME_LENGTH);
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let offer_id = OfferId(reader.read_varint()?);
        let peer_offer_id = OfferId(reader.read_varint()?);
        let count = reader.read_count("Format Count", MAX_CLIPBOARD_FORMATS)?;
        let formats = (0..count)
            .map(|_| {
                reader
                    .read_string_at_most("Format", MAX_NAME_LENGTH)
                    .map(str::to_owned)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            offer_id,
            peer_offer_id,
            formats,
        })
    }
}

/// A `CLIPBOARD_REQUEST` message (Section 10.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardRequest {
    /// Numbered from 1, increasing with each request the sender makes.
    pub request_id: ClipboardRequestId,
    /// The offer being requested from.
    pub offer_id: OfferId,
    /// One format of that offer, copied exactly.
    pub format: String,
}

impl MessagePayload for ClipboardRequest {
    const MESSAGE_TYPE: MessageType = MessageType::CLIPBOARD_REQUEST;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.request_id.0);
        writer.write_varint(self.offer_id.0);
        writer.write_string_at_most(&self.format, MAX_NAME_LENGTH);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            request_id: ClipboardRequestId(reader.read_varint()?),
            offer_id: OfferId(reader.read_varint()?),
            format: reader
                .read_string_at_most("Format", MAX_NAME_LENGTH)?
                .to_owned(),
        })
    }
}

/// Why a clipboard request was refused (Section 10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RejectReason {
    /// The offer has been replaced.
    StaleOffer = 0x01,
    /// The data can no longer be produced, or the format was not offered.
    Unavailable = 0x02,
    /// The data exceeds the requester's Max Size.
    TooLarge = 0x03,
    /// Policy or the user forbids the transfer.
    Denied = 0x04,
}

impl TryFrom<VarInt> for RejectReason {
    type Error = WireError;

    fn try_from(value: VarInt) -> Result<Self, Self::Error> {
        match value.into_inner() {
            0x01 => Ok(Self::StaleOffer),
            0x02 => Ok(Self::Unavailable),
            0x03 => Ok(Self::TooLarge),
            0x04 => Ok(Self::Denied),
            _ => Err(WireError::InvalidValue { field: "Reason" }),
        }
    }
}

/// A `CLIPBOARD_REJECT` message (Section 10.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardReject {
    /// The request being refused.
    pub request_id: ClipboardRequestId,
    /// Why it was refused.
    pub reason: RejectReason,
}

impl MessagePayload for ClipboardReject {
    const MESSAGE_TYPE: MessageType = MessageType::CLIPBOARD_REJECT;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.request_id.0);
        writer.write_varint(VarInt::from(self.reason as u8));
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            request_id: ClipboardRequestId(reader.read_varint()?),
            reason: RejectReason::try_from(reader.read_varint()?)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes `message` and decodes it again.
    fn round_trip<M: MessagePayload>(message: &M) -> Result<M, WireError> {
        let frame = message.to_frame();
        M::decode(&mut Reader::new(&frame.payload))
    }

    /// Clipboard messages round-trip.
    #[test]
    fn clipboard_messages_round_trip() {
        let offer = ClipboardOffer {
            offer_id: OfferId(VarInt::from_u32(2)),
            peer_offer_id: OfferId::NONE,
            formats: vec![PLAIN_TEXT_FORMAT.to_owned(), "image/png".to_owned()],
        };
        assert_eq!(round_trip(&offer).unwrap(), offer);

        let request = ClipboardRequest {
            request_id: ClipboardRequestId(VarInt::from_u32(1)),
            offer_id: offer.offer_id,
            format: PLAIN_TEXT_FORMAT.to_owned(),
        };
        assert_eq!(round_trip(&request).unwrap(), request);

        for reason in [
            RejectReason::StaleOffer,
            RejectReason::Unavailable,
            RejectReason::TooLarge,
            RejectReason::Denied,
        ] {
            let reject = ClipboardReject {
                request_id: request.request_id,
                reason,
            };
            assert_eq!(round_trip(&reject).unwrap(), reject);
        }
    }

    /// More than 32 formats is an error.
    #[test]
    fn too_many_formats() {
        let offer = ClipboardOffer {
            offer_id: OfferId(VarInt::from_u32(1)),
            peer_offer_id: OfferId::NONE,
            formats: vec!["a/b".to_owned(); MAX_CLIPBOARD_FORMATS as usize + 1],
        };
        assert!(matches!(
            round_trip(&offer),
            Err(WireError::LimitExceeded { .. })
        ));
    }
}
