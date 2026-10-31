//! Passthrough devices through uhid, the kernel's interface for HID devices
//! implemented in user space (Section 8.2.5).
//!
//! Each device is created from the client's report descriptor and receives
//! its input reports; the kernel's drivers and applications write output
//! reports and read and write feature reports, which come back to this
//! process as events to forward or answer. The kernel waits up to five
//! seconds for an answer, so requests the client must answer are queued and
//! answered when its response arrives.
//!
//! The vendor and product IDs are passed on only for well-known game
//! controllers, so that untrusted IDs cannot select vendor-specific kernel
//! drivers; other devices get generic IDs, which the generic HID driver
//! takes.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;

use qdp::hid::{ReportDescriptor, ReportType};
use qdp::message::{BusType, FeatureOperation, FeatureStatus};
use qdp_runtime::PassthroughDevice;
use tracing::debug;

/// The uhid device node.
pub(super) const UHID_PATH: &str = "/dev/uhid";

/// The size of `struct uhid_event`: its type, then its largest member,
/// `struct uhid_create2_req`.
const EVENT_SIZE: usize = 4 + 4372;

/// The most bytes a report carries (`UHID_DATA_MAX`).
const DATA_MAX: usize = 4096;

/// The longest report descriptor uhid takes (`HID_MAX_DESCRIPTOR_SIZE`).
const MAX_DESCRIPTOR: usize = 4096;

/// The size of `create2`'s name field.
const NAME_SIZE: usize = 128;

/// The size of `create2`'s phys and uniq fields.
const PHYS_SIZE: usize = 64;

/// `UHID_DESTROY`.
const UHID_DESTROY: u32 = 1;

/// `UHID_OUTPUT`: an output report to send to the device.
const UHID_OUTPUT: u32 = 6;

/// `UHID_GET_REPORT`: a report to read from the device.
const UHID_GET_REPORT: u32 = 9;

/// `UHID_GET_REPORT_REPLY`.
const UHID_GET_REPORT_REPLY: u32 = 10;

/// `UHID_CREATE2`.
const UHID_CREATE2: u32 = 11;

/// `UHID_INPUT2`: an input report from the device.
const UHID_INPUT2: u32 = 12;

/// `UHID_SET_REPORT`: a report to write to the device.
const UHID_SET_REPORT: u32 = 13;

/// `UHID_SET_REPORT_REPLY`.
const UHID_SET_REPORT_REPLY: u32 = 14;

/// `UHID_FEATURE_REPORT`.
const UHID_FEATURE_REPORT: u8 = 0;

/// `UHID_OUTPUT_REPORT`.
const UHID_OUTPUT_REPORT: u8 = 1;

/// `UHID_INPUT_REPORT`.
const UHID_INPUT_REPORT: u8 = 2;

/// The error a failed request is answered with.
const EIO: u16 = libc::EIO as u16;

/// `BUS_USB`.
const BUS_USB: u16 = 0x03;

/// `BUS_BLUETOOTH`, for both Bluetooth Classic and LE.
const BUS_BLUETOOTH: u16 = 0x05;

/// `BUS_VIRTUAL`, for unknown transports.
const BUS_VIRTUAL: u16 = 0x06;

/// `BUS_I2C`.
const BUS_I2C: u16 = 0x18;

/// `BUS_SPI`.
const BUS_SPI: u16 = 0x1c;

/// The game controllers whose vendor and product IDs are passed on, for
/// their kernel drivers' rumble, lights and motion sensors.
const ALLOWED_IDS: [(u16, u16); 20] = [
    (0x054c, 0x0268), // Sony DualShock 3
    (0x054c, 0x05c4), // Sony DualShock 4
    (0x054c, 0x09cc), // Sony DualShock 4, second generation
    (0x054c, 0x0ba0), // Sony DualShock 4 USB wireless adaptor
    (0x054c, 0x0ce6), // Sony DualSense
    (0x054c, 0x0df2), // Sony DualSense Edge
    (0x057e, 0x2006), // Nintendo Joy-Con (L)
    (0x057e, 0x2007), // Nintendo Joy-Con (R)
    (0x057e, 0x2009), // Nintendo Switch Pro Controller
    (0x057e, 0x2017), // Nintendo SNES Controller
    (0x045e, 0x02e0), // Xbox Wireless Controller, Bluetooth
    (0x045e, 0x02fd), // Xbox Wireless Controller, Bluetooth
    (0x045e, 0x0b05), // Xbox Elite Wireless Controller Series 2, Bluetooth
    (0x045e, 0x0b13), // Xbox Wireless Controller (Series), Bluetooth
    (0x045e, 0x0b20), // Xbox Wireless Controller (Series), Bluetooth LE
    (0x045e, 0x0b22), // Xbox Elite Wireless Controller Series 2, Bluetooth LE
    (0x28de, 0x1102), // Valve Steam Controller
    (0x28de, 0x1142), // Valve Steam Controller wireless receiver
    (0x28de, 0x1205), // Valve Steam Deck controls
    (0x2dc8, 0x6101), // 8BitDo SN30 Pro
];

/// Returns the kernel's bus type for a QDP bus type.
fn kernel_bus(bus: BusType) -> u16 {
    match bus {
        BusType::USB => BUS_USB,
        BusType::BLUETOOTH_CLASSIC | BusType::BLUETOOTH_LE => BUS_BLUETOOTH,
        BusType::I2C => BUS_I2C,
        BusType::SPI => BUS_SPI,
        _ => BUS_VIRTUAL,
    }
}

/// Returns an empty event of `kind`.
fn event(kind: u32) -> Vec<u8> {
    let mut bytes = vec![0; EVENT_SIZE];
    bytes[..4].copy_from_slice(&kind.to_ne_bytes());
    bytes
}

/// Copies `text` into a fixed-size, NUL-terminated field.
fn copy_text(field: &mut [u8], text: &str) {
    let length = text.len().min(field.len() - 1);
    field[..length].copy_from_slice(&text.as_bytes()[..length]);
}

/// Returns the `UHID_CREATE2` event for a device, or why uhid cannot create
/// it.
fn create_event(device: &PassthroughDevice) -> Result<Vec<u8>, String> {
    let info = &device.info;
    if info.report_descriptor.len() > MAX_DESCRIPTOR {
        return Err(format!(
            "uhid takes report descriptors of at most {MAX_DESCRIPTOR} bytes"
        ));
    }
    let (vendor, product) = if ALLOWED_IDS.contains(&(info.vendor_id, info.product_id)) {
        (info.vendor_id, info.product_id)
    } else {
        (0, 0)
    };
    let mut bytes = event(UHID_CREATE2);
    let request = &mut bytes[4..];
    copy_text(&mut request[..NAME_SIZE], &device.name);
    copy_text(&mut request[NAME_SIZE..NAME_SIZE + PHYS_SIZE], "qdp");
    let mut offset = NAME_SIZE + 2 * PHYS_SIZE;
    let mut put = |value: &[u8]| {
        request[offset..offset + value.len()].copy_from_slice(value);
        offset += value.len();
    };
    put(&(info.report_descriptor.len() as u16).to_ne_bytes());
    put(&kernel_bus(info.bus_type).to_ne_bytes());
    put(&u32::from(vendor).to_ne_bytes());
    put(&u32::from(product).to_ne_bytes());
    put(&u32::from(info.version).to_ne_bytes());
    put(&0_u32.to_ne_bytes()); // Country
    put(&info.report_descriptor);
    Ok(bytes)
}

/// Returns the `UHID_INPUT2` event delivering an input report.
fn input_event(report: &[u8]) -> Vec<u8> {
    let report = &report[..report.len().min(DATA_MAX)];
    let mut bytes = event(UHID_INPUT2);
    bytes[4..6].copy_from_slice(&(report.len() as u16).to_ne_bytes());
    bytes[6..6 + report.len()].copy_from_slice(report);
    bytes
}

/// Returns the `UHID_GET_REPORT_REPLY` event answering request `id`.
fn get_reply_event(id: u32, error: u16, report: &[u8]) -> Vec<u8> {
    let report = &report[..report.len().min(DATA_MAX)];
    let mut bytes = event(UHID_GET_REPORT_REPLY);
    bytes[4..8].copy_from_slice(&id.to_ne_bytes());
    bytes[8..10].copy_from_slice(&error.to_ne_bytes());
    bytes[10..12].copy_from_slice(&(report.len() as u16).to_ne_bytes());
    bytes[12..12 + report.len()].copy_from_slice(report);
    bytes
}

/// Returns the `UHID_SET_REPORT_REPLY` event answering request `id`.
fn set_reply_event(id: u32, error: u16) -> Vec<u8> {
    let mut bytes = event(UHID_SET_REPORT_REPLY);
    bytes[4..8].copy_from_slice(&id.to_ne_bytes());
    bytes[8..10].copy_from_slice(&error.to_ne_bytes());
    bytes
}

/// An event the kernel sent about a device.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KernelEvent {
    /// An output report to send to the device.
    Output {
        /// The report.
        data: Vec<u8>,
    },
    /// A report to read from the device, to answer with request `id`.
    GetReport {
        /// The request.
        id: u32,
        /// The report ID.
        number: u8,
        /// Whether a feature, output or input report.
        kind: u8,
    },
    /// A report to write to the device, to answer with request `id`.
    SetReport {
        /// The request.
        id: u32,
        /// Whether a feature or output report.
        kind: u8,
        /// The report.
        data: Vec<u8>,
    },
    /// Anything else, such as the device starting or being opened.
    Other,
}

/// Returns the bytes from `start` of `length`, at most [`DATA_MAX`].
fn data(bytes: &[u8], start: usize, length: u16) -> Vec<u8> {
    let end = (start + usize::from(length).min(DATA_MAX)).min(bytes.len());
    bytes.get(start..end).unwrap_or_default().to_vec()
}

/// Reads one `struct uhid_event`.
fn parse_event(bytes: &[u8]) -> Option<KernelEvent> {
    let u32_at = |offset: usize| -> Option<u32> {
        Some(u32::from_ne_bytes(
            bytes.get(offset..offset + 4)?.try_into().ok()?,
        ))
    };
    let u16_at = |offset: usize| -> Option<u16> {
        Some(u16::from_ne_bytes(
            bytes.get(offset..offset + 2)?.try_into().ok()?,
        ))
    };
    Some(match u32_at(0)? {
        UHID_OUTPUT => {
            // struct uhid_output_req: data, then size and report type.
            let size = u16_at(4 + DATA_MAX)?;
            KernelEvent::Output {
                data: data(bytes, 4, size),
            }
        }
        UHID_GET_REPORT => KernelEvent::GetReport {
            id: u32_at(4)?,
            number: *bytes.get(8)?,
            kind: *bytes.get(9)?,
        },
        UHID_SET_REPORT => KernelEvent::SetReport {
            id: u32_at(4)?,
            kind: *bytes.get(9)?,
            data: data(bytes, 12, u16_at(10)?),
        },
        _ => KernelEvent::Other,
    })
}

/// A request the client answers: a feature report read or write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Forwarded {
    /// The kernel's request ID.
    id: u32,
    /// Read or write.
    operation: FeatureOperation,
}

impl Forwarded {
    /// Returns whether the request reads or writes.
    pub(super) fn operation(self) -> FeatureOperation {
        self.operation
    }
}

/// What a device's kernel events call for.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Requests {
    /// Output reports to send, starting with their report ID byte.
    pub(super) outputs: Vec<Vec<u8>>,
    /// Feature report operations for the client, with the report ID byte
    /// for a read or the whole report for a write.
    pub(super) features: Vec<(Forwarded, Vec<u8>)>,
}

/// A passthrough device created through uhid.
#[derive(Debug)]
pub(super) struct UhidDevice {
    /// The device's uhid file; closing it removes the device.
    file: File,
    /// The device's report layouts.
    descriptor: ReportDescriptor,
    /// The input reports last delivered, by report ID, for reads of input
    /// reports.
    inputs: BTreeMap<u8, Vec<u8>>,
}

impl UhidDevice {
    /// Creates a device.
    pub(super) fn create(device: &PassthroughDevice) -> Result<Self, String> {
        let descriptor = ReportDescriptor::parse(&device.info.report_descriptor)
            .map_err(|error| error.to_string())?;
        let create = create_event(device)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(UHID_PATH)
            .map_err(|error| format!("cannot open {UHID_PATH}: {error}"))?;
        file.write_all(&create)
            .map_err(|error| format!("cannot create the device: {error}"))?;
        Ok(Self {
            file,
            descriptor,
            inputs: device
                .info
                .rest_reports
                .iter()
                .filter_map(|report| Some((*report.first()?, report.clone())))
                .collect(),
        })
    }

    /// Delivers an input report, starting with its report ID byte.
    pub(super) fn input(&mut self, report: &[u8]) -> io::Result<()> {
        let Some(&report_id) = report.first() else {
            return Ok(());
        };
        self.inputs.insert(report_id, report.to_vec());
        self.file
            .write_all(&input_event(to_kernel(&self.descriptor, report)))
    }

    /// Answers the kernel's requests that need no client, and returns those
    /// that do, with the output reports to send.
    pub(super) fn requests(&mut self) -> Requests {
        let mut requests = Requests::default();
        let mut buffer = vec![0; EVENT_SIZE];
        while let Ok(read) = self.file.read(&mut buffer) {
            if read == 0 {
                break;
            }
            let Some(event) = parse_event(&buffer[..read]) else {
                continue;
            };
            let reply = match event {
                KernelEvent::Output { data } => {
                    requests
                        .outputs
                        .push(from_kernel(&self.descriptor, &data, ReportType::Output));
                    None
                }
                KernelEvent::SetReport {
                    id,
                    kind: UHID_OUTPUT_REPORT,
                    data,
                } => {
                    requests
                        .outputs
                        .push(from_kernel(&self.descriptor, &data, ReportType::Output));
                    Some(set_reply_event(id, 0))
                }
                KernelEvent::SetReport {
                    id,
                    kind: UHID_FEATURE_REPORT,
                    data,
                } => {
                    let operation = FeatureOperation::Set;
                    let report = from_kernel(&self.descriptor, &data, ReportType::Feature);
                    requests
                        .features
                        .push((Forwarded { id, operation }, report));
                    None
                }
                KernelEvent::GetReport {
                    id,
                    number,
                    kind: UHID_FEATURE_REPORT,
                } => {
                    let operation = FeatureOperation::Get;
                    requests
                        .features
                        .push((Forwarded { id, operation }, vec![number]));
                    None
                }
                KernelEvent::GetReport {
                    id,
                    number,
                    kind: UHID_INPUT_REPORT,
                } => Some(match self.inputs.get(&number) {
                    Some(report) => get_reply_event(id, 0, report),
                    None => get_reply_event(id, EIO, &[]),
                }),
                KernelEvent::GetReport { id, .. } => Some(get_reply_event(id, EIO, &[])),
                KernelEvent::SetReport { id, .. } => Some(set_reply_event(id, EIO)),
                KernelEvent::Other => None,
            };
            if let Some(reply) = reply {
                self.reply(&reply);
            }
        }
        requests
    }

    /// Answers a forwarded request with the client's outcome.
    pub(super) fn answer(&mut self, request: Forwarded, status: FeatureStatus, report: &[u8]) {
        let error = if status == FeatureStatus::Ok { 0 } else { EIO };
        let reply = match request.operation {
            // Reads return the report with its report ID byte, as the
            // kernel's raw requests carry it.
            FeatureOperation::Get => get_reply_event(request.id, error, report),
            FeatureOperation::Set => set_reply_event(request.id, error),
        };
        self.reply(&reply);
    }

    /// Writes a reply, which the kernel times out on if it is lost.
    fn reply(&mut self, reply: &[u8]) {
        if let Err(error) = self.file.write_all(reply) {
            debug!(%error, "uhid reply lost");
        }
    }
}

/// Returns a QDP report, which always starts with a report ID byte, as uhid
/// takes input: without the byte when the descriptor uses no report IDs.
fn to_kernel<'a>(descriptor: &ReportDescriptor, report: &'a [u8]) -> &'a [u8] {
    if descriptor.uses_report_ids() {
        report
    } else {
        &report[1..]
    }
}

/// Returns a report from the kernel with the report ID byte QDP reports
/// start with: the kernel gives it unless the descriptor uses no report IDs,
/// when some paths give a 0 and others nothing.
fn from_kernel(descriptor: &ReportDescriptor, data: &[u8], kind: ReportType) -> Vec<u8> {
    if descriptor.uses_report_ids() {
        return data.to_vec();
    }
    let length = descriptor.report_length(kind, 0);
    if length.is_some_and(|length| data.len() == length + 1) && data.first() == Some(&0) {
        data.to_vec()
    } else {
        [&[0], data].concat()
    }
}

impl Drop for UhidDevice {
    fn drop(&mut self) {
        // Closing the file destroys the device too; asking first lets the
        // kernel remove it cleanly.
        let _ = self.file.write_all(&event(UHID_DESTROY));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::{DeviceId, PassthroughInfo};
    use qdp::wire::VarInt;

    /// A gamepad without report IDs: one byte of buttons in, two bytes of
    /// output.
    const UNNUMBERED: &[u8] = &[
        0x05, 0x01, 0x09, 0x05, 0xa1, 0x01, // Gamepad
        0x05, 0x09, 0x19, 0x01, 0x29, 0x08, 0x15, 0x00, 0x25, 0x01, // 8 buttons
        0x75, 0x01, 0x95, 0x08, 0x81, 0x02, // Input
        0x06, 0x00, 0xff, 0x09, 0x01, 0x75, 0x08, 0x95, 0x02, 0x91, 0x02, // Output
        0xc0,
    ];

    /// Returns a passthrough device with `vendor` and `product`.
    fn device(vendor_id: u16, product_id: u16) -> PassthroughDevice {
        PassthroughDevice {
            device_id: DeviceId(VarInt::from_u32(16)),
            name: "Wireless Controller".to_owned(),
            info: PassthroughInfo {
                bus_type: BusType::BLUETOOTH_CLASSIC,
                vendor_id,
                product_id,
                version: 0x0100,
                report_descriptor: UNNUMBERED.to_vec(),
                rest_reports: vec![vec![0, 0]],
            },
        }
    }

    /// The creation event lays out `struct uhid_create2_req`, passing IDs on
    /// only for allowed controllers.
    #[test]
    fn creates_devices() {
        let bytes = create_event(&device(0x054c, 0x09cc)).unwrap();
        assert_eq!(bytes.len(), EVENT_SIZE);
        assert_eq!(bytes[..4], UHID_CREATE2.to_ne_bytes());
        assert_eq!(&bytes[4..23], b"Wireless Controller");
        assert_eq!(&bytes[4 + 128..4 + 131], b"qdp");
        let fields = 4 + 256;
        assert_eq!(
            bytes[fields..fields + 2],
            (UNNUMBERED.len() as u16).to_ne_bytes()
        );
        assert_eq!(bytes[fields + 2..fields + 4], BUS_BLUETOOTH.to_ne_bytes());
        assert_eq!(bytes[fields + 4..fields + 8], 0x054c_u32.to_ne_bytes());
        assert_eq!(bytes[fields + 8..fields + 12], 0x09cc_u32.to_ne_bytes());
        assert_eq!(
            bytes[fields + 20..fields + 20 + UNNUMBERED.len()],
            *UNNUMBERED
        );
        let unknown = create_event(&device(0x1234, 0x5678)).unwrap();
        assert_eq!(unknown[fields + 4..fields + 12], [0; 8]);
        let mut huge = device(1, 2);
        huge.info.report_descriptor = vec![0; MAX_DESCRIPTOR + 1];
        assert!(create_event(&huge).is_err());
    }

    /// Kernel events are read from their `struct uhid_event` layouts.
    #[test]
    fn reads_kernel_events() {
        let mut output = event(UHID_OUTPUT);
        output[4..7].copy_from_slice(&[0, 0x10, 0x20]);
        output[4 + DATA_MAX..4 + DATA_MAX + 2].copy_from_slice(&3_u16.to_ne_bytes());
        assert_eq!(
            parse_event(&output),
            Some(KernelEvent::Output {
                data: vec![0, 0x10, 0x20]
            })
        );
        let mut get = event(UHID_GET_REPORT);
        get[4..8].copy_from_slice(&7_u32.to_ne_bytes());
        get[8] = 3;
        get[9] = UHID_FEATURE_REPORT;
        assert_eq!(
            parse_event(&get),
            Some(KernelEvent::GetReport {
                id: 7,
                number: 3,
                kind: UHID_FEATURE_REPORT
            })
        );
        let mut set = event(UHID_SET_REPORT);
        set[4..8].copy_from_slice(&8_u32.to_ne_bytes());
        set[9] = UHID_FEATURE_REPORT;
        set[10..12].copy_from_slice(&2_u16.to_ne_bytes());
        set[12..14].copy_from_slice(&[3, 0x55]);
        assert_eq!(
            parse_event(&set),
            Some(KernelEvent::SetReport {
                id: 8,
                kind: UHID_FEATURE_REPORT,
                data: vec![3, 0x55]
            })
        );
        assert_eq!(parse_event(&event(4)), Some(KernelEvent::Other));
        assert_eq!(parse_event(&[1, 2]), None);
    }

    /// Replies and input reports lay out their requests.
    #[test]
    fn writes_replies() {
        let reply = get_reply_event(9, 0, &[3, 0x55]);
        assert_eq!(reply[4..8], 9_u32.to_ne_bytes());
        assert_eq!(reply[10..12], 2_u16.to_ne_bytes());
        assert_eq!(reply[12..14], [3, 0x55]);
        let reply = set_reply_event(9, EIO);
        assert_eq!(reply[8..10], EIO.to_ne_bytes());
        let input = input_event(&[0x01, 0x02]);
        assert_eq!(input[4..6], 2_u16.to_ne_bytes());
        assert_eq!(input[6..8], [0x01, 0x02]);
        assert_eq!(kernel_bus(BusType::USB), BUS_USB);
        assert_eq!(kernel_bus(BusType::UNKNOWN), BUS_VIRTUAL);
    }

    /// Reports gain the report ID byte of a descriptor without report IDs
    /// once, however the kernel gave them, and lose it again for input.
    #[test]
    fn converts_report_ids() {
        let descriptor = ReportDescriptor::parse(UNNUMBERED).unwrap();
        assert_eq!(to_kernel(&descriptor, &[0, 0x05]), [0x05]);
        let output = ReportType::Output;
        assert_eq!(
            from_kernel(&descriptor, &[0x10, 0x20], output),
            [0, 0x10, 0x20]
        );
        assert_eq!(
            from_kernel(&descriptor, &[0, 0x10, 0x20], output),
            [0, 0x10, 0x20]
        );
    }
}
