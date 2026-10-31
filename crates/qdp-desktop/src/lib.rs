//! Desktop integration for Quick Desktop Protocol (QDP) hosts: performing the
//! client's input on each operating system.
//!
//! - Linux: [`UinputInjector`] creates virtual keyboard, mouse and pointer
//!   devices through uinput, which works under every Wayland compositor and
//!   X11 server.

#[cfg(target_os = "linux")]
mod linux;

use qdp_runtime::{InputInjector, NoInput};
use tracing::warn;

#[cfg(target_os = "linux")]
pub use linux::UinputInjector;

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
