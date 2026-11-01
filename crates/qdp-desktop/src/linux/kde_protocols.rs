//! KWin's output device and output management protocols, generated from the
//! files of plasma-wayland-protocols 1.23.0 in `protocols/`.
//!
//! The wayland-protocols-plasma crate predates the output device registry,
//! through which KWin 6.7 and later announce their monitors instead of as
//! one global each, so the host generates the protocols itself, as that
//! crate does.

#![allow(
    missing_docs,
    clippy::all,
    clippy::missing_docs_in_private_items,
    unused_imports
)]

/// The output device protocol: the monitors, their modes, and the registry
/// that announces them.
pub(super) mod output_device {
    use wayland_client;
    use wayland_client::backend as wayland_backend;
    use wayland_client::protocol::*;

    /// The protocol's interface descriptions.
    pub mod __interfaces {
        use wayland_client::backend as wayland_backend;
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/kde-output-device-v2.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/kde-output-device-v2.xml");
}

/// The output management protocol, which applies monitor settings.
pub(super) mod output_management {
    use super::output_device::*;
    use wayland_client;
    use wayland_client::backend as wayland_backend;
    use wayland_client::protocol::*;

    /// The protocol's interface descriptions.
    pub mod __interfaces {
        use super::super::output_device::__interfaces::*;
        use wayland_client::backend as wayland_backend;
        use wayland_client::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/kde-output-management-v2.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_client_code!("protocols/kde-output-management-v2.xml");
}
