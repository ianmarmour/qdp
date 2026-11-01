//! Locking the host's desktop, as a session that ends other than by its
//! client disconnecting does, so that the desktop is not left unlocked
//! (Section 11.4).

/// Locks the desktop: through logind on Linux, `LockWorkStation` on
/// Windows, and the login framework on macOS.
pub fn lock_desktop() -> Result<(), String> {
    platform::lock_desktop()
}

/// Locking on Linux, through logind, which asks the session's screen
/// locker to lock.
#[cfg(target_os = "linux")]
mod platform {
    use std::process::Command;

    /// The systemd command that asks logind to lock a session.
    const LOGINCTL: &str = "loginctl";

    /// The variable naming the logind session this process belongs to.
    const SESSION_VARIABLE: &str = "XDG_SESSION_ID";

    /// The seat whose screen and input the host has: the one with the
    /// machine's own display, keyboard and mouse.
    const SEAT: &str = "seat0";

    /// Locks this process's logind session or, for a host running as a
    /// service, outside any session, the session shown on the seat.
    pub(super) fn lock_desktop() -> Result<(), String> {
        let mut command = Command::new(LOGINCTL);
        command.arg("lock-session");
        if let Some(session) = std::env::var(SESSION_VARIABLE).ok().or_else(seat_session) {
            command.arg(session);
        }
        let output = command
            .output()
            .map_err(|error| format!("cannot run {LOGINCTL}: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "{LOGINCTL} lock-session failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    /// Returns the session shown on the seat, if one is.
    fn seat_session() -> Option<String> {
        let output = Command::new(LOGINCTL)
            .args(["show-seat", SEAT, "--property=ActiveSession", "--value"])
            .output()
            .ok()?;
        let session = String::from_utf8(output.stdout).ok()?.trim().to_owned();
        (output.status.success() && !session.is_empty()).then_some(session)
    }
}

/// Locking on Windows. `LockWorkStation` is a C interface, so this module
/// uses `unsafe`.
#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    use windows::Win32::System::Shutdown::LockWorkStation;

    /// Locks the workstation, as Windows+L does.
    pub(super) fn lock_desktop() -> Result<(), String> {
        // SAFETY: LockWorkStation takes no arguments.
        unsafe { LockWorkStation() }
            .map_err(|error| format!("Windows refused to lock the workstation: {error}"))
    }
}

/// Locking on macOS, which has no public interface for it: the login
/// framework's `SACLockScreenImmediate` locks the screen as the menu bar's
/// Lock Screen does. It is loaded at run time, so this module uses
/// `unsafe`.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod platform {
    use std::ffi::{CStr, c_char, c_int, c_void};

    /// The private framework that holds the function.
    const LOGIN_FRAMEWORK: &CStr =
        c"/System/Library/PrivateFrameworks/login.framework/Versions/Current/login";

    /// The function that locks the screen.
    const LOCK_FUNCTION: &CStr = c"SACLockScreenImmediate";

    /// `dlopen`'s flag to resolve symbols when they are first used.
    const RTLD_LAZY: c_int = 0x1;

    /// The type of `SACLockScreenImmediate`, which returns 0 on success.
    type LockScreen = unsafe extern "C" fn() -> c_int;

    unsafe extern "C" {
        /// Loads a dynamic library, returning null if it cannot.
        fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
        /// Finds a symbol in a loaded library, returning null if it cannot.
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }

    /// Locks the screen.
    pub(super) fn lock_desktop() -> Result<(), String> {
        // SAFETY: the path is a valid C string, and dlopen returns null if
        // the library cannot be loaded. The library stays loaded.
        let library = unsafe { dlopen(LOGIN_FRAMEWORK.as_ptr(), RTLD_LAZY) };
        if library.is_null() {
            return Err("cannot load the login framework".to_owned());
        }
        // SAFETY: the library is loaded and the name is a valid C string.
        let symbol = unsafe { dlsym(library, LOCK_FUNCTION.as_ptr()) };
        if symbol.is_null() {
            return Err("the login framework cannot lock the screen".to_owned());
        }
        // SAFETY: the symbol is SACLockScreenImmediate, a function that
        // takes no arguments and returns an int.
        let lock = unsafe { std::mem::transmute::<*mut c_void, LockScreen>(symbol) };
        // SAFETY: the function takes no arguments.
        let status = unsafe { lock() };
        if status == 0 {
            Ok(())
        } else {
            Err(format!("locking the screen failed with status {status}"))
        }
    }
}

/// Locking where the platform offers no way to.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    /// Fails: this platform cannot lock its desktop yet.
    pub(super) fn lock_desktop() -> Result<(), String> {
        Err("locking the desktop is not supported on this platform yet".to_owned())
    }
}
