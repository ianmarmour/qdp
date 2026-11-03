//! The clock that the probe and the measurer, separate processes on one
//! machine, both read: the monotonic clock, which no one sets.

#![allow(unsafe_code)]

/// Microseconds in a second.
const MICROS_PER_SECOND: u64 = 1_000_000;

/// Nanoseconds in a microsecond.
const NANOS_PER_MICRO: u64 = 1_000;

/// Returns the monotonic clock's time, in microseconds.
pub fn now_micros() -> u64 {
    let mut now = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `now` is a valid timespec for the call to write.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now) };
    let seconds = u64::try_from(now.tv_sec).unwrap_or(0);
    let nanos = u64::try_from(now.tv_nsec).unwrap_or(0);
    seconds * MICROS_PER_SECOND + nanos / NANOS_PER_MICRO
}

/// Returns how many clock ticks a second the kernel counts processes' CPU
/// time in.
pub fn ticks_per_second() -> u64 {
    // SAFETY: sysconf only reads the system's configuration.
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(ticks)
        .ok()
        .filter(|&ticks| ticks > 0)
        .unwrap_or(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The clock never goes back.
    #[test]
    fn goes_forward() {
        let first = now_micros();
        std::thread::sleep(std::time::Duration::from_millis(2));
        assert!(now_micros() >= first + 2_000);
    }
}
