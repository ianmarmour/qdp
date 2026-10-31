//! Cursor images, shapes and warps (Section 9).

use crate::message::{CursorId, ImageSerial, MessagePayload, WarpId};
use crate::registry::MessageType;
use crate::wire::{Reader, SignedVarInt, VarInt, WireError, Writer};

/// The largest cursor image width or height, in pixels (Section 9.2).
pub const MAX_CURSOR_SIZE: u64 = 512;

/// The largest number of Cursor IDs defined at once (Section 9.2).
pub const MAX_CURSOR_IDS: usize = 128;

/// The number of bytes per pixel of the RGBA cursor format.
const RGBA_BYTES_PER_PIXEL: usize = 4;

/// The pixel format of a cursor image (Sections 9.2 and 12.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CursorFormat(pub VarInt);

impl CursorFormat {
    /// 8-bit red, green, blue and alpha per pixel, sRGB, straight alpha.
    pub const RGBA: Self = Self(VarInt::from_u32(0x01));
}

/// A cursor image, the contents of a `CURSOR_IMAGE` stream after its Stream Type
/// (Section 9.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorImage {
    /// The slot the image defines or replaces.
    pub cursor_id: CursorId,
    /// The image's serial, increasing across all Cursor IDs.
    pub image_serial: ImageSerial,
    /// Width in pixels, from 1 to 512.
    pub width: u32,
    /// Height in pixels, from 1 to 512.
    pub height: u32,
    /// The hotspot's column, less than the width.
    pub hotspot_x: u32,
    /// The hotspot's row, less than the height.
    pub hotspot_y: u32,
    /// The interface scale, in percent, the image was drawn for.
    pub scale: VarInt,
    /// RGBA pixels, row by row from the top.
    pub pixels: Vec<u8>,
}

impl CursorImage {
    /// Returns the encoded stream data, without the Stream Type.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut writer = Writer::with_capacity(self.pixels.len() + 32);
        writer.write_varint(self.cursor_id.0);
        writer.write_varint(self.image_serial.0);
        writer.write_varint(self.width.into());
        writer.write_varint(self.height.into());
        writer.write_varint(self.hotspot_x.into());
        writer.write_varint(self.hotspot_y.into());
        writer.write_varint(self.scale);
        writer.write_varint(CursorFormat::RGBA.0);
        writer.write_bytes(&self.pixels);
        writer.into_bytes()
    }

    /// Decodes the complete stream data, without the Stream Type.
    ///
    /// An error means the client aborts the stream with `QDP_PROTOCOL_VIOLATION`
    /// and ignores the image; it is not a connection error.
    pub fn decode(data: &[u8]) -> Result<Self, WireError> {
        let mut reader = Reader::new(data);
        let cursor_id = CursorId(reader.read_varint()?);
        let image_serial = ImageSerial(reader.read_varint()?);
        let width = read_dimension(&mut reader, "Width")?;
        let height = read_dimension(&mut reader, "Height")?;
        let hotspot_x = read_hotspot(&mut reader, "Hotspot X", width)?;
        let hotspot_y = read_hotspot(&mut reader, "Hotspot Y", height)?;
        let scale = reader.read_varint()?;
        if CursorFormat(reader.read_varint()?) != CursorFormat::RGBA {
            return Err(WireError::InvalidValue { field: "Format" });
        }
        if cursor_id == CursorId::HIDDEN {
            return Err(WireError::InvalidValue { field: "Cursor ID" });
        }
        let pixel_len = width as usize * height as usize * RGBA_BYTES_PER_PIXEL;
        let pixels = reader.read_bytes(pixel_len)?.to_vec();
        if !reader.is_empty() {
            return Err(WireError::InvalidValue {
                field: "Pixel Data",
            });
        }
        Ok(Self {
            cursor_id,
            image_serial,
            width,
            height,
            hotspot_x,
            hotspot_y,
            scale,
            pixels,
        })
    }
}

/// Reads a cursor width or height, which must be from 1 to [`MAX_CURSOR_SIZE`].
fn read_dimension(reader: &mut Reader<'_>, field: &'static str) -> Result<u32, WireError> {
    let value = reader.read_varint()?.into_inner();
    if (1..=MAX_CURSOR_SIZE).contains(&value) {
        // The range check above guarantees the value fits.
        Ok(value as u32)
    } else {
        Err(WireError::InvalidValue { field })
    }
}

/// Reads a hotspot coordinate, which must be less than `size`.
fn read_hotspot(reader: &mut Reader<'_>, field: &'static str, size: u32) -> Result<u32, WireError> {
    let value = reader.read_varint()?.into_inner();
    if value < u64::from(size) {
        // The value is below a u32, so it fits.
        Ok(value as u32)
    } else {
        Err(WireError::InvalidValue { field })
    }
}

/// A `CURSOR_SET` message (Section 9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorSet {
    /// The cursor to show, or [`CursorId::HIDDEN`].
    pub cursor_id: CursorId,
    /// The serial of the image meant, or 0 with a hidden cursor.
    pub image_serial: ImageSerial,
}

impl MessagePayload for CursorSet {
    const MESSAGE_TYPE: MessageType = MessageType::CURSOR_SET;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.cursor_id.0);
        writer.write_varint(self.image_serial.0);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            cursor_id: CursorId(reader.read_varint()?),
            image_serial: ImageSerial(reader.read_varint()?),
        })
    }
}

/// The flags of a `CURSOR_WARP` (Section 9.4).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct WarpFlags(pub u8);

impl WarpFlags {
    /// The pointer is driven by a device whose movement the client does not know;
    /// the client draws the cursor there but does not move its own pointer.
    pub const FOLLOW: Self = Self(0x01);

    /// Returns whether every flag in `other` is also in `self`.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// A `CURSOR_WARP` message (Section 9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorWarp {
    /// Numbered from 1, increasing with each warp.
    pub warp_id: WarpId,
    /// The follow flag.
    pub flags: WarpFlags,
    /// The pointer's new horizontal desktop position.
    pub x: SignedVarInt,
    /// The pointer's new vertical desktop position.
    pub y: SignedVarInt,
}

impl MessagePayload for CursorWarp {
    const MESSAGE_TYPE: MessageType = MessageType::CURSOR_WARP;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.warp_id.0);
        writer.write_u8(self.flags.0);
        writer.write_signed_varint(self.x);
        writer.write_signed_varint(self.y);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            warp_id: WarpId(reader.read_varint()?),
            flags: WarpFlags(reader.read_u8()?),
            x: reader.read_signed_varint()?,
            y: reader.read_signed_varint()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a 2 × 1 image with its hotspot on the left pixel.
    fn image() -> CursorImage {
        CursorImage {
            cursor_id: CursorId(VarInt::from_u32(1)),
            image_serial: ImageSerial(VarInt::from_u32(7)),
            width: 2,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            scale: VarInt::from_u32(200),
            pixels: vec![255, 0, 0, 255, 0, 0, 0, 0],
        }
    }

    /// A cursor image round-trips.
    #[test]
    fn cursor_image_round_trip() {
        assert_eq!(CursorImage::decode(&image().to_bytes()).unwrap(), image());
    }

    /// Images that end early or carry extra bytes are errors.
    #[test]
    fn cursor_image_length_mismatch() {
        let bytes = image().to_bytes();
        assert_eq!(
            CursorImage::decode(&bytes[..bytes.len() - 1]),
            Err(WireError::UnexpectedEnd)
        );
        let mut extended = bytes;
        extended.push(0);
        assert_eq!(
            CursorImage::decode(&extended),
            Err(WireError::InvalidValue {
                field: "Pixel Data"
            })
        );
    }

    /// Sizes outside 1 to 512 and hotspots outside the image are errors.
    #[test]
    fn cursor_image_bounds() {
        let oversized = CursorImage {
            width: 513,
            ..image()
        };
        assert_eq!(
            CursorImage::decode(&oversized.to_bytes()),
            Err(WireError::InvalidValue { field: "Width" })
        );
        let bad_hotspot = CursorImage {
            hotspot_x: 2,
            ..image()
        };
        assert_eq!(
            CursorImage::decode(&bad_hotspot.to_bytes()),
            Err(WireError::InvalidValue { field: "Hotspot X" })
        );
        let hidden = CursorImage {
            cursor_id: CursorId::HIDDEN,
            ..image()
        };
        assert_eq!(
            CursorImage::decode(&hidden.to_bytes()),
            Err(WireError::InvalidValue { field: "Cursor ID" })
        );
    }

    /// Cursor messages round-trip.
    #[test]
    fn cursor_messages_round_trip() {
        let set = CursorSet {
            cursor_id: CursorId::HIDDEN,
            image_serial: ImageSerial::default(),
        };
        let frame = set.to_frame();
        assert_eq!(
            CursorSet::decode(&mut Reader::new(&frame.payload)).unwrap(),
            set
        );

        let warp = CursorWarp {
            warp_id: WarpId(VarInt::from_u32(3)),
            flags: WarpFlags::FOLLOW,
            x: SignedVarInt::from(256 * 960),
            y: SignedVarInt::from(-256),
        };
        let frame = warp.to_frame();
        assert_eq!(
            CursorWarp::decode(&mut Reader::new(&frame.payload)).unwrap(),
            warp
        );
    }
}
