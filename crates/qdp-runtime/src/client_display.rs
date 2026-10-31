//! The client's display requests: the first during configuration, then one
//! whenever the display changes, at most one every 250 ms while it keeps
//! changing and one once it stops (Section 6.2).

use std::time::{Duration, Instant};

use qdp::message::{
    ClientMonitor, ClientMonitorFlags, ClientMonitorId, DisplayRequest, DisplayRequestId,
};
use qdp::wire::{SignedVarInt, VarInt};

use crate::client::DisplaySize;

/// The shortest time between two `DISPLAY_REQUEST`s (Section 6.2).
const REQUEST_INTERVAL: Duration = Duration::from_millis(250);

/// The Client Monitor ID of the client's one display area.
const DISPLAY_AREA_ID: ClientMonitorId = ClientMonitorId(VarInt::from_u32(1));

/// The client's display, as last requested and as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayRequests {
    /// The Request ID of the last request.
    last_id: u64,
    /// The display last requested.
    requested: DisplaySize,
    /// When the last request was sent.
    sent_at: Instant,
    /// The display, if it differs from the one last requested.
    changed: Option<DisplaySize>,
}

impl DisplayRequests {
    /// Returns the first request, for `display`, to send at `now`, and the
    /// state that follows the display from then on.
    pub(crate) fn start(display: DisplaySize, now: Instant) -> (Self, DisplayRequest) {
        let requests = Self {
            last_id: 1,
            requested: display,
            sent_at: now,
            changed: None,
        };
        let request = display_request(display, requests.last_id);
        (requests, request)
    }

    /// Notes that the display is now `display`.
    pub(crate) fn display_changed(&mut self, display: DisplaySize) {
        self.changed = (display != self.requested).then_some(display);
    }

    /// Returns when the next request is due, if the display changed.
    pub(crate) fn poll_timeout(&self) -> Option<Instant> {
        self.changed.map(|_| self.sent_at + REQUEST_INTERVAL)
    }

    /// Returns the request to send at `now`, if the display changed and the
    /// last request is old enough.
    pub(crate) fn poll_request(&mut self, now: Instant) -> Option<DisplayRequest> {
        if self.poll_timeout()? > now {
            return None;
        }
        let display = self.changed.take()?;
        self.last_id += 1;
        self.requested = display;
        self.sent_at = now;
        Some(display_request(display, self.last_id))
    }
}

/// Returns the request numbered `request_id` for `display`.
fn display_request(display: DisplaySize, request_id: u64) -> DisplayRequest {
    DisplayRequest {
        request_id: DisplayRequestId(VarInt::new(request_id).unwrap_or(VarInt::MAX)),
        monitors: vec![ClientMonitor {
            id: DISPLAY_AREA_ID,
            flags: ClientMonitorFlags::PRIMARY,
            x: SignedVarInt::ZERO,
            y: SignedVarInt::ZERO,
            width: VarInt::from_u32(display.width),
            height: VarInt::from_u32(display.height),
            refresh_rate: VarInt::from_u32(display.refresh_millihertz),
            scale: VarInt::from_u32(display.scale_percent),
            physical_width: VarInt::ZERO,
            physical_height: VarInt::ZERO,
            peak_luminance: VarInt::ZERO,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a display `width` pixels wide.
    fn display(width: u32) -> DisplaySize {
        DisplaySize {
            width,
            height: 720,
            refresh_millihertz: 60_000,
            scale_percent: DisplaySize::UNSCALED,
        }
    }

    /// Changes are requested at most every 250 ms, the last one included,
    /// and a display changed back needs no request.
    #[test]
    fn throttles_requests() {
        let start = Instant::now();
        let (mut requests, first) = DisplayRequests::start(display(1280), start);
        assert_eq!(first.request_id, DisplayRequestId(VarInt::from_u32(1)));
        assert_eq!(first.monitors[0].width, VarInt::from_u32(1280));
        assert_eq!(requests.poll_timeout(), None);

        requests.display_changed(display(1300));
        requests.display_changed(display(1400));
        let due = start + REQUEST_INTERVAL;
        assert_eq!(requests.poll_timeout(), Some(due));
        assert_eq!(requests.poll_request(due - Duration::from_millis(1)), None);
        let second = requests.poll_request(due).unwrap();
        assert_eq!(second.request_id, DisplayRequestId(VarInt::from_u32(2)));
        assert_eq!(second.monitors[0].width, VarInt::from_u32(1400));
        assert_eq!(requests.poll_request(due), None);

        requests.display_changed(display(1500));
        requests.display_changed(display(1400));
        assert_eq!(requests.poll_timeout(), None);
    }
}
