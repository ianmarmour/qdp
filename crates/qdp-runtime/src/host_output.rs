//! The host's output reports of standard devices: keyboard LEDs and gamepad
//! rumble and LEDs, which travel as state in `DEVICE_OUTPUT` datagrams
//! (Section 8.8.1).
//!
//! Each device's Output Sequence starts at 1 and grows by 1 for each new
//! state. The host sends a state when it changes and repeats it unchanged at
//! least once per second, and every 100 ms while it rumbles, so that a lost
//! datagram is soon replaced and the client stops rumbling if the host goes
//! quiet.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use qdp::message::{DatagramPayload, DeviceId, DeviceOutput, GamepadOutput, KeyboardOutput};
use qdp::wire::VarInt;

/// How often the host repeats an unchanged output.
const REPEAT_INTERVAL: Duration = Duration::from_secs(1);

/// How often the host repeats an output while any rumble value is non-zero.
const RUMBLE_REPEAT_INTERVAL: Duration = Duration::from_millis(100);

/// The output report of one standard device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputReport {
    /// A keyboard's LEDs.
    Keyboard(KeyboardOutput),
    /// A gamepad's rumble and LED.
    Gamepad(GamepadOutput),
}

impl OutputReport {
    /// Returns the encoded report.
    fn to_bytes(self) -> Vec<u8> {
        match self {
            Self::Keyboard(report) => report.to_bytes(),
            Self::Gamepad(report) => report.to_bytes(),
        }
    }

    /// Returns how long the host waits before repeating the report.
    fn repeat_interval(self) -> Duration {
        match self {
            Self::Gamepad(report) if report.is_rumbling() => RUMBLE_REPEAT_INTERVAL,
            _ => REPEAT_INTERVAL,
        }
    }
}

/// The output last sent for one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SentOutput {
    /// Its Output Sequence.
    sequence: u64,
    /// The report.
    report: OutputReport,
    /// When it was last sent.
    sent_at: Instant,
}

impl SentOutput {
    /// Returns when the output is next repeated.
    fn repeat_at(&self) -> Instant {
        self.sent_at + self.report.repeat_interval()
    }

    /// Returns the `DEVICE_OUTPUT` datagram carrying the output.
    fn datagram(&self, device_id: DeviceId) -> Vec<u8> {
        let data = self.report.to_bytes();
        DeviceOutput {
            device_id,
            output_sequence: VarInt::try_from(self.sequence).unwrap_or(VarInt::MAX),
            data: &data,
        }
        .to_bytes()
    }
}

/// The outputs of a session's devices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DeviceOutputs {
    /// The output last sent for each device that has one.
    devices: HashMap<DeviceId, SentOutput>,
}

impl DeviceOutputs {
    /// Records a device's current output, returning the datagram to send if
    /// it differs from the last one sent.
    pub(crate) fn update(
        &mut self,
        device_id: DeviceId,
        report: OutputReport,
        now: Instant,
    ) -> Option<Vec<u8>> {
        let sequence = match self.devices.get(&device_id) {
            Some(sent) if sent.report == report => return None,
            Some(sent) => sent.sequence + 1,
            None => 1,
        };
        let sent = SentOutput {
            sequence,
            report,
            sent_at: now,
        };
        self.devices.insert(device_id, sent);
        Some(sent.datagram(device_id))
    }

    /// Returns the datagrams repeating the outputs that are due.
    pub(crate) fn repeats(&mut self, now: Instant) -> Vec<Vec<u8>> {
        self.devices
            .iter_mut()
            .filter(|(_, sent)| sent.repeat_at() <= now)
            .map(|(&device_id, sent)| {
                sent.sent_at = now;
                sent.datagram(device_id)
            })
            .collect()
    }

    /// Returns when the next repeat is due, if any device has an output.
    pub(crate) fn poll_timeout(&self) -> Option<Instant> {
        self.devices.values().map(SentOutput::repeat_at).min()
    }

    /// Forgets a detached device's output.
    pub(crate) fn remove(&mut self, device_id: DeviceId) {
        self.devices.remove(&device_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::message::{Datagram, KeyboardLeds};

    /// The gamepad's Device ID in the tests.
    const GAMEPAD: DeviceId = DeviceId(VarInt::from_u32(16));

    /// The keyboard's Device ID in the tests.
    const KEYBOARD: DeviceId = DeviceId(VarInt::from_u32(1));

    /// Returns the Output Sequence and report data of a datagram.
    fn decode(datagram: &[u8]) -> (u64, Vec<u8>) {
        match Datagram::decode(datagram) {
            Ok(Datagram::DeviceOutput(output)) => {
                (output.output_sequence.into_inner(), output.data.to_vec())
            }
            other => panic!("not a DEVICE_OUTPUT: {other:?}"),
        }
    }

    /// Each device numbers its own states from 1, sending only changes.
    #[test]
    fn numbers_each_devices_states() {
        let mut outputs = DeviceOutputs::default();
        let now = Instant::now();
        let leds = OutputReport::Keyboard(KeyboardOutput {
            leds: KeyboardLeds(1),
        });
        let rumble = OutputReport::Gamepad(GamepadOutput {
            low_frequency_rumble: 1000,
            ..GamepadOutput::default()
        });
        let first = outputs.update(KEYBOARD, leds, now).expect("new output");
        assert_eq!(decode(&first), (1, leds.to_bytes()));
        assert_eq!(outputs.update(KEYBOARD, leds, now), None);
        let gamepad = outputs.update(GAMEPAD, rumble, now).expect("new output");
        assert_eq!(decode(&gamepad).0, 1);
        let stopped = OutputReport::Gamepad(GamepadOutput::default());
        let changed = outputs.update(GAMEPAD, stopped, now).expect("changed");
        assert_eq!(decode(&changed), (2, stopped.to_bytes()));
        outputs.remove(GAMEPAD);
        let again = outputs.update(GAMEPAD, stopped, now).expect("new output");
        assert_eq!(decode(&again).0, 1);
    }

    /// Outputs repeat every second, and every 100 ms while rumbling, keeping
    /// their Output Sequence.
    #[test]
    fn repeats_outputs() {
        let mut outputs = DeviceOutputs::default();
        let start = Instant::now();
        let rumble = OutputReport::Gamepad(GamepadOutput {
            high_frequency_rumble: 2000,
            ..GamepadOutput::default()
        });
        let leds = OutputReport::Keyboard(KeyboardOutput::default());
        outputs.update(GAMEPAD, rumble, start);
        outputs.update(KEYBOARD, leds, start);
        assert_eq!(outputs.poll_timeout(), Some(start + RUMBLE_REPEAT_INTERVAL));
        assert!(outputs.repeats(start).is_empty());
        let repeats = outputs.repeats(start + RUMBLE_REPEAT_INTERVAL);
        assert_eq!(repeats.len(), 1);
        assert_eq!(decode(&repeats[0]), (1, rumble.to_bytes()));
        let repeats = outputs.repeats(start + REPEAT_INTERVAL);
        assert_eq!(repeats.len(), 2);
        assert_eq!(
            outputs.poll_timeout(),
            Some(start + REPEAT_INTERVAL + RUMBLE_REPEAT_INTERVAL)
        );
    }
}
