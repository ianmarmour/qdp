//! The host's side of passthrough devices: which it accepts (Section 8.2.5),
//! the output reports its applications send them on a stream per device
//! (Section 8.8.2), and the feature report requests it relays to the client
//! (Section 8.8.3).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use qdp::hid::{CollectionClass, ReportDescriptor, Usage};
use qdp::message::{
    DeviceId, DeviceStatusCode, FeatureOperation, FeatureRequestId, FeatureStatus,
    HidFeatureRequest, HidFeatureResponse, PassthroughInfo, StreamHeader, encode_hid_output_report,
};
use qdp::wire::VarInt;
use qdp_quic::{OutgoingStream, QdpConnection};
use tracing::debug;

/// How long the host waits for a feature report response before telling the
/// application the operation failed.
const FEATURE_TIMEOUT: Duration = Duration::from_secs(1);

/// Which passthrough devices a host accepts, by the usages of their
/// top-level collections (Section 8.2.5).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PassthroughPolicy {
    /// None.
    Off,
    /// Game controllers, which may also carry vendor-defined collections.
    #[default]
    GameControllers,
    /// Every device, keyboards, mice and authenticators included, as an
    /// administrator may allow.
    Any,
}

impl PassthroughPolicy {
    /// Returns whether the policy accepts a device with these top-level
    /// collections.
    pub fn allows(self, collections: &[Usage]) -> bool {
        let class = |usage: &Usage| usage.class();
        match self {
            Self::Off => false,
            Self::GameControllers => {
                collections
                    .iter()
                    .any(|usage| class(usage) == CollectionClass::GameController)
                    && collections.iter().all(|usage| {
                        matches!(
                            class(usage),
                            CollectionClass::GameController | CollectionClass::VendorDefined
                        )
                    })
            }
            Self::Any => true,
        }
    }
}

/// A passthrough device the host's injector presents to its applications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassthroughDevice {
    /// The device.
    pub device_id: DeviceId,
    /// Its product name.
    pub name: String,
    /// Its bus, IDs, report descriptor and rest reports.
    pub info: PassthroughInfo,
}

/// A feature report operation an application on the host asked of a
/// passthrough device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureRequest {
    /// The device.
    pub device_id: DeviceId,
    /// Identifies the request to the injector when it is answered.
    pub token: u64,
    /// Read or write.
    pub operation: FeatureOperation,
    /// The report ID byte for a read, or the whole report for a write.
    pub report: Vec<u8>,
}

/// Checks a passthrough device against the descriptor's limits and the
/// host's policy, returning the status and reason to refuse it with.
pub(crate) fn check(
    policy: PassthroughPolicy,
    info: &PassthroughInfo,
) -> Result<(), (DeviceStatusCode, String)> {
    let descriptor = ReportDescriptor::parse(&info.report_descriptor).map_err(|error| {
        (
            DeviceStatusCode::Failed,
            format!("invalid report descriptor: {error}"),
        )
    })?;
    if !policy.allows(descriptor.collections()) {
        return Err((
            DeviceStatusCode::Denied,
            "the host accepts only game controllers for passthrough".to_owned(),
        ));
    }
    Ok(())
}

/// A feature request sent to the client and not yet answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pending {
    /// The device.
    device_id: DeviceId,
    /// The injector's token for it.
    token: u64,
    /// When it was sent.
    sent_at: Instant,
}

/// The output streams and pending feature requests of a session's
/// passthrough devices.
#[derive(Debug, Default)]
pub(crate) struct PassthroughLink {
    /// The output stream of each device that has sent output.
    streams: HashMap<DeviceId, OutgoingStream>,
    /// The requests waiting for a response, by Request ID.
    pending: HashMap<u64, Pending>,
    /// The Request ID of the last request.
    last_request_id: u64,
}

impl PassthroughLink {
    /// Sends output reports, each on its device's stream, which the first
    /// report opens.
    pub(crate) fn send_outputs(
        &mut self,
        connection: &QdpConnection,
        outputs: Vec<(DeviceId, Vec<u8>)>,
    ) {
        for (device_id, report) in outputs {
            let stream = self.streams.entry(device_id).or_insert_with(|| {
                connection.open_stream(StreamHeader::HidOutput { device_id }.to_bytes())
            });
            if !stream.send(encode_hid_output_report(&report)) {
                debug!(device = device_id.0.into_inner(), "output report not sent");
            }
        }
    }

    /// Returns the `HID_FEATURE_REQUEST`s for the applications' requests,
    /// remembering them until they are answered.
    pub(crate) fn requests(
        &mut self,
        requests: Vec<FeatureRequest>,
        now: Instant,
    ) -> Vec<HidFeatureRequest> {
        requests
            .into_iter()
            .map(|request| {
                self.last_request_id += 1;
                self.pending.insert(
                    self.last_request_id,
                    Pending {
                        device_id: request.device_id,
                        token: request.token,
                        sent_at: now,
                    },
                );
                HidFeatureRequest {
                    request_id: FeatureRequestId(
                        VarInt::new(self.last_request_id).unwrap_or(VarInt::MAX),
                    ),
                    device_id: request.device_id,
                    operation: request.operation,
                    report_data: request.report,
                }
            })
            .collect()
    }

    /// Returns the token a response answers, with its outcome, or `None` for
    /// an unknown or timed-out Request ID, which is ignored.
    pub(crate) fn response(
        &mut self,
        response: HidFeatureResponse,
    ) -> Option<(u64, FeatureStatus, Vec<u8>)> {
        let pending = self.pending.remove(&response.request_id.0.into_inner())?;
        Some((pending.token, response.status, response.report_data))
    }

    /// Forgets the requests that have waited too long, returning their
    /// tokens, which fail.
    pub(crate) fn expired(&mut self, now: Instant) -> Vec<u64> {
        let expired: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, pending)| now >= pending.sent_at + FEATURE_TIMEOUT)
            .map(|(&request_id, _)| request_id)
            .collect();
        expired
            .into_iter()
            .filter_map(|request_id| self.pending.remove(&request_id))
            .map(|pending| pending.token)
            .collect()
    }

    /// Returns when the oldest pending request times out.
    pub(crate) fn poll_timeout(&self) -> Option<Instant> {
        self.pending
            .values()
            .map(|pending| pending.sent_at + FEATURE_TIMEOUT)
            .min()
    }

    /// Forgets a detached device: finishes its output stream and returns the
    /// tokens of its pending requests, which fail.
    pub(crate) fn remove(&mut self, device_id: DeviceId) -> Vec<u64> {
        self.streams.remove(&device_id);
        let requests: Vec<u64> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.device_id == device_id)
            .map(|(&request_id, _)| request_id)
            .collect();
        requests
            .into_iter()
            .filter_map(|request_id| self.pending.remove(&request_id))
            .map(|pending| pending.token)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::BusType;

    /// The gamepad collection's usage.
    const GAMEPAD: Usage = Usage {
        page: 0x01,
        id: 0x05,
    };

    /// A vendor-defined collection's usage.
    const VENDOR: Usage = Usage {
        page: 0xff00,
        id: 0x01,
    };

    /// The keyboard collection's usage.
    const KEYBOARD: Usage = Usage {
        page: 0x01,
        id: 0x06,
    };

    /// By default only game controllers pass, with or without vendor
    /// collections, and never with a keyboard.
    #[test]
    fn applies_the_policy() {
        let policy = PassthroughPolicy::default();
        assert!(policy.allows(&[GAMEPAD]));
        assert!(policy.allows(&[GAMEPAD, VENDOR]));
        assert!(!policy.allows(&[VENDOR]));
        assert!(!policy.allows(&[GAMEPAD, KEYBOARD]));
        assert!(PassthroughPolicy::Any.allows(&[KEYBOARD]));
        assert!(!PassthroughPolicy::Off.allows(&[GAMEPAD]));
    }

    /// Invalid descriptors fail and keyboards are denied.
    #[test]
    fn checks_devices() {
        let info = |report_descriptor: Vec<u8>| PassthroughInfo {
            bus_type: BusType::USB,
            vendor_id: 1,
            product_id: 2,
            version: 3,
            report_descriptor,
            rest_reports: Vec::new(),
        };
        let policy = PassthroughPolicy::default();
        let keyboard = info(vec![0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0xc0]);
        assert_eq!(
            check(policy, &keyboard).unwrap_err().0,
            DeviceStatusCode::Denied
        );
        let broken = info(vec![0xa1, 0x01]);
        assert_eq!(
            check(policy, &broken).unwrap_err().0,
            DeviceStatusCode::Failed
        );
        let gamepad = info(vec![0x05, 0x01, 0x09, 0x05, 0xa1, 0x01, 0xc0]);
        assert_eq!(check(policy, &gamepad), Ok(()));
    }

    /// Requests are numbered from 1, answered once, time out after a second,
    /// and fail when their device goes.
    #[test]
    fn tracks_feature_requests() {
        let mut link = PassthroughLink::default();
        let now = Instant::now();
        let device = DeviceId(VarInt::from_u32(20));
        let request = |token| FeatureRequest {
            device_id: device,
            token,
            operation: FeatureOperation::Get,
            report: vec![3],
        };
        let sent = link.requests(vec![request(70), request(71)], now);
        assert_eq!(sent[0].request_id, FeatureRequestId(VarInt::from_u32(1)));
        assert_eq!(sent[1].request_id, FeatureRequestId(VarInt::from_u32(2)));
        let answer = HidFeatureResponse {
            request_id: sent[0].request_id,
            status: FeatureStatus::Ok,
            report_data: vec![3, 9],
        };
        assert_eq!(
            link.response(answer.clone()),
            Some((70, FeatureStatus::Ok, vec![3, 9]))
        );
        assert_eq!(link.response(answer), None);
        assert_eq!(link.poll_timeout(), Some(now + FEATURE_TIMEOUT));
        assert!(link.expired(now).is_empty());
        assert_eq!(link.expired(now + FEATURE_TIMEOUT), [71]);
        link.requests(vec![request(72)], now);
        assert_eq!(link.remove(device), [72]);
        assert_eq!(link.poll_timeout(), None);
    }
}
