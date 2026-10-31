//! Desktop integration for Quick Desktop Protocol (QDP) hosts: capturing the
//! screen and performing the client's input on each operating system.
//!
//! - Linux: `ScreenCapture` captures a monitor through the ScreenCast portal
//!   and PipeWire, and [`UinputInjector`] creates virtual keyboard, mouse and
//!   pointer devices through uinput, which works under every Wayland
//!   compositor and X11 server.

#[cfg(target_os = "linux")]
mod linux;

use std::path::Path;

use qdp_runtime::{FrameSource, InputInjector, NoInput};
use tracing::warn;

#[cfg(all(target_os = "linux", feature = "screen-capture"))]
pub use linux::ScreenCapture;
#[cfg(target_os = "linux")]
pub use linux::UinputInjector;

/// The file, in the host's state directory, that keeps the token restoring the
/// user's choice of monitor.
#[cfg_attr(
    not(all(target_os = "linux", feature = "screen-capture")),
    allow(dead_code)
)]
const RESTORE_TOKEN_FILE: &str = "screencast-restore-token";

/// Starts capturing the screen, keeping in `state_directory` what lets later
/// sessions start without asking the user again. Blocks until the first
/// frame arrives, which may wait for the user to choose a monitor.
#[cfg(all(target_os = "linux", feature = "screen-capture"))]
pub fn open_screen(state_directory: &Path) -> Result<Box<dyn FrameSource>, String> {
    let token_path = state_directory.join(RESTORE_TOKEN_FILE);
    let token = std::fs::read_to_string(&token_path)
        .ok()
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty());
    let capture = ScreenCapture::open(token)?;
    if let Some(token) = capture.restore_token() {
        if let Err(error) = std::fs::write(&token_path, token) {
            warn!(%error, path = %token_path.display(), "cannot keep the screen sharing choice");
        }
    }
    Ok(Box::new(capture))
}

/// Starts capturing the screen; this platform cannot yet.
#[cfg(not(all(target_os = "linux", feature = "screen-capture")))]
pub fn open_screen(_state_directory: &Path) -> Result<Box<dyn FrameSource>, String> {
    Err("screen capture is not supported on this platform yet".to_owned())
}

/// Returns the platform's input injector for a `width × height` pixel
/// desktop, or one that ignores input, with a warning, if the platform's
/// cannot be used.
pub fn input_injector(width: u32, height: u32) -> Box<dyn InputInjector> {
    match platform_injector(width, height) {
        Ok(injector) => injector,
        Err(error) => {
            warn!(%error, "the client's input will be ignored");
            Box::new(NoInput)
        }
    }
}

/// Opens the platform's input injector: uinput on Linux.
#[cfg(target_os = "linux")]
fn platform_injector(width: u32, height: u32) -> Result<Box<dyn InputInjector>, String> {
    UinputInjector::new(width, height)
        .map(|injector| Box::new(injector) as Box<dyn InputInjector>)
        .map_err(|error| {
            format!(
                "cannot create virtual input devices in /dev/uinput ({error}); load the uinput \
                 module and give this user write access to /dev/uinput"
            )
        })
}

/// Opens the platform's input injector; this platform has none yet.
#[cfg(not(target_os = "linux"))]
fn platform_injector(_width: u32, _height: u32) -> Result<Box<dyn InputInjector>, String> {
    Err("input injection is not supported on this platform yet".to_owned())
}
