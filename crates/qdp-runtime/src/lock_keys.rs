//! Keeps the host's Num, Caps and Scroll Lock in step with the client's
//! (Section 8.4.1): when the session gains focus and they differ, the
//! client presses and releases the host's lock keys, but only once it knows
//! the host's state from the keyboard's output report, and never while an
//! earlier press is unacknowledged.

use qdp::message::{InputSequence, KeyboardLeds};

/// The Keyboard/Keypad page usages of the lock keys, with their LEDs.
const LOCK_KEYS: [(KeyboardLeds, u8); 3] = [
    (KeyboardLeds::NUM_LOCK, 0x53),
    (KeyboardLeds::CAPS_LOCK, 0x39),
    (KeyboardLeds::SCROLL_LOCK, 0x47),
];

/// The state of the client's lock keys, where the platform tells it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct LockKeys {
    /// The lock keys whose state is known, as their LEDs.
    pub known: KeyboardLeds,
    /// Those of them that are on.
    pub on: KeyboardLeds,
}

/// Brings the host's lock keys to the client's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LockSync {
    /// The host keyboard's LEDs, once its output report arrived.
    host: Option<KeyboardLeds>,
    /// The client's lock keys, as they were when the session last gained
    /// focus, until the host's are known.
    waiting: Option<LockKeys>,
    /// The last lock-key report sent, until the host acknowledges it.
    unacknowledged: Option<InputSequence>,
}

impl LockSync {
    /// Notes the host keyboard's LEDs, from its output report, returning
    /// the usages of the lock keys to press and release if the session
    /// gained focus before they were known.
    pub(crate) fn host_leds(&mut self, leds: KeyboardLeds, acknowledged: InputSequence) -> Vec<u8> {
        self.host = Some(leds);
        match self.waiting.take() {
            Some(local) => self.focus_gained(local, acknowledged),
            None => Vec::new(),
        }
    }

    /// Returns the usages of the lock keys to press and release so that the
    /// host's match `local`, as the session gains focus, given the highest
    /// sequence the host acknowledged.
    pub(crate) fn focus_gained(&mut self, local: LockKeys, acknowledged: InputSequence) -> Vec<u8> {
        if self
            .unacknowledged
            .is_some_and(|sequence| sequence > acknowledged)
        {
            return Vec::new();
        }
        self.unacknowledged = None;
        let Some(host) = self.host else {
            self.waiting = Some(local);
            return Vec::new();
        };
        LOCK_KEYS
            .iter()
            .filter(|(led, _)| {
                local.known.contains(*led) && local.on.contains(*led) != host.contains(*led)
            })
            .map(|&(_, usage)| usage)
            .collect()
    }

    /// Notes the sequence of the last lock-key report sent.
    pub(crate) fn sent(&mut self, sequence: InputSequence) {
        self.unacknowledged = Some(sequence);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp::wire::VarInt;

    /// Returns a sequence number.
    fn sequence(value: u32) -> InputSequence {
        InputSequence(VarInt::from_u32(value))
    }

    /// Only the known lock keys that differ are toggled, once the host's
    /// LEDs are known, and not again while the last toggle is
    /// unacknowledged.
    #[test]
    fn toggles_differing_lock_keys() {
        let mut sync = LockSync::default();
        let local = LockKeys {
            known: KeyboardLeds(KeyboardLeds::CAPS_LOCK.0 | KeyboardLeds::NUM_LOCK.0),
            on: KeyboardLeds::CAPS_LOCK,
        };
        // Before the host's LEDs are known, nothing is sent.
        assert!(sync.focus_gained(local, sequence(0)).is_empty());
        // The host's Num Lock is on and its Caps Lock off; Scroll Lock is not
        // known locally, so it is left alone.
        let host = KeyboardLeds(KeyboardLeds::NUM_LOCK.0 | KeyboardLeds::SCROLL_LOCK.0);
        assert_eq!(sync.host_leds(host, sequence(0)), [0x53, 0x39]);
        sync.sent(sequence(4));
        assert!(sync.focus_gained(local, sequence(3)).is_empty());
        assert_eq!(sync.focus_gained(local, sequence(4)), [0x53, 0x39]);
        sync.host_leds(KeyboardLeds::CAPS_LOCK, sequence(4));
        assert!(sync.focus_gained(local, sequence(4)).is_empty());
    }
}
