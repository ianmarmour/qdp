//! The client's passthrough devices: their input reports, sent as the
//! physical devices produce them (Section 8.3), returned to rest when the
//! session loses focus (Section 8.6, rule 7), and the output reports the host
//! sends them on a stream per device (Section 8.8.2).

use std::collections::{BTreeMap, HashMap};

use qdp::input::{DeviceRest, ReportKind};
use qdp::message::{
    DeviceAttach, DeviceFeatures, DeviceId, DeviceKind, HidOutputDecoder, PassthroughInfo,
    StreamHeader,
};
use qdp_quic::StreamChunk;
use tracing::debug;

use crate::client_input::PendingReport;

/// One passthrough device: its reports at rest and as last sent, by report
/// ID.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Passthrough {
    /// The device.
    device_id: DeviceId,
    /// The reports with every control at rest.
    rest: BTreeMap<u8, Vec<u8>>,
    /// The reports last produced.
    current: BTreeMap<u8, Vec<u8>>,
}

impl Passthrough {
    /// Returns whether any report is away from rest.
    fn holding(&self) -> bool {
        self.current
            .iter()
            .any(|(report_id, report)| self.rest.get(report_id).is_some_and(|rest| rest != report))
    }

    /// Returns a report for the input sender.
    fn pending(&self, report: Vec<u8>) -> PendingReport {
        PendingReport {
            device_id: self.device_id,
            data: report,
            kind: ReportKind::Transition,
            rest: if self.holding() {
                DeviceRest::Active
            } else {
                DeviceRest::AtRest
            },
        }
    }
}

/// The user's passthrough devices, by the slot the platform names them by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PassthroughDevices {
    /// The connected devices.
    devices: HashMap<u32, Passthrough>,
}

impl PassthroughDevices {
    /// Starts tracking a device, returning the `DEVICE_ATTACH` that offers it
    /// to the host.
    pub(crate) fn connect(
        &mut self,
        slot: u32,
        device_id: DeviceId,
        name: String,
        info: PassthroughInfo,
    ) -> DeviceAttach {
        let rest: BTreeMap<u8, Vec<u8>> = info
            .rest_reports
            .iter()
            .filter_map(|report| Some((*report.first()?, report.clone())))
            .collect();
        self.devices.insert(
            slot,
            Passthrough {
                device_id,
                current: rest.clone(),
                rest,
            },
        );
        DeviceAttach {
            device_id,
            kind: DeviceKind::PASSTHROUGH,
            features: DeviceFeatures::default(),
            name,
            touchpad: None,
            passthrough: Some(info),
        }
    }

    /// Stops tracking a device, returning its Device ID to detach.
    pub(crate) fn disconnect(&mut self, slot: u32) -> Option<DeviceId> {
        self.devices.remove(&slot).map(|device| device.device_id)
    }

    /// Returns the slot of a device's Device ID.
    pub(crate) fn slot(&self, device_id: DeviceId) -> Option<u32> {
        self.devices
            .iter()
            .find(|(_, device)| device.device_id == device_id)
            .map(|(&slot, _)| slot)
    }

    /// Returns the report for an input report a device produced, starting
    /// with its report ID byte.
    pub(crate) fn report(&mut self, slot: u32, report: Vec<u8>) -> Option<PendingReport> {
        let device = self.devices.get_mut(&slot)?;
        let &report_id = report.first()?;
        device.current.insert(report_id, report.clone());
        Some(device.pending(report))
    }

    /// Returns the reports a device's state needs once the host accepts it:
    /// those away from rest, since earlier ones were not sent.
    pub(crate) fn state(&self, device_id: DeviceId) -> Vec<PendingReport> {
        let Some(device) = self
            .devices
            .values()
            .find(|device| device.device_id == device_id)
        else {
            return Vec::new();
        };
        device
            .current
            .iter()
            .filter(|(report_id, report)| device.rest.get(report_id) != Some(report))
            .map(|(_, report)| device.pending(report.clone()))
            .collect()
    }

    /// Returns every device's rest reports where it is away from rest, as
    /// when the session loses focus, and leaves the devices at rest.
    pub(crate) fn release_all(&mut self) -> Vec<PendingReport> {
        let mut reports = Vec::new();
        for device in self.devices.values_mut() {
            let away: Vec<Vec<u8>> = device
                .current
                .iter()
                .filter_map(|(report_id, report)| {
                    device
                        .rest
                        .get(report_id)
                        .filter(|rest| *rest != report)
                        .cloned()
                })
                .collect();
            for rest in away {
                device.current.insert(rest[0], rest.clone());
                reports.push(device.pending(rest));
            }
        }
        reports
    }
}

/// The output reports arriving on the host's `HID_OUTPUT` streams, split
/// from each stream's data as it arrives.
#[derive(Debug, Default)]
pub(crate) struct HidOutputs {
    /// Each open stream's device and the data not yet split into reports.
    streams: HashMap<u64, (DeviceId, HidOutputDecoder)>,
}

impl HidOutputs {
    /// Takes data of a followed stream, returning the complete output reports
    /// it carries, each starting with its report ID byte, with their device.
    pub(crate) fn chunk(&mut self, chunk: StreamChunk) -> Vec<(DeviceId, Vec<u8>)> {
        let StreamHeader::HidOutput { device_id } = chunk.header else {
            return Vec::new();
        };
        let (_, decoder) = self
            .streams
            .entry(chunk.stream)
            .or_insert_with(|| (device_id, HidOutputDecoder::new()));
        decoder.push(&chunk.data);
        let mut reports = Vec::new();
        loop {
            match decoder.next_report() {
                Ok(Some(report)) => reports.push((device_id, report)),
                Ok(None) => break,
                Err(error) => {
                    debug!(%error, "malformed output stream dropped");
                    self.streams.remove(&chunk.stream);
                    return reports;
                }
            }
        }
        if chunk.end.is_some() {
            self.streams.remove(&chunk.stream);
        }
        reports
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::{BusType, encode_hid_output_report};
    use qdp::wire::VarInt;

    /// The test device's slot.
    const SLOT: u32 = 3;

    /// The test device's Device ID.
    const DEVICE: DeviceId = DeviceId(VarInt::from_u32(17));

    /// Returns a device with rest reports for report IDs 1 and 2.
    fn connected() -> PassthroughDevices {
        let mut devices = PassthroughDevices::default();
        let info = PassthroughInfo {
            bus_type: BusType::USB,
            vendor_id: 1,
            product_id: 2,
            version: 3,
            report_descriptor: Vec::new(),
            rest_reports: vec![vec![1, 0x80], vec![2, 0x00]],
        };
        let attach = devices.connect(SLOT, DEVICE, "Pad".to_owned(), info);
        assert_eq!(attach.kind, DeviceKind::PASSTHROUGH);
        devices
    }

    /// Reports away from rest keep the device active until every report ID
    /// is back at rest; releasing sends the rest reports needed.
    #[test]
    fn follows_rest() {
        let mut devices = connected();
        assert_eq!(devices.slot(DEVICE), Some(SLOT));
        let moved = devices.report(SLOT, vec![1, 0x20]).unwrap();
        assert_eq!(moved.rest, DeviceRest::Active);
        let pressed = devices.report(SLOT, vec![2, 0x01]).unwrap();
        assert_eq!(pressed.rest, DeviceRest::Active);
        assert_eq!(devices.state(DEVICE).len(), 2);
        let centred = devices.report(SLOT, vec![1, 0x80]).unwrap();
        assert_eq!(centred.rest, DeviceRest::Active);
        let released = devices.release_all();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].data, [2, 0x00]);
        assert_eq!(released[0].rest, DeviceRest::AtRest);
        assert!(devices.release_all().is_empty());
        assert!(devices.report(9, vec![1]).is_none());
        assert_eq!(devices.disconnect(SLOT), Some(DEVICE));
    }

    /// Output reports split across chunks come out whole, and a stream is
    /// forgotten when it ends.
    #[test]
    fn splits_output_streams() {
        let mut outputs = HidOutputs::default();
        let data = [
            encode_hid_output_report(&[5, 1, 2]),
            encode_hid_output_report(&[5, 3]),
        ]
        .concat();
        let chunk = |data: &[u8], end| StreamChunk {
            stream: 2,
            header: StreamHeader::HidOutput { device_id: DEVICE },
            data: data.to_vec(),
            end,
        };
        assert!(outputs.chunk(chunk(&data[..2], None)).is_empty());
        assert_eq!(
            outputs.chunk(chunk(&data[2..], Some(Ok(())))),
            [(DEVICE, vec![5, 1, 2]), (DEVICE, vec![5, 3])]
        );
        assert!(outputs.streams.is_empty());
    }
}
