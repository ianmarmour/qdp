//! Input device messages and datagrams (Section 8).

use crate::message::{
    DatagramPayload, DeviceId, DisplayConfigId, FeatureRequestId, InputClasses, InputSequence,
    MAX_NAME_LENGTH, MessagePayload, Timestamp, WarpId,
};
use crate::registry::{DatagramType, MessageType};
use crate::wire::{Reader, SignedVarInt, VarInt, WireError, Writer};

/// The longest passthrough report descriptor, in bytes (Section 8.2.1).
pub const MAX_REPORT_DESCRIPTOR_LENGTH: usize = 16384;

/// The longest passthrough input report, in bytes (Section 8.2.1).
pub const MAX_PASSTHROUGH_REPORT_LENGTH: usize = 1024;

/// The kind of an input device (Sections 8.2.1 and 12.7).
///
/// Unknown kinds are representable, because a host answers them with
/// `UNSUPPORTED` rather than an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DeviceKind(pub VarInt);

impl DeviceKind {
    /// A keyboard.
    pub const KEYBOARD: Self = Self(VarInt::from_u32(0x01));
    /// An absolute pointer.
    pub const POINTER: Self = Self(VarInt::from_u32(0x02));
    /// A relative mouse.
    pub const MOUSE: Self = Self(VarInt::from_u32(0x03));
    /// A touchscreen.
    pub const TOUCHSCREEN: Self = Self(VarInt::from_u32(0x04));
    /// A pen.
    pub const PEN: Self = Self(VarInt::from_u32(0x05));
    /// A touchpad with raw contacts.
    pub const TOUCHPAD: Self = Self(VarInt::from_u32(0x06));
    /// A standard gamepad.
    pub const GAMEPAD: Self = Self(VarInt::from_u32(0x07));
    /// Committed text.
    pub const TEXT: Self = Self(VarInt::from_u32(0x08));
    /// A passthrough HID device.
    pub const PASSTHROUGH: Self = Self(VarInt::from_u32(0x10));

    /// Returns the input class a session must negotiate to use devices of
    /// this kind (Section 4.6.5), or `None` for an unknown kind.
    pub fn input_class(self) -> Option<InputClasses> {
        Some(match self {
            Self::KEYBOARD => InputClasses::KEYBOARD,
            Self::POINTER => InputClasses::POINTER,
            Self::MOUSE => InputClasses::MOUSE,
            Self::TOUCHSCREEN => InputClasses::TOUCHSCREEN,
            Self::PEN => InputClasses::PEN,
            Self::TOUCHPAD => InputClasses::TOUCHPAD,
            Self::GAMEPAD => InputClasses::GAMEPAD,
            Self::TEXT => InputClasses::TEXT,
            Self::PASSTHROUGH => InputClasses::PASSTHROUGH,
            _ => return None,
        })
    }
}

/// The kind-specific features of an input device (Section 8.2.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct DeviceFeatures(pub VarInt);

impl DeviceFeatures {
    /// Touchscreen: contacts report pressure.
    pub const TOUCHSCREEN_PRESSURE: Self = Self(VarInt::from_u32(0x01));
    /// Pen: reports pressure.
    pub const PEN_PRESSURE: Self = Self(VarInt::from_u32(0x01));
    /// Pen: reports tilt.
    pub const PEN_TILT: Self = Self(VarInt::from_u32(0x02));
    /// Pen: reports twist.
    pub const PEN_TWIST: Self = Self(VarInt::from_u32(0x04));
    /// Pen: has an eraser.
    pub const PEN_ERASER: Self = Self(VarInt::from_u32(0x08));
    /// Pen: has a barrel button.
    pub const PEN_BARREL_BUTTON: Self = Self(VarInt::from_u32(0x10));
    /// Touchpad: the surface is a button.
    pub const TOUCHPAD_CLICKPAD: Self = Self(VarInt::from_u32(0x01));
    /// Gamepad: has rumble motors.
    pub const GAMEPAD_RUMBLE: Self = Self(VarInt::from_u32(0x01));
    /// Gamepad: has trigger rumble.
    pub const GAMEPAD_TRIGGER_RUMBLE: Self = Self(VarInt::from_u32(0x02));
    /// Gamepad: has motion sensors.
    pub const GAMEPAD_MOTION_SENSORS: Self = Self(VarInt::from_u32(0x04));
    /// Gamepad: has an LED.
    pub const GAMEPAD_LED: Self = Self(VarInt::from_u32(0x08));
    /// Gamepad: has a PlayStation button layout.
    pub const GAMEPAD_PLAYSTATION_LAYOUT: Self = Self(VarInt::from_u32(0x10));
    /// Gamepad: has a Nintendo button layout.
    pub const GAMEPAD_NINTENDO_LAYOUT: Self = Self(VarInt::from_u32(0x20));

    /// Returns whether every feature in `other` is also in `self`.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns the features in either `self` or `other`.
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// The size of a touchpad's surface, in hundredths of a millimetre (Section 8.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TouchpadInfo {
    /// The width of the surface.
    pub width: VarInt,
    /// The height of the surface.
    pub height: VarInt,
}

/// The transport a passthrough device uses (Section 8.2.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct BusType(pub u8);

impl BusType {
    /// Unknown transport.
    pub const UNKNOWN: Self = Self(0x00);
    /// USB.
    pub const USB: Self = Self(0x01);
    /// Bluetooth Classic.
    pub const BLUETOOTH_CLASSIC: Self = Self(0x02);
    /// Bluetooth Low Energy.
    pub const BLUETOOTH_LE: Self = Self(0x03);
    /// I2C.
    pub const I2C: Self = Self(0x04);
    /// SPI.
    pub const SPI: Self = Self(0x05);
}

/// The description of a passthrough HID device (Section 8.2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassthroughInfo {
    /// The physical device's transport.
    pub bus_type: BusType,
    /// The vendor ID, in the bus's namespace.
    pub vendor_id: u16,
    /// The product ID.
    pub product_id: u16,
    /// The device's version, `bcdDevice` for USB.
    pub version: u16,
    /// The HID report descriptor, at most 16384 bytes.
    pub report_descriptor: Vec<u8>,
    /// One input report per report ID with every control at rest, each preceded by
    /// its report ID byte.
    pub rest_reports: Vec<Vec<u8>>,
}

impl PassthroughInfo {
    /// Appends the fields to `writer`.
    fn encode(&self, writer: &mut Writer) {
        writer.write_u8(self.bus_type.0);
        writer.write_u16(self.vendor_id);
        writer.write_u16(self.product_id);
        writer.write_u16(self.version);
        writer.write_octets(&self.report_descriptor);
        writer.write_count(self.rest_reports.len());
        for report in &self.rest_reports {
            writer.write_octets(report);
        }
    }

    /// Reads the fields from `reader`.
    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let bus_type = BusType(reader.read_u8()?);
        let vendor_id = reader.read_u16()?;
        let product_id = reader.read_u16()?;
        let version = reader.read_u16()?;
        let report_descriptor = reader
            .read_octets_at_most("Report Descriptor", MAX_REPORT_DESCRIPTOR_LENGTH)?
            .to_vec();
        let count = reader.read_count("Rest Report Count", u64::MAX)?;
        let rest_reports = (0..count)
            .map(|_| {
                reader
                    .read_octets_at_most("Rest Report", MAX_PASSTHROUGH_REPORT_LENGTH + 1)
                    .map(<[u8]>::to_vec)
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            bus_type,
            vendor_id,
            product_id,
            version,
            report_descriptor,
            rest_reports,
        })
    }
}

/// A `DEVICE_ATTACH` message (Section 8.2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceAttach {
    /// Chosen by the client, from 1, never reused.
    pub device_id: DeviceId,
    /// The kind of device.
    pub kind: DeviceKind,
    /// Kind-specific features.
    pub features: DeviceFeatures,
    /// The product name, at most 255 bytes.
    pub name: String,
    /// The touch surface size. Present exactly for `TOUCHPAD` devices.
    pub touchpad: Option<TouchpadInfo>,
    /// The HID description. Present exactly for `PASSTHROUGH` devices.
    pub passthrough: Option<PassthroughInfo>,
}

impl MessagePayload for DeviceAttach {
    const MESSAGE_TYPE: MessageType = MessageType::DEVICE_ATTACH;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.device_id.0);
        writer.write_varint(self.kind.0);
        writer.write_varint(self.features.0);
        writer.write_string_at_most(&self.name, MAX_NAME_LENGTH);
        if let Some(touchpad) = self.touchpad {
            writer.write_varint(touchpad.width);
            writer.write_varint(touchpad.height);
        }
        if let Some(passthrough) = &self.passthrough {
            passthrough.encode(writer);
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let device_id = DeviceId(reader.read_varint()?);
        let kind = DeviceKind(reader.read_varint()?);
        let features = DeviceFeatures(reader.read_varint()?);
        let name = reader
            .read_string_at_most("Name", MAX_NAME_LENGTH)?
            .to_owned();

        let both_layouts = DeviceFeatures::GAMEPAD_PLAYSTATION_LAYOUT
            .union(DeviceFeatures::GAMEPAD_NINTENDO_LAYOUT);
        if kind == DeviceKind::GAMEPAD && features.contains(both_layouts) {
            return Err(WireError::InvalidValue { field: "Features" });
        }

        let touchpad = if kind == DeviceKind::TOUCHPAD {
            Some(TouchpadInfo {
                width: reader.read_varint()?,
                height: reader.read_varint()?,
            })
        } else {
            None
        };
        let passthrough = if kind == DeviceKind::PASSTHROUGH {
            Some(PassthroughInfo::decode(reader)?)
        } else {
            None
        };
        Ok(Self {
            device_id,
            kind,
            features,
            name,
            touchpad,
            passthrough,
        })
    }
}

/// A `DEVICE_DETACH` message (Section 8.2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceDetach {
    /// The device being removed.
    pub device_id: DeviceId,
}

impl MessagePayload for DeviceDetach {
    const MESSAGE_TYPE: MessageType = MessageType::DEVICE_DETACH;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.device_id.0);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            device_id: DeviceId(reader.read_varint()?),
        })
    }
}

/// How the Layout of a `KEYBOARD_INFO` is to be read (Section 8.2.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct LayoutPlatform(pub u8);

impl LayoutPlatform {
    /// Unknown.
    pub const UNKNOWN: Self = Self(0x00);
    /// A Windows keyboard layout identifier.
    pub const WINDOWS: Self = Self(0x01);
    /// An XKB layout name.
    pub const XKB: Self = Self(0x02);
    /// A macOS input source identifier.
    pub const MACOS: Self = Self(0x03);
    /// An Android keyboard layout.
    pub const ANDROID: Self = Self(0x04);
    /// An iOS keyboard identifier.
    pub const IOS: Self = Self(0x05);
}

/// A `KEYBOARD_INFO` message (Section 8.2.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardInfo {
    /// The BCP 47 tag of the user's input language.
    pub language: String,
    /// How to read `layout`.
    pub layout_platform: LayoutPlatform,
    /// The client's keyboard layout.
    pub layout: String,
}

impl MessagePayload for KeyboardInfo {
    const MESSAGE_TYPE: MessageType = MessageType::KEYBOARD_INFO;

    fn encode(&self, writer: &mut Writer) {
        writer.write_string(&self.language);
        writer.write_u8(self.layout_platform.0);
        writer.write_string(&self.layout);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            language: reader.read_string()?.to_owned(),
            layout_platform: LayoutPlatform(reader.read_u8()?),
            layout: reader.read_string()?.to_owned(),
        })
    }
}

/// The host's answer to a `DEVICE_ATTACH` (Section 8.2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DeviceStatusCode {
    /// The host will apply the device's reports.
    Accepted = 0x00,
    /// The host cannot support this kind of device.
    Unsupported = 0x01,
    /// Policy forbids this device.
    Denied = 0x02,
    /// The host has as many devices as it allows.
    LimitReached = 0x03,
    /// The host tried and failed to create the device.
    Failed = 0x04,
}

impl TryFrom<u8> for DeviceStatusCode {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Accepted),
            0x01 => Ok(Self::Unsupported),
            0x02 => Ok(Self::Denied),
            0x03 => Ok(Self::LimitReached),
            0x04 => Ok(Self::Failed),
            _ => Err(WireError::InvalidValue { field: "Status" }),
        }
    }
}

/// A `DEVICE_STATUS` message (Section 8.2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceStatus {
    /// The device being answered.
    pub device_id: DeviceId,
    /// Whether the host accepted it.
    pub status: DeviceStatusCode,
    /// A human-readable explanation, at most 255 bytes.
    pub reason: String,
}

impl MessagePayload for DeviceStatus {
    const MESSAGE_TYPE: MessageType = MessageType::DEVICE_STATUS;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.device_id.0);
        writer.write_u8(self.status as u8);
        writer.write_string_at_most(&self.reason, MAX_NAME_LENGTH);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            device_id: DeviceId(reader.read_varint()?),
            status: DeviceStatusCode::try_from(reader.read_u8()?)?,
            reason: reader
                .read_string_at_most("Reason", MAX_NAME_LENGTH)?
                .to_owned(),
        })
    }
}

/// One report in an `INPUT` datagram, borrowing its data (Section 8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputReport<'a> {
    /// The report's sequence number, shared by all devices.
    pub sequence: InputSequence,
    /// The device that produced the report.
    pub device_id: DeviceId,
    /// How many microseconds before the datagram's Timestamp the report was
    /// produced.
    pub time_offset: VarInt,
    /// A standard report, or a passthrough report preceded by its report ID byte.
    pub data: &'a [u8],
}

/// An `INPUT` datagram (Section 8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input<'a> {
    /// When the datagram was sent, on the client's clock.
    pub timestamp: Timestamp,
    /// The lowest sequence number the client still retains.
    pub oldest_retained: InputSequence,
    /// The `DISPLAY_CONFIG` whose layout the positions use.
    pub display_config_id: DisplayConfigId,
    /// The latest `CURSOR_WARP` the client had processed.
    pub warp_id: WarpId,
    /// The reports, in ascending sequence order.
    pub reports: Vec<InputReport<'a>>,
}

impl<'a> DatagramPayload<'a> for Input<'a> {
    const DATAGRAM_TYPE: DatagramType = DatagramType::INPUT;

    fn encode(&self, writer: &mut Writer) {
        self.timestamp.write(writer);
        writer.write_varint(self.oldest_retained.0);
        writer.write_varint(self.display_config_id.0);
        writer.write_varint(self.warp_id.0);
        for report in &self.reports {
            writer.write_varint(report.sequence.0);
            writer.write_varint(report.device_id.0);
            writer.write_varint(report.time_offset);
            writer.write_octets(report.data);
        }
    }

    fn decode(reader: &mut Reader<'a>) -> Result<Self, WireError> {
        let timestamp = Timestamp::read(reader)?;
        let oldest_retained = InputSequence(reader.read_varint()?);
        let display_config_id = DisplayConfigId(reader.read_varint()?);
        let warp_id = WarpId(reader.read_varint()?);
        let mut reports: Vec<InputReport<'a>> = Vec::new();
        while !reader.is_empty() {
            let report = InputReport {
                sequence: InputSequence(reader.read_varint()?),
                device_id: DeviceId(reader.read_varint()?),
                time_offset: reader.read_varint()?,
                data: reader.read_octets()?,
            };
            if reports
                .last()
                .is_some_and(|previous| previous.sequence >= report.sequence)
            {
                return Err(WireError::InvalidValue { field: "Sequence" });
            }
            reports.push(report);
        }
        Ok(Self {
            timestamp,
            oldest_retained,
            display_config_id,
            warp_id,
            reports,
        })
    }
}

/// An `INPUT_ACK` datagram (Section 8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputAck {
    /// The sequence number up to which the host has applied or skipped every
    /// report.
    pub largest_sequence: InputSequence,
}

impl DatagramPayload<'_> for InputAck {
    const DATAGRAM_TYPE: DatagramType = DatagramType::INPUT_ACK;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.largest_sequence.0);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            largest_sequence: InputSequence(reader.read_varint()?),
        })
    }
}

/// A `DEVICE_OUTPUT` datagram, borrowing its report (Section 8.8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceOutput<'a> {
    /// The standard device the output is for.
    pub device_id: DeviceId,
    /// Numbered from 1 per device, increasing with each new output state.
    pub output_sequence: VarInt,
    /// A Keyboard Output or Gamepad Output report.
    pub data: &'a [u8],
}

impl<'a> DatagramPayload<'a> for DeviceOutput<'a> {
    const DATAGRAM_TYPE: DatagramType = DatagramType::DEVICE_OUTPUT;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.device_id.0);
        writer.write_varint(self.output_sequence);
        writer.write_bytes(self.data);
    }

    fn decode(reader: &mut Reader<'a>) -> Result<Self, WireError> {
        Ok(Self {
            device_id: DeviceId(reader.read_varint()?),
            output_sequence: reader.read_varint()?,
            data: reader.read_rest(),
        })
    }
}

/// A feature report operation (Section 8.8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FeatureOperation {
    /// Read a feature report.
    Get = 0x00,
    /// Write a feature report.
    Set = 0x01,
}

impl TryFrom<u8> for FeatureOperation {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Get),
            0x01 => Ok(Self::Set),
            _ => Err(WireError::InvalidValue { field: "Operation" }),
        }
    }
}

/// The outcome of a feature report operation (Section 8.8.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum FeatureStatus {
    /// The operation succeeded.
    Ok = 0x00,
    /// The operation failed.
    Failed = 0x01,
    /// Policy forbids the operation.
    Denied = 0x02,
}

impl TryFrom<u8> for FeatureStatus {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Ok),
            0x01 => Ok(Self::Failed),
            0x02 => Ok(Self::Denied),
            _ => Err(WireError::InvalidValue { field: "Status" }),
        }
    }
}

/// A `HID_FEATURE_REQUEST` message (Section 8.8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidFeatureRequest {
    /// Numbered from 1, increasing with each request.
    pub request_id: FeatureRequestId,
    /// The passthrough device.
    pub device_id: DeviceId,
    /// Read or write.
    pub operation: FeatureOperation,
    /// The report ID byte for `GET`, or the whole feature report for `SET`.
    pub report_data: Vec<u8>,
}

impl MessagePayload for HidFeatureRequest {
    const MESSAGE_TYPE: MessageType = MessageType::HID_FEATURE_REQUEST;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.request_id.0);
        writer.write_varint(self.device_id.0);
        writer.write_u8(self.operation as u8);
        writer.write_octets(&self.report_data);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            request_id: FeatureRequestId(reader.read_varint()?),
            device_id: DeviceId(reader.read_varint()?),
            operation: FeatureOperation::try_from(reader.read_u8()?)?,
            report_data: reader.read_octets()?.to_vec(),
        })
    }
}

/// A `HID_FEATURE_RESPONSE` message (Section 8.8.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidFeatureResponse {
    /// The request being answered.
    pub request_id: FeatureRequestId,
    /// The outcome.
    pub status: FeatureStatus,
    /// The whole feature report for a successful `GET`; otherwise empty.
    pub report_data: Vec<u8>,
}

impl MessagePayload for HidFeatureResponse {
    const MESSAGE_TYPE: MessageType = MessageType::HID_FEATURE_RESPONSE;

    fn encode(&self, writer: &mut Writer) {
        writer.write_varint(self.request_id.0);
        writer.write_u8(self.status as u8);
        writer.write_octets(&self.report_data);
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Self {
            request_id: FeatureRequestId(reader.read_varint()?),
            status: FeatureStatus::try_from(reader.read_u8()?)?,
            report_data: reader.read_octets()?.to_vec(),
        })
    }
}

/// How the client reports pointer input (Section 8.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PointerKind {
    /// The client draws the cursor and reports through the `POINTER` device.
    Absolute = 0x00,
    /// The client captures the pointer and reports through the `MOUSE` device.
    Relative = 0x01,
}

impl TryFrom<u8> for PointerKind {
    type Error = WireError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x00 => Ok(Self::Absolute),
            0x01 => Ok(Self::Relative),
            _ => Err(WireError::InvalidValue { field: "Mode" }),
        }
    }
}

/// The `CONFINED` flag of `POINTER_MODE` (Section 8.9).
const POINTER_MODE_CONFINED: u8 = 0x01;

/// A rectangle of the host desktop, in desktop positions (Section 1.5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopRect {
    /// The left edge.
    pub x: SignedVarInt,
    /// The top edge.
    pub y: SignedVarInt,
    /// The width.
    pub width: VarInt,
    /// The height.
    pub height: VarInt,
}

/// A `POINTER_MODE` message (Section 8.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PointerMode {
    /// Absolute or relative.
    pub mode: PointerKind,
    /// The area the host has confined the pointer to, sent with the `CONFINED`
    /// flag.
    pub confinement: Option<DesktopRect>,
}

impl MessagePayload for PointerMode {
    const MESSAGE_TYPE: MessageType = MessageType::POINTER_MODE;

    fn encode(&self, writer: &mut Writer) {
        writer.write_u8(self.mode as u8);
        match self.confinement {
            Some(area) => {
                writer.write_u8(POINTER_MODE_CONFINED);
                writer.write_signed_varint(area.x);
                writer.write_signed_varint(area.y);
                writer.write_varint(area.width);
                writer.write_varint(area.height);
            }
            None => writer.write_u8(0),
        }
    }

    fn decode(reader: &mut Reader<'_>) -> Result<Self, WireError> {
        let mode = PointerKind::try_from(reader.read_u8()?)?;
        let flags = reader.read_u8()?;
        let confinement = if flags & POINTER_MODE_CONFINED != 0 {
            Some(DesktopRect {
                x: reader.read_signed_varint()?,
                y: reader.read_signed_varint()?,
                width: reader.read_varint()?,
                height: reader.read_varint()?,
            })
        } else {
            None
        };
        Ok(Self { mode, confinement })
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

    /// Returns a device ID.
    fn device(id: u32) -> DeviceId {
        DeviceId(VarInt::from_u32(id))
    }

    /// Every known device kind belongs to the input class of the same name,
    /// and unknown kinds to none.
    #[test]
    fn device_kind_classes() {
        assert_eq!(
            DeviceKind::GAMEPAD.input_class(),
            Some(InputClasses::GAMEPAD)
        );
        assert_eq!(
            DeviceKind::PASSTHROUGH.input_class(),
            Some(InputClasses::PASSTHROUGH)
        );
        assert_eq!(DeviceKind(VarInt::from_u32(0x09)).input_class(), None);
    }

    /// Device attachments of every shape round-trip.
    #[test]
    fn device_attach_round_trip() {
        let keyboard = DeviceAttach {
            device_id: device(1),
            kind: DeviceKind::KEYBOARD,
            features: DeviceFeatures::default(),
            name: "Built-in Keyboard".to_owned(),
            touchpad: None,
            passthrough: None,
        };
        assert_eq!(round_trip(&keyboard).unwrap(), keyboard);

        let touchpad = DeviceAttach {
            device_id: device(2),
            kind: DeviceKind::TOUCHPAD,
            features: DeviceFeatures::TOUCHPAD_CLICKPAD,
            name: "Trackpad".to_owned(),
            touchpad: Some(TouchpadInfo {
                width: VarInt::from_u32(16_000),
                height: VarInt::from_u32(10_000),
            }),
            passthrough: None,
        };
        assert_eq!(round_trip(&touchpad).unwrap(), touchpad);

        let wheel = DeviceAttach {
            device_id: device(3),
            kind: DeviceKind::PASSTHROUGH,
            features: DeviceFeatures::default(),
            name: "Racing Wheel".to_owned(),
            touchpad: None,
            passthrough: Some(PassthroughInfo {
                bus_type: BusType::USB,
                vendor_id: 0x046d,
                product_id: 0xc24f,
                version: 0x0100,
                report_descriptor: vec![0x05, 0x01, 0x09, 0x04, 0xa1, 0x01, 0xc0],
                rest_reports: vec![vec![0x00, 0x80, 0x80], vec![0x02, 0x00]],
            }),
        };
        assert_eq!(round_trip(&wheel).unwrap(), wheel);
    }

    /// A gamepad with both layout flags is an error.
    #[test]
    fn gamepad_with_both_layouts() {
        let gamepad = DeviceAttach {
            device_id: device(4),
            kind: DeviceKind::GAMEPAD,
            features: DeviceFeatures::GAMEPAD_PLAYSTATION_LAYOUT
                .union(DeviceFeatures::GAMEPAD_NINTENDO_LAYOUT),
            name: "Controller".to_owned(),
            touchpad: None,
            passthrough: None,
        };
        assert_eq!(
            round_trip(&gamepad),
            Err(WireError::InvalidValue { field: "Features" })
        );
    }

    /// A report descriptor above 16384 bytes is an error.
    #[test]
    fn oversized_report_descriptor() {
        let device = DeviceAttach {
            device_id: device(5),
            kind: DeviceKind::PASSTHROUGH,
            features: DeviceFeatures::default(),
            name: String::new(),
            touchpad: None,
            passthrough: Some(PassthroughInfo {
                bus_type: BusType::UNKNOWN,
                vendor_id: 0,
                product_id: 0,
                version: 0,
                report_descriptor: vec![0; MAX_REPORT_DESCRIPTOR_LENGTH + 1],
                rest_reports: Vec::new(),
            }),
        };
        assert!(matches!(
            round_trip(&device),
            Err(WireError::LimitExceeded { .. })
        ));
    }

    /// The other device and feature messages round-trip.
    #[test]
    fn device_messages_round_trip() {
        let detach = DeviceDetach {
            device_id: device(3),
        };
        assert_eq!(round_trip(&detach).unwrap(), detach);

        let info = KeyboardInfo {
            language: "en-GB".to_owned(),
            layout_platform: LayoutPlatform::XKB,
            layout: "gb".to_owned(),
        };
        assert_eq!(round_trip(&info).unwrap(), info);

        let status = DeviceStatus {
            device_id: device(3),
            status: DeviceStatusCode::LimitReached,
            reason: "too many devices".to_owned(),
        };
        assert_eq!(round_trip(&status).unwrap(), status);

        let request = HidFeatureRequest {
            request_id: FeatureRequestId(VarInt::from_u32(1)),
            device_id: device(3),
            operation: FeatureOperation::Get,
            report_data: vec![0x05],
        };
        assert_eq!(round_trip(&request).unwrap(), request);

        let response = HidFeatureResponse {
            request_id: FeatureRequestId(VarInt::from_u32(1)),
            status: FeatureStatus::Ok,
            report_data: vec![0x05, 0x01, 0x02],
        };
        assert_eq!(round_trip(&response).unwrap(), response);
    }

    /// Pointer modes round-trip with and without confinement.
    #[test]
    fn pointer_mode_round_trip() {
        let relative = PointerMode {
            mode: PointerKind::Relative,
            confinement: None,
        };
        assert_eq!(relative.to_frame().payload, [0x01, 0x00]);
        assert_eq!(round_trip(&relative).unwrap(), relative);

        let confined = PointerMode {
            mode: PointerKind::Absolute,
            confinement: Some(DesktopRect {
                x: SignedVarInt::from(-256 * 1920),
                y: SignedVarInt::ZERO,
                width: VarInt::from_u32(256 * 1920),
                height: VarInt::from_u32(256 * 1080),
            }),
        };
        assert_eq!(round_trip(&confined).unwrap(), confined);
    }

    /// An `INPUT` datagram round-trips its header and reports.
    #[test]
    fn input_round_trip() {
        let input = Input {
            timestamp: Timestamp(VarInt::from_u32(5_000_000)),
            oldest_retained: InputSequence(VarInt::from_u32(41)),
            display_config_id: DisplayConfigId(VarInt::from_u32(2)),
            warp_id: WarpId::NONE,
            reports: vec![
                InputReport {
                    sequence: InputSequence(VarInt::from_u32(41)),
                    device_id: device(2),
                    time_offset: VarInt::from_u32(900),
                    data: &[0x01; 17],
                },
                InputReport {
                    sequence: InputSequence(VarInt::from_u32(43)),
                    device_id: device(1),
                    time_offset: VarInt::ZERO,
                    data: &[0x00; 40],
                },
            ],
        };
        let bytes = input.to_bytes();
        assert_eq!(bytes[0], 0x04);
        let mut reader = Reader::new(&bytes[1..]);
        assert_eq!(Input::decode(&mut reader).unwrap(), input);
    }

    /// Reports out of sequence order are an error.
    #[test]
    fn input_out_of_order() {
        let report = |sequence| InputReport {
            sequence: InputSequence(VarInt::from_u32(sequence)),
            device_id: device(1),
            time_offset: VarInt::ZERO,
            data: &[],
        };
        let input = Input {
            timestamp: Timestamp::default(),
            oldest_retained: InputSequence(VarInt::from_u32(1)),
            display_config_id: DisplayConfigId(VarInt::from_u32(1)),
            warp_id: WarpId::NONE,
            reports: vec![report(5), report(5)],
        };
        let bytes = input.to_bytes();
        assert_eq!(
            Input::decode(&mut Reader::new(&bytes[1..])),
            Err(WireError::InvalidValue { field: "Sequence" })
        );
    }

    /// `INPUT_ACK` and `DEVICE_OUTPUT` round-trip.
    #[test]
    fn ack_and_output_round_trip() {
        let ack = InputAck {
            largest_sequence: InputSequence(VarInt::from_u32(1000)),
        };
        let bytes = ack.to_bytes();
        assert_eq!(
            InputAck::decode(&mut Reader::new(&bytes[1..])).unwrap(),
            ack
        );

        let output = DeviceOutput {
            device_id: device(1),
            output_sequence: VarInt::from_u32(3),
            data: &[0x02],
        };
        let bytes = output.to_bytes();
        assert_eq!(bytes, [0x06, 0x01, 0x03, 0x02]);
        assert_eq!(
            DeviceOutput::decode(&mut Reader::new(&bytes[1..])).unwrap(),
            output
        );
    }
}
