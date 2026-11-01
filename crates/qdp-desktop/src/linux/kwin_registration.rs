//! Registering a program with KWin. KWin offers its restricted interfaces,
//! as the one that makes virtual monitors, only to programs that a desktop
//! entry lists them for: KWin finds the entry whose `Exec` names the
//! program's own path, with symlinks resolved, and reads the interfaces from
//! its `X-KDE-Wayland-Interfaces` key. The host writes such an entry for its
//! virtual monitor helper, hidden from menus, and has KDE refresh its
//! service database.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tracing::{debug, info};

use super::tools;

/// The desktop entry's file name.
const DESKTOP_FILE: &str = "qdp-kwin-monitor.desktop";

/// The directory under the user's data directory that holds desktop entries.
const APPLICATIONS_DIRECTORY: &str = "applications";

/// The user's data directory, when the environment names it.
const DATA_HOME_VARIABLE: &str = "XDG_DATA_HOME";

/// The user's data directory under their home, otherwise.
const DEFAULT_DATA_HOME: &str = ".local/share";

/// The tools that rebuild KDE's service database, newest first.
const SERVICE_DATABASE_TOOLS: [&str; 2] = ["kbuildsycoca6", "kbuildsycoca5"];

/// How long rebuilding the service database may take.
const SERVICE_DATABASE_TIMEOUT: Duration = Duration::from_secs(30);

/// Characters a path cannot hold in a desktop entry's `Exec`: KWin splits
/// the command as Qt does, with double quotes and no escapes, and `%` starts
/// a field code.
const UNWRITABLE: &[char] = &['"', '\\', '%'];

/// Writes the desktop entry that lists `interfaces` for `program`, if it
/// does not already, and refreshes KDE's service database so that KWin finds
/// it. Returns the entry's path.
pub(super) fn register(program: &Path, interfaces: &[&str]) -> Result<PathBuf, String> {
    let executable = program
        .canonicalize()
        .map_err(|error| format!("cannot find {}: {error}", program.display()))?;
    let directory = applications_directory()
        .ok_or_else(|| "cannot find the user's applications directory".to_owned())?;
    let path = directory.join(DESKTOP_FILE);
    let contents = desktop_entry(&executable, interfaces).ok_or_else(|| {
        format!(
            "KWin cannot be told about {}, as its path holds a quote, a backslash or a \
             percent sign",
            executable.display()
        )
    })?;
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == contents) {
        debug!(entry = %path.display(), "the helper's desktop entry is up to date");
    } else {
        std::fs::create_dir_all(&directory)
            .and_then(|()| std::fs::write(&path, contents))
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        info!(
            entry = %path.display(),
            program = %executable.display(),
            "registered the virtual monitor helper with KWin"
        );
    }
    // KDE's service database also rebuilds itself when it notices the
    // entry; the tool makes it happen now, and catches up a database that
    // missed an entry written before.
    for tool in SERVICE_DATABASE_TOOLS {
        match tools::run(tool, [], SERVICE_DATABASE_TIMEOUT) {
            Ok(()) => break,
            Err(error) => debug!(%error, "the service database was not rebuilt"),
        }
    }
    Ok(path)
}

/// Returns the user's directory of desktop entries.
fn applications_directory() -> Option<PathBuf> {
    let data_home = std::env::var_os(DATA_HOME_VARIABLE)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(DEFAULT_DATA_HOME))
        })?;
    Some(data_home.join(APPLICATIONS_DIRECTORY))
}

/// Returns the desktop entry that lists `interfaces` for `executable`, or
/// `None` if the path cannot be written in one.
fn desktop_entry(executable: &Path, interfaces: &[&str]) -> Option<String> {
    Some(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=QDP virtual monitors\n\
         Comment=Makes virtual monitors for the QDP host\n\
         Exec={}\n\
         NoDisplay=true\n\
         X-KDE-Wayland-Interfaces={}\n",
        exec_argument(executable)?,
        interfaces.join(","),
    ))
}

/// Returns `path` as the first argument of a desktop entry's `Exec`, in
/// double quotes if it holds spaces, or `None` if it cannot be written.
fn exec_argument(path: &Path) -> Option<String> {
    let path = path.to_str()?;
    if path.contains(|character: char| character.is_control() || UNWRITABLE.contains(&character)) {
        return None;
    }
    if path.contains(char::is_whitespace) {
        Some(format!("\"{path}\""))
    } else {
        Some(path.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The entry names the program as its command and lists the interfaces;
    /// paths with spaces are quoted, and ones KWin cannot read are refused.
    #[test]
    fn writes_desktop_entries() {
        let entry = desktop_entry(
            Path::new("/home/ian/qdp/target/release/qdp-kwin-monitor"),
            &["zkde_screencast_unstable_v1", "org_kde_kwin_fake_input"],
        )
        .unwrap();
        assert!(entry.starts_with("[Desktop Entry]\n"));
        assert!(entry.contains("\nExec=/home/ian/qdp/target/release/qdp-kwin-monitor\n"));
        assert!(entry.contains(
            "\nX-KDE-Wayland-Interfaces=zkde_screencast_unstable_v1,org_kde_kwin_fake_input\n"
        ));
        assert!(entry.contains("\nNoDisplay=true\n"));
        assert_eq!(
            exec_argument(Path::new("/home/ian/my programs/qdp")).as_deref(),
            Some("\"/home/ian/my programs/qdp\"")
        );
        for unwritable in [
            "/home/ian/q\"dp",
            "/home/ian/q\\dp",
            "/home/ian/100%/qdp",
            "/a\nb",
        ] {
            assert_eq!(exec_argument(Path::new(unwritable)), None, "{unwritable}");
        }
    }
}
