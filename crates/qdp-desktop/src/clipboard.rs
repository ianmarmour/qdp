//! The system clipboard, shared as plain text (Section 10): on Linux through
//! Wayland's data control protocols when the compositor offers them, and
//! otherwise, and on other platforms, through arboard.
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
    /// How the clipboard is reached.
    backend: Backend,
    /// Finds the markers of secret content.
    secrets: secret::Markers,
}

/// How the system clipboard is reached.
enum Backend {
    /// Wayland's data control protocols (Linux).
    #[cfg(target_os = "linux")]
    Wayland,
    /// arboard: X11 on Linux, the platform's own clipboard elsewhere.
    Arboard(arboard::Clipboard),
}

impl std::fmt::Debug for SystemClipboard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SystemClipboard")
            .finish_non_exhaustive()
    }
}

impl SystemClipboard {
    /// Opens the system clipboard: on Linux, Wayland's when the compositor
    /// offers data control, and otherwise arboard's.
    pub fn open() -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        if wayland::is_available() {
            return Ok(Self {
                backend: Backend::Wayland,
                secrets: secret::Markers::new(),
            });
        }
        Ok(Self {
            backend: Backend::Arboard(
                arboard::Clipboard::new().map_err(|error| error.to_string())?,
            ),
            secrets: secret::Markers::new(),
        })
    }
}

impl Clipboard for SystemClipboard {
    fn text(&mut self) -> Result<Option<String>, String> {
        if self.secrets.marked() {
            return Ok(None);
        }
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            Backend::Wayland => wayland::text(),
            Backend::Arboard(clipboard) => match clipboard.get_text() {
                Ok(text) => Ok(Some(text).filter(|text| !text.is_empty())),
                Err(arboard::Error::ContentNotAvailable) => Ok(None),
                Err(error) => Err(error.to_string()),
            },
        }
    }

    fn set_text(&mut self, text: &str) -> Result<(), String> {
        match &mut self.backend {
            #[cfg(target_os = "linux")]
            Backend::Wayland => wayland::set_text(text),
            Backend::Arboard(clipboard) => {
                clipboard.set_text(text).map_err(|error| error.to_string())
            }
        }
    }

    fn description(&self) -> &str {
        match self.backend {
            #[cfg(target_os = "linux")]
            Backend::Wayland => "Wayland data control",
            Backend::Arboard(_) => "system clipboard",
        }
    }
}

/// Names the session's Wayland display in this process's environment when
/// the environment names none, as when the host is started over SSH, and
/// returns its name. Wayland libraries find the compositor only through
/// `WAYLAND_DISPLAY`; without it the clipboard would be tried through X11
/// alone. The display is the first Wayland socket in `XDG_RUNTIME_DIR`:
/// `wayland-0` for KWin and Mutter.
///
/// # Safety
///
/// It changes the environment, so no other thread may be running.
#[cfg(target_os = "linux")]
pub unsafe fn adopt_wayland_display() -> Option<String> {
    if std::env::var_os(wayland::DISPLAY_VARIABLE).is_some() {
        return None;
    }
    let display = wayland::session_display()?;
    // SAFETY: the caller guarantees that no other thread is running.
    unsafe { std::env::set_var(wayland::DISPLAY_VARIABLE, &display) };
    Some(display)
}

/// The clipboard through Wayland's data control protocols, which reach the
/// compositor's clipboard without a window or input focus.
#[cfg(target_os = "linux")]
mod wayland {
    use std::io::{self, Read};
    use std::os::fd::{AsFd, AsRawFd};
    use std::os::unix::fs::FileTypeExt;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use wl_clipboard_rs::{copy, paste, utils};

    /// How long the clipboard's owner may take to send its text.
    const READ_TIMEOUT: Duration = Duration::from_secs(1);

    /// The most text read from the clipboard, in bytes: the Max Size the
    /// host and client accept (Section 4.6.6).
    const MAX_TEXT_BYTES: usize = 16 << 20;

    /// The bytes read from the owner at a time.
    const READ_CHUNK: usize = 64 << 10;

    /// The variable that names the Wayland display.
    pub(super) const DISPLAY_VARIABLE: &str = "WAYLAND_DISPLAY";

    /// The variable that names the directory holding the display's socket.
    const RUNTIME_VARIABLE: &str = "XDG_RUNTIME_DIR";

    /// The prefix of Wayland display sockets' names.
    const SOCKET_PREFIX: &str = "wayland-";

    /// The suffix of the lock files beside them.
    const LOCK_SUFFIX: &str = ".lock";

    /// Returns whether the compositor can be reached and offers data control.
    pub(super) fn is_available() -> bool {
        utils::is_primary_selection_supported().is_ok()
    }

    /// Returns the name of the first Wayland display socket in the session's
    /// runtime directory, if any.
    pub(super) fn session_display() -> Option<String> {
        first_display(Path::new(&std::env::var_os(RUNTIME_VARIABLE)?))
    }

    /// Returns the name of the first Wayland display socket in `directory`,
    /// in name order.
    fn first_display(directory: &Path) -> Option<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .ok()?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let name = entry.file_name().into_string().ok()?;
                (name.starts_with(SOCKET_PREFIX)
                    && !name.ends_with(LOCK_SUFFIX)
                    && std::fs::metadata(entry.path())
                        .ok()?
                        .file_type()
                        .is_socket())
                .then_some(name)
            })
            .collect();
        names.sort();
        names.into_iter().next()
    }

    /// Returns the clipboard's text, or `None` if it holds none.
    pub(super) fn text() -> Result<Option<String>, String> {
        let (pipe, _) = match paste::get_contents(
            paste::ClipboardType::Regular,
            paste::Seat::Unspecified,
            paste::MimeType::Text,
        ) {
            Ok(contents) => contents,
            Err(paste::Error::ClipboardEmpty | paste::Error::NoMimeType) => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let Some(bytes) = read_within(pipe, READ_TIMEOUT, MAX_TEXT_BYTES)? else {
            return Ok(None);
        };
        let text = String::from_utf8(bytes)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
        Ok(Some(text).filter(|text| !text.is_empty()))
    }

    /// Puts `text` on the clipboard. A thread offers it until another
    /// application takes the clipboard.
    pub(super) fn set_text(text: &str) -> Result<(), String> {
        copy::Options::new()
            .copy(
                copy::Source::Bytes(text.as_bytes().into()),
                copy::MimeType::Text,
            )
            .map_err(|error| error.to_string())
    }

    /// Reads `source` to its end within `timeout`, or returns `None` if it
    /// holds more than `limit` bytes: an owner that never finishes writing
    /// cannot stall the clipboard.
    fn read_within(
        mut source: impl Read + AsFd,
        timeout: Duration,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, String> {
        let deadline = Instant::now() + timeout;
        let mut contents = Vec::new();
        let mut chunk = vec![0_u8; READ_CHUNK];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("the clipboard's owner did not send its text in time".to_owned());
            }
            let mut ready = libc::pollfd {
                fd: source.as_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let millis = i32::try_from(remaining.as_millis().max(1)).unwrap_or(i32::MAX);
            // SAFETY: `ready` is one valid pollfd, alive for the call, and
            // its descriptor stays open while `source` is borrowed.
            let polled = unsafe { libc::poll(&mut ready, 1, millis) };
            if polled < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.to_string());
            }
            if polled == 0 {
                continue;
            }
            match source.read(&mut chunk) {
                Ok(0) => return Ok(Some(contents)),
                Ok(read) => {
                    if contents.len() + read > limit {
                        return Ok(None);
                    }
                    contents.extend_from_slice(&chunk[..read]);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        use std::io::Write;

        /// The owner's text is read to its end, text beyond the limit is
        /// not shared, and an owner that never finishes is given up on.
        #[test]
        fn reads_the_owner_s_text_within_bounds() {
            let (reader, mut writer) = std::io::pipe().unwrap();
            writer.write_all(b"copied").unwrap();
            drop(writer);
            assert_eq!(
                read_within(reader, READ_TIMEOUT, MAX_TEXT_BYTES),
                Ok(Some(b"copied".to_vec()))
            );

            let (reader, mut writer) = std::io::pipe().unwrap();
            writer.write_all(b"too long").unwrap();
            drop(writer);
            assert_eq!(read_within(reader, READ_TIMEOUT, 4), Ok(None));

            let (reader, mut writer) = std::io::pipe().unwrap();
            writer.write_all(b"never ends").unwrap();
            let started = Instant::now();
            let timeout = Duration::from_millis(50);
            assert!(read_within(reader, timeout, MAX_TEXT_BYTES).is_err());
            assert!(started.elapsed() >= timeout);
            drop(writer);
        }

        /// The first socket by name is the session's display; lock files and
        /// other files are not displays.
        #[test]
        fn finds_the_display_socket() {
            let directory =
                std::env::temp_dir().join(format!("qdp-wayland-{}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            assert_eq!(first_display(&directory), None);
            std::fs::write(directory.join("wayland-0.lock"), b"").unwrap();
            std::fs::write(directory.join("wayland-0"), b"").unwrap();
            assert_eq!(first_display(&directory), None);
            let _second =
                std::os::unix::net::UnixListener::bind(directory.join("wayland-2")).unwrap();
            let _first =
                std::os::unix::net::UnixListener::bind(directory.join("wayland-1")).unwrap();
            assert_eq!(first_display(&directory).as_deref(), Some("wayland-1"));
            std::fs::remove_dir_all(&directory).unwrap();
        }
    }
}

/// Markers of secret content on Linux: the `x-kde-passwordManagerHint` type,
/// offered through Wayland's data control protocol or X11's `TARGETS`.
#[cfg(target_os = "linux")]
mod secret {
    use std::time::{Duration, Instant};

    use wl_clipboard_rs::paste::{self, ClipboardType, Seat, get_mime_types};
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

        /// Returns whether the clipboard holds content marked secret. If its
        /// types cannot be read, the content is treated as secret. Wayland is
        /// asked first, even where `WAYLAND_DISPLAY` is not set, and X11 only
        /// when Wayland cannot answer.
        pub(super) fn marked(&mut self) -> bool {
            match get_mime_types(ClipboardType::Regular, Seat::Unspecified) {
                Ok(types) => return types.contains(SECRET_HINT),
                Err(paste::Error::ClipboardEmpty) => return false,
                Err(_) => {}
            }
            if self.x11.is_none() {
                self.x11 = X11Targets::open();
            }
            self.x11
                .as_ref()
                .and_then(|x11| x11.offers(SECRET_HINT))
                .unwrap_or(true)
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

        /// Returns whether the pasteboard holds content marked secret. If
        /// its types cannot be read, the content is treated as secret.
        pub(super) fn marked(&mut self) -> bool {
            autoreleasepool(|_| {
                NSPasteboard::generalPasteboard()
                    .types()
                    .is_none_or(|types| {
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
