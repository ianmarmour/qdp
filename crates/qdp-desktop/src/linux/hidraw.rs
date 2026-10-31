//! The client's passthrough devices on Linux, through hidraw (Section
//! 8.2.5): the HID devices the user named, read and written as the kernel
//! sees them, with their report descriptors from sysfs.
//!
//! Reading `/dev/hidraw*` needs permission, which udev rules for game
//! controllers commonly grant the user at the seat.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::Duration;

use qdp::hid::ReportDescriptor;
use qdp::message::{BusType, FeatureOperation, FeatureRequestId, FeatureStatus, PassthroughInfo};
use qdp_runtime::{InputHandle, PassthroughFeedback, UserInput};
use tracing::{debug, info, warn};

use crate::passthrough::DeviceSelector;

/// Where sysfs lists the hidraw devices.
const SYSFS_HIDRAW: &str = "/sys/class/hidraw";

/// Where the hidraw device nodes are.
const DEV: &str = "/dev";

/// The longest report read or written at once.
const MAX_REPORT: usize = 4096;

/// How long a device's thread waits for a report before checking for
/// output and whether to stop.
const POLL_INTERVAL: Duration = Duration::from_millis(4);

/// How long the dispatcher waits for feedback before checking whether to
/// stop.
const FEEDBACK_INTERVAL: Duration = Duration::from_millis(4);

/// The Linux bus type of USB devices.
const LINUX_BUS_USB: u32 = 0x03;

/// The Linux bus type of Bluetooth devices.
const LINUX_BUS_BLUETOOTH: u32 = 0x05;

/// The Linux bus type of I2C devices.
const LINUX_BUS_I2C: u32 = 0x18;

/// The Linux bus type of SPI devices.
const LINUX_BUS_SPI: u32 = 0x1c;

/// The `_IOC_WRITE | _IOC_READ` direction of the feature report ioctls.
const IOC_READ_WRITE: u32 = 3;

/// The type of hidraw's ioctl requests.
const HIDRAW_IOCTL_TYPE: u8 = b'H';

/// The number of `HIDIOCSFEATURE`.
const SET_FEATURE: u8 = 0x06;

/// The number of `HIDIOCGFEATURE`.
const GET_FEATURE: u8 = 0x07;

/// Returns a hidraw feature report ioctl request for a buffer of `length`.
const fn feature_request(number: u8, length: usize) -> libc::Ioctl {
    (IOC_READ_WRITE << 30 | (length as u32) << 16 | (HIDRAW_IOCTL_TYPE as u32) << 8 | number as u32)
        as libc::Ioctl
}

/// How many directories above a USB HID device sysfs puts the USB device:
/// the interface, then the device.
const USB_DEVICE_DEPTH: usize = 2;

/// A hidraw device sysfs lists.
#[derive(Debug, Clone, PartialEq, Eq)]
struct HidrawDevice {
    /// The device node.
    node: PathBuf,
    /// The product name.
    name: String,
    /// The Linux bus type.
    bus: u32,
    /// The vendor ID.
    vendor_id: u16,
    /// The product ID.
    product_id: u16,
    /// The device's version: a USB device's `bcdDevice`, or 0 if unknown.
    version: u16,
    /// The report descriptor.
    descriptor: Vec<u8>,
}

impl HidrawDevice {
    /// Returns the passthrough information the client attaches the device
    /// with: its rest reports come from its descriptor.
    fn info(&self) -> Result<PassthroughInfo, String> {
        let parsed =
            ReportDescriptor::parse(&self.descriptor).map_err(|error| error.to_string())?;
        let bus_type = match self.bus {
            LINUX_BUS_USB => BusType::USB,
            LINUX_BUS_BLUETOOTH => BusType::BLUETOOTH_CLASSIC,
            LINUX_BUS_I2C => BusType::I2C,
            LINUX_BUS_SPI => BusType::SPI,
            _ => BusType::UNKNOWN,
        };
        Ok(PassthroughInfo {
            bus_type,
            vendor_id: self.vendor_id,
            product_id: self.product_id,
            version: self.version,
            report_descriptor: self.descriptor.clone(),
            rest_reports: parsed.rest_reports(),
        })
    }
}

/// Reads one device's entry under `root`, the directory sysfs lists hidraw
/// devices in.
fn read_device(root: &Path, entry: &str) -> Option<HidrawDevice> {
    let device = root.join(entry).join("device");
    let uevent = fs::read_to_string(device.join("uevent")).ok()?;
    let field = |key: &str| {
        uevent
            .lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
    };
    // HID_ID is the bus, vendor and product, in hexadecimal.
    let mut id = field("HID_ID")?.split(':');
    let mut hex = || u32::from_str_radix(id.next()?, 16).ok();
    let (bus, vendor_id, product_id) = (hex()?, hex()?, hex()?);
    Some(HidrawDevice {
        node: Path::new(DEV).join(entry),
        name: field("HID_NAME").unwrap_or(entry).to_owned(),
        bus,
        vendor_id: vendor_id as u16,
        product_id: product_id as u16,
        version: if bus == LINUX_BUS_USB {
            usb_version(&device).unwrap_or(0)
        } else {
            0
        },
        descriptor: fs::read(device.join("report_descriptor")).ok()?,
    })
}

/// Returns a USB HID device's `bcdDevice` (Section 8.2.1), from the USB
/// device sysfs puts its interface, and so the HID device, under.
fn usb_version(device: &Path) -> Option<u16> {
    let device = fs::canonicalize(device).ok()?;
    device
        .ancestors()
        .skip(1)
        .take(USB_DEVICE_DEPTH)
        .find_map(|ancestor| fs::read_to_string(ancestor.join("bcdDevice")).ok())
        .and_then(|text| u16::from_str_radix(text.trim(), 16).ok())
}

/// Returns the hidraw devices under `root` that `selectors` name.
fn find_devices(root: &Path, selectors: &[DeviceSelector]) -> Vec<HidrawDevice> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    names.sort();
    names
        .iter()
        .filter_map(|name| read_device(root, name))
        .filter(|device| {
            selectors
                .iter()
                .any(|selector| selector.matches(device.vendor_id, device.product_id))
        })
        .collect()
}

/// Returns a report read from hidraw with the report ID byte QDP reports
/// start with, which hidraw leaves out for descriptors without report IDs.
fn with_report_id(report: &[u8], uses_report_ids: bool) -> Vec<u8> {
    if uses_report_ids {
        report.to_vec()
    } else {
        [&[0], report].concat()
    }
}

/// Something a device's thread does for the host.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// Writes an output report, starting with its report ID byte, as hidraw
    /// takes it.
    Output(Vec<u8>),
    /// Reads or writes a feature report and answers the request.
    Feature {
        /// The request to answer.
        request_id: FeatureRequestId,
        /// Read or write.
        operation: FeatureOperation,
        /// The report ID byte for a read, or the whole report for a write.
        report: Vec<u8>,
    },
}

/// Performs a feature report read or write, returning the report read.
fn feature(file: &File, operation: FeatureOperation, report: &[u8]) -> io::Result<Vec<u8>> {
    let mut buffer = match operation {
        FeatureOperation::Get => {
            let mut buffer = vec![0; MAX_REPORT];
            buffer[0] = report.first().copied().unwrap_or_default();
            buffer
        }
        FeatureOperation::Set => report.to_vec(),
    };
    let number = match operation {
        FeatureOperation::Get => GET_FEATURE,
        FeatureOperation::Set => SET_FEATURE,
    };
    // SAFETY: the request's length is the buffer's, which the call reads
    // and, for a read, fills.
    let result = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            feature_request(number, buffer.len()),
            buffer.as_mut_ptr(),
        )
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    buffer.truncate(result as usize);
    Ok(match operation {
        FeatureOperation::Get => buffer,
        FeatureOperation::Set => Vec::new(),
    })
}

/// Reads one device and carries out the host's output for it until `stop`
/// is raised, the device goes, or the session ends.
fn run_device(
    slot: u32,
    mut file: File,
    uses_report_ids: bool,
    input: &InputHandle,
    commands: &Receiver<Command>,
    focused: &AtomicBool,
    stop: &AtomicBool,
) {
    let mut buffer = vec![0; MAX_REPORT];
    while !stop.load(Ordering::Relaxed) {
        let mut poll = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `poll` is one valid `pollfd`.
        let ready = unsafe { libc::poll(&mut poll, 1, POLL_INTERVAL.as_millis() as i32) };
        if ready > 0 {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    // Reports are sent only while the window has focus.
                    if focused.load(Ordering::Relaxed) {
                        let report = with_report_id(&buffer[..read], uses_report_ids);
                        if !input.send(UserInput::PassthroughReport { slot, report }) {
                            return;
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    info!(%error, "passthrough device gone");
                    break;
                }
            }
        }
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Output(report) => {
                    if let Err(error) = file.write_all(&report) {
                        debug!(%error, "output report not written");
                    }
                }
                Command::Feature {
                    request_id,
                    operation,
                    report,
                } => {
                    let (status, report) = match feature(&file, operation, &report) {
                        Ok(report) => (FeatureStatus::Ok, report),
                        Err(error) => {
                            debug!(%error, "feature report failed");
                            (FeatureStatus::Failed, Vec::new())
                        }
                    };
                    input.send(UserInput::FeatureResponse {
                        request_id,
                        status,
                        report,
                    });
                }
            }
        }
    }
    input.send(UserInput::PassthroughDisconnected { slot });
}

/// Passes the host's feedback to each device's thread until `stop` is
/// raised.
fn dispatch(input: &InputHandle, devices: &[(u32, Sender<Command>)], stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        let Some(feedback) = input.try_passthrough_feedback() else {
            std::thread::sleep(FEEDBACK_INTERVAL);
            continue;
        };
        let (slot, command) = match feedback {
            PassthroughFeedback::Output { slot, report } => (slot, Command::Output(report)),
            PassthroughFeedback::FeatureRequest {
                slot,
                request_id,
                operation,
                report,
            } => (
                slot,
                Command::Feature {
                    request_id,
                    operation,
                    report,
                },
            ),
        };
        let sent = devices
            .iter()
            .find(|(device_slot, _)| *device_slot == slot)
            .is_some_and(|(_, commands)| commands.send(command.clone()).is_ok());
        if let (false, Command::Feature { request_id, .. }) = (sent, command) {
            input.send(UserInput::FeatureResponse {
                request_id,
                status: FeatureStatus::Failed,
                report: Vec::new(),
            });
        }
    }
}

/// Opens the devices `selectors` name, attaches them, and starts their
/// threads, which stop when `stop` is raised.
pub(crate) fn start(
    input: &InputHandle,
    selectors: &[DeviceSelector],
    focused: &Arc<AtomicBool>,
    stop: &Arc<AtomicBool>,
) -> Vec<JoinHandle<()>> {
    let mut threads = Vec::new();
    let mut routes = Vec::new();
    for (slot, device) in (1..).zip(find_devices(Path::new(SYSFS_HIDRAW), selectors)) {
        let info = match device.info() {
            Ok(info) => info,
            Err(error) => {
                warn!(name = device.name, error, "device not passed through");
                continue;
            }
        };
        let opened = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&device.node);
        let file = match opened {
            Ok(file) => file,
            Err(error) => {
                warn!(node = %device.node.display(), %error, "cannot open the device to pass it through");
                continue;
            }
        };
        let uses_report_ids = ReportDescriptor::parse(&device.descriptor)
            .is_ok_and(|parsed| parsed.uses_report_ids());
        info!(name = device.name, "passing through");
        input.send(UserInput::PassthroughConnected {
            slot,
            name: device.name.clone(),
            info,
        });
        let (commands, received) = mpsc::channel();
        routes.push((slot, commands));
        let (input, focused, stop) = (input.clone(), Arc::clone(focused), Arc::clone(stop));
        let spawned = std::thread::Builder::new()
            .name(format!("qdp-passthrough-{slot}"))
            .spawn(move || {
                run_device(
                    slot,
                    file,
                    uses_report_ids,
                    &input,
                    &received,
                    &focused,
                    &stop,
                );
            });
        match spawned {
            Ok(thread) => threads.push(thread),
            Err(error) => warn!(%error, "cannot read the device"),
        }
    }
    if !routes.is_empty() {
        let (input, stop) = (input.clone(), Arc::clone(stop));
        match std::thread::Builder::new()
            .name("qdp-passthrough".to_owned())
            .spawn(move || dispatch(&input, &routes, &stop))
        {
            Ok(thread) => threads.push(thread),
            Err(error) => warn!(%error, "cannot pass the host's output on"),
        }
    }
    threads
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gamepad's report descriptor, with report IDs.
    const GAMEPAD: &[u8] = &[
        0x05, 0x01, 0x09, 0x05, 0xa1, 0x01, 0x85, 0x01, 0x05, 0x09, 0x19, 0x01, 0x29, 0x08, 0x15,
        0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0xc0,
    ];

    /// Writes a hidraw entry under `root`.
    fn write_entry(root: &Path, name: &str, id: &str, descriptor: &[u8]) {
        let device = root.join(name).join("device");
        fs::create_dir_all(&device).unwrap();
        fs::write(
            device.join("uevent"),
            format!("DRIVER=hid-generic\nHID_ID={id}\nHID_NAME=Test {name}\n"),
        )
        .unwrap();
        fs::write(device.join("report_descriptor"), descriptor).unwrap();
    }

    /// Devices are found by vendor and product ID, with their bus, name,
    /// descriptor and rest reports.
    #[test]
    fn finds_named_devices() {
        let root = std::env::temp_dir().join(format!("qdp-hidraw-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        write_entry(&root, "hidraw0", "0003:0000046D:0000C52B", &[0xc0]);
        write_entry(&root, "hidraw1", "0005:0000054C:000009CC", GAMEPAD);
        let selectors = ["054c:09cc".parse::<DeviceSelector>().unwrap()];
        let devices = find_devices(&root, &selectors);
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(devices.len(), 1);
        let device = &devices[0];
        assert_eq!(device.node, Path::new("/dev/hidraw1"));
        assert_eq!(device.name, "Test hidraw1");
        let info = device.info().unwrap();
        assert_eq!(info.bus_type, BusType::BLUETOOTH_CLASSIC);
        assert_eq!((info.vendor_id, info.product_id), (0x054c, 0x09cc));
        assert_eq!(info.rest_reports, [vec![1, 0]]);
    }

    /// A USB device's version is its `bcdDevice`, found above the HID
    /// device its hidraw entry links to.
    #[test]
    fn reads_usb_versions() {
        let root = std::env::temp_dir().join(format!("qdp-hidraw-usb-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let usb_device = root.join("usb").join("1-2");
        let hid = usb_device.join("1-2:1.0").join("0003:054C:0CE6.0001");
        fs::create_dir_all(&hid).unwrap();
        fs::write(usb_device.join("bcdDevice"), "0110\n").unwrap();
        fs::write(hid.join("uevent"), "HID_ID=0003:0000054C:00000CE6\n").unwrap();
        fs::write(hid.join("report_descriptor"), GAMEPAD).unwrap();
        let entry = root.join("class").join("hidraw2");
        fs::create_dir_all(&entry).unwrap();
        std::os::unix::fs::symlink(&hid, entry.join("device")).unwrap();
        let device = read_device(&root.join("class"), "hidraw2");
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(device.unwrap().info().unwrap().version, 0x0110);
    }

    /// Reports gain the report ID byte that hidraw leaves out, and the
    /// feature ioctls have the numbers of `linux/hidraw.h`.
    #[test]
    fn reads_reports() {
        assert_eq!(with_report_id(&[1, 5], true), [1, 5]);
        assert_eq!(with_report_id(&[5], false), [0, 5]);
        assert_eq!(
            feature_request(GET_FEATURE, 64),
            0xc040_4807_u32 as libc::Ioctl
        );
        assert_eq!(
            feature_request(SET_FEATURE, 64),
            0xc040_4806_u32 as libc::Ioctl
        );
    }
}
