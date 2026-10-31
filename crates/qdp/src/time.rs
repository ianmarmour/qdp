//! The session clock (Section 5.1).

use std::time::Instant;

use crate::message::Timestamp;
use crate::wire::VarInt;

/// An endpoint's session clock: a monotonic clock counting microseconds from an
/// epoch chosen when the session starts. Every Timestamp the endpoint sends is on
/// this clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionClock {
    /// The instant that is Timestamp 0.
    epoch: Instant,
}

impl SessionClock {
    /// Creates a clock whose epoch is `epoch`.
    pub const fn new(epoch: Instant) -> Self {
        Self { epoch }
    }

    /// Returns the clock's epoch.
    pub const fn epoch(&self) -> Instant {
        self.epoch
    }

    /// Returns the Timestamp of `instant`. Instants before the epoch map to 0, and
    /// instants beyond the range of a variable-length integer, which is over
    /// 146,000 years, saturate.
    pub fn timestamp(&self, instant: Instant) -> Timestamp {
        let micros = instant.saturating_duration_since(self.epoch).as_micros();
        let micros = u64::try_from(micros).unwrap_or(u64::MAX);
        Timestamp(VarInt::new(micros).unwrap_or(VarInt::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Timestamps count microseconds from the epoch.
    #[test]
    fn timestamps() {
        let epoch = Instant::now();
        let clock = SessionClock::new(epoch);
        assert_eq!(clock.timestamp(epoch).as_micros(), 0);
        assert_eq!(
            clock
                .timestamp(epoch + Duration::from_millis(1500))
                .as_micros(),
            1_500_000
        );
    }
}
