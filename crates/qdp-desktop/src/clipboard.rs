//! The system clipboard, shared as plain text (Section 10), through arboard.
//!
//! Content that password managers mark as secret is never shared (Section
//! 10.2). arboard reads it like any other text, so each platform's marker is
//! looked for here: on macOS the `org.nspasteboard.ConcealedType` and
//! `TransientType` types, on Windows the formats that exclude content from
//! clipboard monitoring and history, and on Linux the
//! `x-kde-passwordManagerHint` type.

#![allow(unsafe_code)]

use qdp_runtime::Clipboard;

/// The system clipboard.
pub struct SystemClipboard {
    /// The clipboard.
    clipboard: arboard::Clipboard,
    /// Finds the markers of secret content.
    secrets: secret::Markers,
}

impl std::fmt::Debug for SystemClipboard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SystemClipboard")
            .finish_non_exhaustive()
    }
}

impl SystemClipboard {
    /// Opens the system clipboard.
    pub fn open() -> Result<Self, String> {
        Ok(Self {
            clipboard: arboard::Clipboard::new().map_err(|error| error.to_string())?,
            secrets: secret::Markers::new(),
        })
    }
}

impl Clipboard for SystemClipboard {
    fn text(&mut self) -> Result<Option<String>, String> {
        if self.secrets.marked() {
            return Ok(None);
        }
        match self.clipboard.get_text() {
            Ok(text) => Ok(Some(text).filter(|text| !text.is_empty())),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    fn set_text(&mut self, text: &str) -> Result<(), String> {
        self.clipboard
            .set_text(text)
            .map_err(|error| error.to_string())
    }

    fn description(&self) -> &str {
        "system clipboard"
    }
}

/// Markers of secret content on Linux: the `x-kde-passwordManagerHint` type,
/// offered through Wayland's data control protocol or X11's `TARGETS`.
#[cfg(target_os = "linux")]
mod secret {
    use std::time::{Duration, Instant};

    use wl_clipboard_rs::paste::{ClipboardType, Seat, get_mime_types};
    use x11rb::connection::Connection;
    use x11rb::protocol::Event;
    use x11rb::protocol::xproto::{Atom, AtomEnum, ConnectionExt, CreateWindowAux, WindowClass};
    use x11rb::rust_connection::RustConnection;
    use x11rb::{COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE};

    /// The type KDE's password managers add to secret content.
    const SECRET_HINT: &str = "x-kde-passwordManagerHint";

    /// How long the X11 clipboard owner may take to list its types.
    const TARGETS_TIMEOUT: Duration = Duration::from_millis(200);

    /// How long to sleep between checks for the owner's answer.
    const POLL_STEP: Duration = Duration::from_millis(2);

    /// The most types read from a `TARGETS` list.
    const MAX_TARGETS: u32 = 1024;

    /// A window that asks the X11 clipboard owner for its types.
    struct X11Targets {
        /// The connection to the X server.
        connection: RustConnection,
        /// The window that receives the answers.
        window: u32,
        /// The `CLIPBOARD` atom.
        clipboard: Atom,
        /// The `TARGETS` atom.
        targets: Atom,
        /// The property the answer is written to.
        property: Atom,
    }

    impl X11Targets {
        /// Connects to the X server and creates the window.
        fn open() -> Option<Self> {
            let (connection, screen) = x11rb::connect(None).ok()?;
            let root = connection.setup().roots.get(screen)?.root;
            let window = connection.generate_id().ok()?;
            connection
                .create_window(
                    COPY_DEPTH_FROM_PARENT,
                    window,
                    root,
                    0,
                    0,
                    1,
                    1,
                    0,
                    WindowClass::INPUT_ONLY,
                    COPY_FROM_PARENT,
                    &CreateWindowAux::new(),
                )
                .ok()?;
            let atom = |name: &str| {
                connection
                    .intern_atom(false, name.as_bytes())
                    .ok()?
                    .reply()
                    .ok()
                    .map(|reply| reply.atom)
            };
            let (clipboard, targets, property) =
                (atom("CLIPBOARD")?, atom("TARGETS")?, atom("QDP_TARGETS")?);
            Some(Self {
                connection,
                window,
                clipboard,
                targets,
                property,
            })
        }

        /// Returns whether the clipboard owner offers `name`, or `None` if
        /// it did not answer.
        fn offers(&self, name: &str) -> Option<bool> {
            // The atom exists only once some client named it.
            let wanted = self
                .connection
                .intern_atom(true, name.as_bytes())
                .ok()?
                .reply()
                .ok()?
                .atom;
            if wanted == NONE {
                return Some(false);
            }
            self.connection
                .convert_selection(
                    self.window,
                    self.clipboard,
                    self.targets,
                    self.property,
                    CURRENT_TIME,
                )
                .ok()?;
            self.connection.flush().ok()?;
            let deadline = Instant::now() + TARGETS_TIMEOUT;
            loop {
                match self.connection.poll_for_event().ok()? {
                    Some(Event::SelectionNotify(notify)) if notify.requestor == self.window => {
                        if notify.property == NONE {
                            return Some(false);
                        }
                        let reply = self
                            .connection
                            .get_property(
                                true,
                                self.window,
                                self.property,
                                AtomEnum::ATOM,
                                0,
                                MAX_TARGETS,
                            )
                            .ok()?
                            .reply()
                            .ok()?;
                        return Some(reply.value32()?.any(|atom| atom == wanted));
                    }
                    Some(_) => {}
                    None if Instant::now() >= deadline => return None,
                    None => std::thread::sleep(POLL_STEP),
                }
            }
        }
    }

    /// Finds the secret marker on the clipboard.
    pub(super) struct Markers {
        /// The X11 window, once opened.
        x11: Option<X11Targets>,
    }

    impl Markers {
        /// Returns a finder that connects to X11 when first needed.
        pub(super) fn new() -> Self {
            Self { x11: None }
        }

        /// Returns whether the clipboard holds content marked secret.
        pub(super) fn marked(&mut self) -> bool {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                if let Ok(types) = get_mime_types(ClipboardType::Regular, Seat::Unspecified) {
                    return types.contains(SECRET_HINT);
                }
            }
            if self.x11.is_none() {
                self.x11 = X11Targets::open();
            }
            self.x11
                .as_ref()
                .and_then(|x11| x11.offers(SECRET_HINT))
                .unwrap_or(false)
        }
    }
}

/// Markers of secret content on macOS: the pasteboard types of
/// nspasteboard.org's convention.
#[cfg(target_os = "macos")]
mod secret {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSPasteboard;

    /// The types password managers add to secret or short-lived content.
    const SECRET_TYPES: [&str; 2] = [
        "org.nspasteboard.ConcealedType",
        "org.nspasteboard.TransientType",
    ];

    /// Finds the secret markers on the pasteboard.
    pub(super) struct Markers;

    impl Markers {
        /// Returns a finder.
        pub(super) fn new() -> Self {
            Self
        }

        /// Returns whether the pasteboard holds content marked secret.
        pub(super) fn marked(&mut self) -> bool {
            autoreleasepool(|_| {
                NSPasteboard::generalPasteboard()
                    .types()
                    .is_some_and(|types| {
                        types
                            .iter()
                            .any(|kind| SECRET_TYPES.contains(&kind.to_string().as_str()))
                    })
            })
        }
    }
}

/// Markers of secret content on Windows: the formats that exclude content
/// from clipboard monitoring and from the clipboard history.
#[cfg(windows)]
mod secret {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::core::w;

    /// Finds the secret markers on the clipboard.
    pub(super) struct Markers;

    impl Markers {
        /// Returns a finder.
        pub(super) fn new() -> Self {
            Self
        }

        /// Returns whether the clipboard holds content marked secret.
        pub(super) fn marked(&mut self) -> bool {
            // SAFETY: the names are valid wide strings.
            let (excluded, history) = unsafe {
                (
                    RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing")),
                    RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory")),
                )
            };
            // SAFETY: checking a format needs no open clipboard.
            if unsafe { IsClipboardFormatAvailable(excluded) }.is_ok() {
                return true;
            }
            // SAFETY: as above.
            if unsafe { IsClipboardFormatAvailable(history) }.is_err() {
                return false;
            }
            // The history format holds a DWORD: 0 keeps the content out. If it
            // cannot be read, the content is treated as secret.
            history_allowed(history) != Some(true)
        }
    }

    /// Returns whether the `CanIncludeInClipboardHistory` value allows the
    /// content into the history, or `None` if it cannot be read.
    fn history_allowed(format: u32) -> Option<bool> {
        // SAFETY: the clipboard is closed below whenever it was opened.
        unsafe { OpenClipboard(None) }.ok()?;
        // SAFETY: the clipboard is open; the data's memory is read while
        // locked, within its size, and unlocked before it is closed.
        let allowed = unsafe {
            GetClipboardData(format).ok().and_then(|handle| {
                let memory = HGLOBAL(handle.0);
                let data = GlobalLock(memory).cast::<u32>();
                let value = (!data.is_null() && GlobalSize(memory) >= size_of::<u32>())
                    .then(|| data.read_unaligned() != 0);
                let _ = GlobalUnlock(memory);
                value
            })
        };
        // SAFETY: the clipboard was opened above.
        let _ = unsafe { CloseClipboard() };
        allowed
    }
}

/// No markers of secret content on other platforms.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod secret {
    /// Finds no markers.
    pub(super) struct Markers;

    impl Markers {
        /// Returns a finder.
        pub(super) fn new() -> Self {
            Self
        }

        /// Returns that nothing is marked.
        pub(super) fn marked(&mut self) -> bool {
            false
        }
    }
}
