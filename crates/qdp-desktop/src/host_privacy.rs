//! Privacy for whoever is at the host while a session streams its screen
//! (Section 11.4): the host's displays covered, saying that the computer is
//! in use from another device, and its own keyboard and mouse held back, so
//! that only the device streaming sees and controls it. Windows hosts have
//! it; other systems have no way yet to cover their displays without
//! covering the stream too.

/// The host's displays covered, and its own keyboard and mouse held back,
/// until dropped.
#[derive(Debug)]
pub struct HostPrivacy {
    /// The covers and hooks.
    #[cfg(windows)]
    _privacy: crate::win32::Privacy,
}

/// Covers the host's displays and holds back its keyboard and mouse, until
/// the returned privacy is dropped.
///
/// # Errors
///
/// Fails on systems without privacy, on Windows before 10 version 2004,
/// which cannot leave the covers out of the stream, or if the covers or the
/// hooks holding back input cannot be made.
pub fn host_privacy() -> Result<HostPrivacy, String> {
    #[cfg(windows)]
    {
        crate::win32::Privacy::start().map(|privacy| HostPrivacy { _privacy: privacy })
    }
    #[cfg(not(windows))]
    {
        Err("only Windows hosts can cover their displays".to_owned())
    }
}

/// Whether this system's hosts have privacy at all.
pub const HAS_PRIVACY: bool = cfg!(windows);
