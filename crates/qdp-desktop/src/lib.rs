//! Desktop integration for Quick Desktop Protocol (QDP) hosts: capturing the
//! screen and performing the client's input on each operating system.
//!
//! - Linux: `ConsoleCapture` captures what the graphics card shows, login
//!   screen included, for a host running as root, and `ScreenCapture`
//!   captures a monitor through the ScreenCast portal
//!   and PipeWire, and can change its mode and interface scale to the
//!   client's display, and `UinputInjector` creates virtual keyboard, mouse
//!   and pointer devices through uinput, which works under every Wayland
//!   compositor and X11 server.
//! - macOS: `MacScreenCapture` captures the main display with
//!   ScreenCaptureKit, and `QuartzInjector` posts Quartz events.
//! - Windows: `DesktopDuplication` captures the primary monitor with DXGI
//!   Desktop Duplication, `SendInputInjector` sends input with `SendInput`,
//!   and `WindowPen` reads the pen over a client's window.
//!
//! [`SystemClipboard`] shares the system clipboard's text on each of them,
//! [`host_audio`] and [`client_audio`] open the audio devices sessions play
//! and capture through, `GamepadCapture` watches a client's gamepads,
//! [`PassthroughCapture`] passes the devices the user chose through,
//! [`TouchpadCapture`] sends the touchpad's raw contacts, and
//! [`lock_desktop`] locks the desktop when a session ends unexpectedly,
//! [`host_privacy`] covers its displays and holds back its own input during
//! one, [`advertise`] advertises the host on the local network, and
//! [`notify`] shows the host's user a notification.

mod advertise;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod clipboard;
#[cfg(all(
    feature = "audio",
    any(target_os = "macos", target_os = "ios", windows, target_os = "android")
))]
mod cpal_audio;
mod desktop_lock;
#[cfg(feature = "gamepad")]
mod gamepad;
mod host_privacy;
mod keyboard_layout;
#[cfg(any(
    all(target_os = "linux", feature = "screen-capture"),
    target_os = "macos"
))]
mod latest_frame;
#[cfg(target_os = "linux")]
mod linux;
mod lock_keys;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", target_os = "ios"))]
#[allow(unsafe_code)]
mod metal_video;
#[cfg(all(target_os = "linux", feature = "screen-capture"))]
mod monitor_modes;
mod notification;
mod passthrough;
mod touchpad;
#[cfg(any(target_os = "macos", target_os = "ios", windows))]
mod video_scene;
#[cfg(windows)]
mod win32;

use std::path::Path;

pub use qdp_runtime::Microphone;
use qdp_runtime::{
    ClientAudio, Clipboard, DisplayTiming, FrameSource, HostAudio, InputInjector, NoInput,
};
use tracing::warn;

pub use advertise::{Advertisement, Advertising, advertise};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub use clipboard::SystemClipboard;
#[cfg(target_os = "linux")]
pub use clipboard::adopt_wayland_display;
pub use desktop_lock::lock_desktop;
#[cfg(feature = "gamepad")]
pub use gamepad::GamepadCapture;
pub use host_privacy::{HAS_PRIVACY, HostPrivacy, host_privacy};
pub use keyboard_layout::keyboard_info;
pub use lock_keys::lock_keys;
pub use notification::{Notification, Urgency, notify};
pub use passthrough::{DeviceSelector, PassthroughCapture};
pub use touchpad::TouchpadCapture;

#[cfg(target_os = "linux")]
pub use linux::UinputInjector;
#[cfg(all(target_os = "linux", feature = "screen-capture"))]
pub use linux::{ConsoleCapture, ScreenCapture};
#[cfg(target_os = "macos")]
pub use macos::{MacScreenCapture, QuartzInjector, mac_key_usage};
#[cfg(any(target_os = "macos", target_os = "ios"))]
pub use metal_video::{MetalVideo, Submitter};
#[cfg(any(target_os = "macos", target_os = "ios", windows))]
pub use video_scene::{Fate, Overlay, PixelRect, Scene};
#[cfg(windows)]
pub use win32::{
    D3d11Video, DesktopDuplication, SendInputInjector, Submitter, SyncMode, SystemKeys, WindowPen,
};

/// The file, in the host's state directory, that keeps the token restoring the
/// user's choice of monitor.
#[cfg_attr(
    not(all(target_os = "linux", feature = "screen-capture")),
    allow(dead_code)
)]
const RESTORE_TOKEN_FILE: &str = "screencast-restore-token";

/// The file, in the host's state directory, that keeps the captured
/// monitor's own settings while the host has changed them.
#[cfg_attr(
    not(all(target_os = "linux", feature = "screen-capture")),
    allow(dead_code)
)]
const MONITOR_SETTINGS_FILE: &str = "monitor-settings";

/// Whether the captured monitor takes the client's display for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DisplayMatching {
    /// The monitor keeps its own mode and scale, and the host scales its
    /// frames to the client's display.
    #[default]
    Off,
    /// The monitor's mode and interface scale change to come as close to
    /// the client's display as the monitor allows, and change back when the
    /// session ends.
    On,
}

/// Where captured frames are delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FrameMemory {
    /// In memory, where every encoder reads them.
    #[default]
    System,
    /// Left in the GPU's memory, where the desktop shares them so, for a
    /// hardware encoder that reads them there; in memory otherwise. Frames
    /// come in memory instead from the moment the encoder cannot read one
    /// (see [`FrameSource::use_memory_frames`]).
    Gpu,
}

/// How a session captures the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ScreenOptions {
    /// Whether the captured monitor takes the client's display.
    pub matching: DisplayMatching,
    /// Where captured frames are delivered.
    pub frames: FrameMemory,
}

/// Puts back the monitor settings that an earlier host, which stopped
/// without putting them back, saved in `state_directory`.
#[cfg(all(target_os = "linux", feature = "screen-capture"))]
pub fn restore_monitor_settings(state_directory: &Path) {
    linux::restore_monitor_settings(&state_directory.join(MONITOR_SETTINGS_FILE));
}

/// Puts back the monitor settings an earlier host saved; this platform
/// changes none.
#[cfg(not(all(target_os = "linux", feature = "screen-capture")))]
pub fn restore_monitor_settings(_state_directory: &Path) {}

/// Starts capturing the screen, keeping in `state_directory` what lets later
/// sessions start without asking the user again. Blocks until the first
/// frame arrives, which may wait for the user to choose a monitor. With
/// matching on, the monitor takes the client's display for the session.
#[cfg(all(target_os = "linux", feature = "screen-capture"))]
pub fn open_screen(
    state_directory: &Path,
    options: ScreenOptions,
) -> Result<Box<dyn FrameSource>, String> {
    let token_path = state_directory.join(RESTORE_TOKEN_FILE);
    let token = std::fs::read_to_string(&token_path)
        .ok()
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty());
    let mut capture = ScreenCapture::open(token, options.frames)?;
    if let Some(token) = capture.restore_token() {
        if let Err(error) = std::fs::write(&token_path, token) {
            warn!(%error, path = %token_path.display(), "cannot keep the screen sharing choice");
        }
    }
    if options.matching == DisplayMatching::On {
        let saved_path = state_directory.join(MONITOR_SETTINGS_FILE);
        if let Err(error) = capture.match_displays(saved_path) {
            warn!(%error, "the monitor keeps its own display; the host scales its frames");
        }
    }
    Ok(Box::new(capture))
}

/// Starts capturing the main display. Blocks until the first frame arrives.
/// The display keeps its own mode on this platform, and frames come in
/// memory.
#[cfg(target_os = "macos")]
pub fn open_screen(
    _state_directory: &Path,
    options: ScreenOptions,
) -> Result<Box<dyn FrameSource>, String> {
    warn_unmatched(options.matching);
    Ok(Box::new(MacScreenCapture::open()?))
}

/// Starts capturing the primary monitor. Blocks until the first image
/// arrives. The monitor keeps its own mode on this platform, and frames come
/// in memory.
#[cfg(windows)]
pub fn open_screen(
    _state_directory: &Path,
    options: ScreenOptions,
) -> Result<Box<dyn FrameSource>, String> {
    warn_unmatched(options.matching);
    Ok(Box::new(DesktopDuplication::open()?))
}

/// Says that the monitor keeps its own display, if matching was asked for on
/// a platform that cannot yet.
#[cfg(any(target_os = "macos", windows))]
fn warn_unmatched(matching: DisplayMatching) {
    if matching == DisplayMatching::On {
        warn!("this platform cannot change the monitor to the client's display yet");
    }
}

/// Starts capturing the console: what the graphics card shows on a display,
/// whatever shows it, from the login screen to a desktop, delivering frames
/// as `frames` says. Needs root. Blocks until the first frame is read.
#[cfg(all(target_os = "linux", feature = "screen-capture"))]
pub fn open_console(frames: FrameMemory) -> Result<Box<dyn FrameSource>, String> {
    Ok(Box::new(ConsoleCapture::open(frames)?))
}

/// Starts capturing the console; only Linux hosts can.
#[cfg(not(all(target_os = "linux", feature = "screen-capture")))]
pub fn open_console(_frames: FrameMemory) -> Result<Box<dyn FrameSource>, String> {
    Err("console capture is only available on Linux".to_owned())
}

/// Starts capturing the screen; this platform cannot yet.
#[cfg(not(any(
    all(target_os = "linux", feature = "screen-capture"),
    target_os = "macos",
    windows
)))]
pub fn open_screen(
    _state_directory: &Path,
    _options: ScreenOptions,
) -> Result<Box<dyn FrameSource>, String> {
    Err("screen capture is not supported on this platform yet".to_owned())
}

/// Returns a host's audio devices: the desktop's sound, which clients play,
/// and, where the platform allows, a virtual microphone that plays the
/// client's microphone to the host's applications.
#[cfg(all(feature = "audio", target_os = "linux"))]
pub fn host_audio() -> HostAudio {
    HostAudio {
        desktop: Some(Box::new(linux::PipeWireInput::desktop())),
        microphone: Some(Box::new(linux::PipeWireOutput::virtual_microphone())),
    }
}

/// Returns a host's audio devices: the desktop's sound, which clients play.
/// Presenting the client's microphone to applications would need a virtual
/// audio driver, so there is none.
#[cfg(all(feature = "audio", any(target_os = "macos", windows)))]
pub fn host_audio() -> HostAudio {
    HostAudio {
        desktop: cpal_audio::desktop_input(),
        microphone: None,
    }
}

/// Returns a host's audio devices: none without the `audio` feature.
#[cfg(not(all(
    feature = "audio",
    any(target_os = "linux", target_os = "macos", windows)
)))]
pub fn host_audio() -> HostAudio {
    HostAudio::default()
}

/// Returns a client's audio devices: its speakers, and its microphone,
/// which captures only while `microphone` says the user turned it on.
#[cfg(all(feature = "audio", target_os = "linux"))]
pub fn client_audio(microphone: Microphone) -> ClientAudio {
    ClientAudio {
        speakers: Some(Box::new(linux::PipeWireOutput::speakers())),
        microphone: Some(Box::new(linux::PipeWireInput::microphone())),
        microphone_state: microphone,
    }
}

/// Returns a client's audio devices: its speakers, and its microphone,
/// which captures only while `microphone` says the user turned it on.
#[cfg(all(
    feature = "audio",
    any(target_os = "macos", target_os = "ios", windows, target_os = "android")
))]
pub fn client_audio(microphone: Microphone) -> ClientAudio {
    ClientAudio {
        speakers: cpal_audio::speakers(),
        microphone: cpal_audio::microphone(),
        microphone_state: microphone,
    }
}

/// Returns a client's audio devices: none without the `audio` feature.
#[cfg(not(all(
    feature = "audio",
    any(
        target_os = "linux",
        target_os = "macos",
        target_os = "ios",
        windows,
        target_os = "android"
    )
)))]
pub fn client_audio(_microphone: Microphone) -> ClientAudio {
    ClientAudio::default()
}

/// Returns the refresh timing of the display windows are shown on, if the
/// platform tells it: on Windows, from the Desktop Window Manager.
#[cfg(windows)]
pub fn display_timing() -> Option<DisplayTiming> {
    win32::display_timing()
}

/// Returns the refresh timing of the display windows are shown on; this
/// platform does not tell it to applications yet.
#[cfg(not(windows))]
pub fn display_timing() -> Option<DisplayTiming> {
    None
}

/// Opens the system clipboard for sharing, or returns `None`, with a warning,
/// if it cannot be used.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn system_clipboard() -> Option<Box<dyn Clipboard>> {
    match SystemClipboard::open() {
        Ok(clipboard) => {
            tracing::info!(clipboard = clipboard.description(), "sharing the clipboard");
            Some(Box::new(clipboard))
        }
        Err(error) => {
            warn!(%error, "the clipboard will not be shared");
            None
        }
    }
}

/// Returns `None`: Android's and iOS's clipboards are reached through the
/// app, not here.
#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn system_clipboard() -> Option<Box<dyn Clipboard>> {
    None
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

/// Opens the platform's input injector: uinput on Linux, if its devices
/// stay with this session (Section 8.2.5).
#[cfg(target_os = "linux")]
fn platform_injector(width: u32, height: u32) -> Result<Box<dyn InputInjector>, String> {
    if !linux::devices_confined() {
        return Err(
            "virtual input devices would reach another user's session on seat0; \
             only the session of seat0's active user can inject input"
                .to_owned(),
        );
    }
    UinputInjector::new(width, height)
        .map(|injector| Box::new(injector) as Box<dyn InputInjector>)
        .map_err(|error| {
            format!(
                "cannot create virtual input devices in /dev/uinput ({error}); load the uinput \
                 module and give this user write access to /dev/uinput"
            )
        })
}

/// Opens the platform's input injector: Quartz events on macOS.
#[cfg(target_os = "macos")]
fn platform_injector(width: u32, height: u32) -> Result<Box<dyn InputInjector>, String> {
    Ok(Box::new(QuartzInjector::new(width, height)))
}

/// Opens the platform's input injector: `SendInput` on Windows.
#[cfg(windows)]
fn platform_injector(width: u32, height: u32) -> Result<Box<dyn InputInjector>, String> {
    Ok(Box::new(SendInputInjector::new(width, height)))
}

/// Opens the platform's input injector; this platform has none yet.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform_injector(_width: u32, _height: u32) -> Result<Box<dyn InputInjector>, String> {
    Err("input injection is not supported on this platform yet".to_owned())
}
