//! The user's input language and keyboard layout, which clients tell hosts in
//! `KEYBOARD_INFO` so that keys produce the characters printed on them
//! (Section 8.2.3).

use qdp::message::{KeyboardInfo, LayoutPlatform};

/// The environment variables that name the user's locale, most specific
/// first.
const LOCALE_VARIABLES: [&str; 3] = ["LC_ALL", "LC_MESSAGES", "LANG"];

/// The environment variable some desktops set to the XKB layout.
#[cfg(target_os = "linux")]
const XKB_LAYOUT_VARIABLE: &str = "XKB_DEFAULT_LAYOUT";

/// The file Debian-based systems keep the console and X11 keyboard layout in.
#[cfg(target_os = "linux")]
const KEYBOARD_DEFAULTS: &str = "/etc/default/keyboard";

/// Returns a POSIX locale name, such as `en_GB.UTF-8`, as a BCP 47 tag, such
/// as `en-GB`, or `None` for the C and POSIX locales.
fn bcp47_from_posix(locale: &str) -> Option<String> {
    let name = locale.split(['.', '@']).next()?;
    if name.is_empty() || name == "C" || name == "POSIX" {
        return None;
    }
    Some(name.replace('_', "-"))
}

/// Returns the user's input language from the locale environment.
fn language_from_environment() -> Option<String> {
    LOCALE_VARIABLES
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find_map(|locale| bcp47_from_posix(&locale))
}

/// Returns the XKB layout named by a `/etc/default/keyboard` file's text.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn xkb_layout_from_defaults(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("XKBLAYOUT="))
        .map(|value| {
            value
                .trim_matches('"')
                .split(',')
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .find(|layout| !layout.is_empty())
}

/// Returns the user's keyboard layout on Linux: the XKB layout, where the
/// environment or the system's keyboard defaults name it.
#[cfg(target_os = "linux")]
fn layout() -> (LayoutPlatform, String) {
    let layout = std::env::var(XKB_LAYOUT_VARIABLE)
        .ok()
        .filter(|layout| !layout.is_empty())
        .or_else(|| {
            std::fs::read_to_string(KEYBOARD_DEFAULTS)
                .ok()
                .and_then(|text| xkb_layout_from_defaults(&text))
        });
    match layout {
        Some(layout) => (LayoutPlatform::XKB, layout),
        None => (LayoutPlatform::UNKNOWN, String::new()),
    }
}

/// Returns the user's keyboard layout on Windows: the active layout's
/// identifier, such as `00000809`.
#[cfg(windows)]
#[allow(unsafe_code)]
fn layout() -> (LayoutPlatform, String) {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayoutNameW;
    let mut name = [0_u16; 9];
    // SAFETY: the buffer holds the nine characters the identifier takes.
    match unsafe { GetKeyboardLayoutNameW(&mut name) } {
        Ok(()) => {
            let length = name
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(name.len());
            (
                LayoutPlatform::WINDOWS,
                String::from_utf16_lossy(&name[..length]),
            )
        }
        Err(_) => (LayoutPlatform::UNKNOWN, String::new()),
    }
}

/// Returns the user's keyboard layout where the platform does not say.
#[cfg(not(any(target_os = "linux", windows)))]
fn layout() -> (LayoutPlatform, String) {
    (LayoutPlatform::UNKNOWN, String::new())
}

/// Returns the user's input language on Windows.
#[cfg(windows)]
#[allow(unsafe_code)]
fn language() -> Option<String> {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;
    /// The longest locale name, with its terminator (`LOCALE_NAME_MAX_LENGTH`).
    const LOCALE_NAME_MAX_LENGTH: usize = 85;
    let mut name = [0_u16; LOCALE_NAME_MAX_LENGTH];
    // SAFETY: the call writes at most the buffer's length.
    let length = unsafe { GetUserDefaultLocaleName(&mut name) };
    let length = usize::try_from(length).ok()?.checked_sub(1)?;
    Some(String::from_utf16_lossy(&name[..length])).filter(|name| !name.is_empty())
}

/// Returns the user's input language from the locale environment.
#[cfg(not(windows))]
fn language() -> Option<String> {
    language_from_environment()
}

/// Returns the client's `KEYBOARD_INFO`, if it knows the language or the
/// layout.
pub fn keyboard_info() -> Option<KeyboardInfo> {
    let language = language()
        .or_else(language_from_environment)
        .unwrap_or_default();
    let (layout_platform, layout) = layout();
    (!language.is_empty() || layout_platform != LayoutPlatform::UNKNOWN).then_some(KeyboardInfo {
        language,
        layout_platform,
        layout,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// POSIX locales become BCP 47 tags.
    #[test]
    fn converts_locales() {
        assert_eq!(bcp47_from_posix("en_GB.UTF-8").as_deref(), Some("en-GB"));
        assert_eq!(bcp47_from_posix("de_DE@euro").as_deref(), Some("de-DE"));
        assert_eq!(bcp47_from_posix("fr").as_deref(), Some("fr"));
        assert_eq!(bcp47_from_posix("C.UTF-8"), None);
        assert_eq!(bcp47_from_posix("POSIX"), None);
    }

    /// The first XKB layout of the system's defaults is taken.
    #[test]
    fn reads_keyboard_defaults() {
        let text = "XKBMODEL=\"pc105\"\nXKBLAYOUT=\"gb,us\"\nXKBVARIANT=\"\"\n";
        assert_eq!(xkb_layout_from_defaults(text).as_deref(), Some("gb"));
        assert_eq!(xkb_layout_from_defaults("XKBLAYOUT=\"\""), None);
    }
}
