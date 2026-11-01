//! Whether virtual input devices stay with this session (Section 8.2.5).
//! udev assigns uinput and uhid devices, which carry no seat of their own,
//! to `seat0`, so they reach only this session if it is the session of
//! `seat0`'s active user, or the machine has no seat manager.
//!
//! Also the user session shown on `seat0`, whose sound a host running
//! outside any session, as a system service does, streams. logind keeps
//! both in the state files `sd-login` reads.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

/// The seat udev assigns devices to.
const DEFAULT_SEAT: &str = "seat0";

/// Where logind describes `seat0`, its active user among the rest.
const DEFAULT_SEAT_STATE: &str = "/run/systemd/seats/seat0";

/// The field of logind's seat state that names the active user.
const ACTIVE_UID_FIELD: &str = "ACTIVE_UID=";

/// The field of logind's seat state that names the active session.
const ACTIVE_SESSION_FIELD: &str = "ACTIVE=";

/// Where logind describes each session, by its ID.
const SESSION_STATE_DIRECTORY: &str = "/run/systemd/sessions";

/// The field of logind's session state that says what kind it is.
const CLASS_FIELD: &str = "CLASS=";

/// The class of a person's own session, rather than a login screen's.
const USER_CLASS: &str = "user";

/// Where each user's runtime directory is, by their ID.
const RUNTIME_DIRECTORY_ROOT: &str = "/run/user";

/// A person's desktop session, shown on `seat0`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SeatSession {
    /// The person's user ID.
    pub uid: u32,
    /// Their primary group.
    pub gid: u32,
    /// Their runtime directory, where the session's sockets are.
    pub runtime_directory: PathBuf,
}

/// Returns the person's session shown on `seat0`, or `None` while a login
/// screen shows there, or nothing does.
pub fn seat_session() -> Option<SeatSession> {
    let seat_state = fs::read_to_string(DEFAULT_SEAT_STATE).ok()?;
    let (session, uid) = active_session(&seat_state)?;
    let session_state = fs::read_to_string(format!("{SESSION_STATE_DIRECTORY}/{session}")).ok()?;
    if field(&session_state, CLASS_FIELD) != Some(USER_CLASS) {
        return None;
    }
    let runtime_directory = PathBuf::from(format!("{RUNTIME_DIRECTORY_ROOT}/{uid}"));
    // The runtime directory belongs to the user and their primary group.
    let gid = fs::metadata(&runtime_directory).ok()?.gid();
    Some(SeatSession {
        uid,
        gid,
        runtime_directory,
    })
}

/// Returns the active session's ID and its user's ID from logind's state
/// of `seat0`, if it has one with an ID that names a file.
fn active_session(seat_state: &str) -> Option<(&str, u32)> {
    let session = field(seat_state, ACTIVE_SESSION_FIELD)?;
    if session.is_empty() || !session.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let uid = field(seat_state, ACTIVE_UID_FIELD)?.parse().ok()?;
    Some((session, uid))
}

/// Returns the value of a state file's line that starts with `name`.
fn field<'a>(state: &'a str, name: &str) -> Option<&'a str> {
    state
        .lines()
        .find_map(|line| line.strip_prefix(name))
        .map(str::trim)
}

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

    /// The seat's active session and user are read from its state, and an
    /// ID that is not a plain name is refused.
    #[test]
    fn reads_the_active_session() {
        let state = "IS_SEAT0=1\nACTIVE=c2\nACTIVE_UID=1000\nSESSIONS=c1 c2\n";
        assert_eq!(active_session(state), Some(("c2", 1000)));
        assert_eq!(active_session("IS_SEAT0=1\n"), None);
        assert_eq!(active_session("ACTIVE=../x\nACTIVE_UID=0\n"), None);
        assert_eq!(active_session("ACTIVE=c2\n"), None);
        assert_eq!(
            field("CLASS=greeter\nTYPE=wayland\n", CLASS_FIELD),
            Some("greeter")
        );
    }
}
