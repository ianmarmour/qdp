//! Whether virtual input devices stay with this session (Section 8.2.5).
//! udev assigns uinput and uhid devices, which carry no seat of their own,
//! to `seat0`, so they reach only this session if it is the session of
//! `seat0`'s active user, or the machine has no seat manager.

use std::fs;

/// The seat udev assigns devices to.
const DEFAULT_SEAT: &str = "seat0";

/// Where logind describes `seat0`, its active user among the rest.
const DEFAULT_SEAT_STATE: &str = "/run/systemd/seats/seat0";

/// The field of logind's seat state that names the active user.
const ACTIVE_UID_FIELD: &str = "ACTIVE_UID=";

/// Returns whether virtual input devices created now reach only this
/// session's user.
pub(crate) fn devices_confined() -> bool {
    // SAFETY: `getuid` has no preconditions and cannot fail.
    let uid = unsafe { libc::getuid() };
    confined(
        std::env::var("XDG_SEAT").ok().as_deref(),
        fs::read_to_string(DEFAULT_SEAT_STATE).ok().as_deref(),
        uid,
    )
}

/// Returns whether devices assigned to `seat0` reach only the user `uid`,
/// whose session is on `session_seat`, if it has one, given logind's state
/// of `seat0`, if it has one.
fn confined(session_seat: Option<&str>, seat_state: Option<&str>, uid: u32) -> bool {
    if session_seat.is_some_and(|seat| seat != DEFAULT_SEAT) {
        return false;
    }
    let Some(state) = seat_state else {
        // No seat manager: the machine has one console, its user's.
        return true;
    };
    state
        .lines()
        .find_map(|line| line.strip_prefix(ACTIVE_UID_FIELD))
        .and_then(|active| active.trim().parse::<u32>().ok())
        .is_none_or(|active| active == uid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Devices stay with the session on `seat0` while its user is active
    /// there, or without a seat manager.
    #[test]
    fn checks_the_seat() {
        let state = "IS_SEAT0=1\nACTIVE=c2\nACTIVE_UID=1000\n";
        assert!(confined(Some("seat0"), Some(state), 1000));
        assert!(confined(None, Some(state), 1000));
        assert!(!confined(None, Some(state), 1001));
        assert!(!confined(Some("seat1"), Some(state), 1000));
        assert!(confined(None, None, 1001));
        assert!(confined(None, Some("IS_SEAT0=1\n"), 1001));
    }
}
