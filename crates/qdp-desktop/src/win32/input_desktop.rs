//! Following the input desktop: the desktop Windows shows and takes input
//! on. It is the user's desktop most of the time, and the secure desktop
//! while the sign-in screen, the lock screen, Ctrl+Alt+Delete or an
//! elevation prompt shows. A thread captures and sends input only on the
//! desktop it is attached to, so a host allowed to open the secure desktop,
//! as one running as SYSTEM in the console's session is, moves its threads
//! to the input desktop whenever it changes. Others cannot open the secure
//! desktop, and stay where they are.

use std::cell::RefCell;

use tracing::{debug, info};
use windows::Win32::Foundation::{GENERIC_ALL, HANDLE};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, GetThreadDesktop,
    GetUserObjectInformationW, HDESK, OpenInputDesktop, SetThreadDesktop, UOI_NAME,
};
use windows::Win32::System::Threading::GetCurrentThreadId;

/// The longest desktop name read, in UTF-16 units, terminator included.
const NAME_CAPACITY: usize = 256;

/// A desktop this program opened, closed when dropped.
#[derive(Debug)]
struct OpenDesktop(HDESK);

impl Drop for OpenDesktop {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by this program and is closed once;
        // a thread attached to it holds another reference.
        let _ = unsafe { CloseDesktop(self.0) };
    }
}

thread_local! {
    /// The input desktop this thread moved to, kept open while it is on it.
    static ATTACHED: RefCell<Option<OpenDesktop>> = const { RefCell::new(None) };
}

/// Moves this thread to the input desktop if it is not on it and may open
/// it. Returns whether it moved.
pub(super) fn follow_input_desktop() -> bool {
    // SAFETY: opening the input desktop takes no pointers; the handle is
    // owned and closed by `OpenDesktop`.
    let input = match unsafe {
        OpenInputDesktop(
            DESKTOP_CONTROL_FLAGS(0),
            false,
            DESKTOP_ACCESS_FLAGS(GENERIC_ALL.0),
        )
    } {
        Ok(desktop) => OpenDesktop(desktop),
        // Without the right to, as most programs, the thread stays.
        Err(_) => return false,
    };
    // SAFETY: the thread's own desktop handle is not closed here.
    let current = unsafe { GetThreadDesktop(GetCurrentThreadId()) }.ok();
    let input_name = name(input.0);
    if input_name.is_some() && current.and_then(name) == input_name {
        return false;
    }
    // SAFETY: the desktop handle is valid; the call fails, harmlessly, if
    // the thread has windows or hooks on its desktop.
    if let Err(error) = unsafe { SetThreadDesktop(input.0) } {
        debug!(%error, "cannot move to the input desktop");
        return false;
    }
    info!(
        desktop = input_name.as_deref().unwrap_or("?"),
        "following the input desktop"
    );
    // The desktop moved from closes; the thread now holds the new one.
    ATTACHED.with(|attached| *attached.borrow_mut() = Some(input));
    true
}

/// Returns a desktop's name, such as `Default` or `Winlogon`.
fn name(desktop: HDESK) -> Option<String> {
    let mut buffer = [0u16; NAME_CAPACITY];
    let mut needed = 0u32;
    // SAFETY: the buffer holds the bytes given, and the handle is valid.
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0),
            UOI_NAME,
            Some(buffer.as_mut_ptr().cast()),
            (buffer.len() * size_of::<u16>()) as u32,
            Some(&mut needed),
        )
    }
    .ok()?;
    let length = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..length]))
}
