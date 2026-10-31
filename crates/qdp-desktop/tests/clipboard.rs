//! Shares text through the system clipboard of a running desktop session, and
//! hides content a password manager marks as secret. It needs a desktop
//! session (on Linux, a Wayland compositor with the data control protocol or
//! an X server) whose clipboard the test may overwrite, so it is ignored by
//! default; run it with `--ignored` inside one.

#![cfg(any(target_os = "linux", target_os = "macos", windows))]

#[cfg(target_os = "macos")]
use arboard::SetExtApple;
#[cfg(target_os = "linux")]
use arboard::SetExtLinux;
#[cfg(windows)]
use arboard::SetExtWindows;
use qdp_desktop::SystemClipboard;
use qdp_runtime::Clipboard;

/// Text written to the clipboard is read back, and text a password manager
/// marks as secret is not.
#[test]
#[ignore = "needs a desktop session whose clipboard the test may overwrite"]
fn shares_text_and_hides_secrets() {
    let mut clipboard = SystemClipboard::open().unwrap();
    clipboard.set_text("qdp clipboard test").unwrap();
    assert_eq!(
        clipboard.text().unwrap().as_deref(),
        Some("qdp clipboard test")
    );

    let mut password_manager = arboard::Clipboard::new().unwrap();
    let secret = password_manager.set();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let secret = secret.exclude_from_history();
    #[cfg(windows)]
    let secret = secret.exclude_from_monitoring();
    secret.text("hunter2").unwrap();
    assert_eq!(clipboard.text().unwrap(), None);
}
