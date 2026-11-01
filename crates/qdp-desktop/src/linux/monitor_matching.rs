//! Bringing the captured monitor to the client's display on Linux, when the
//! host is asked to: its mode and interface scale, changed through the
//! compositor's own display settings, and put back when the session ends.
//!
//! The settings the monitor had are saved to a file first, since some
//! compositors keep what is applied: a host that stops without putting them
//! back puts them back when it next starts. One session at a time changes
//! the monitor, so that sessions do not take each other's settings for the
//! monitor's own.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use qdp_runtime::WantedDisplay;
use tracing::{info, warn};

use super::kde_displays::KdeDisplays;
use crate::monitor_modes::{MonitorMode, MonitorSettings, SavedSettings, matching_settings};

/// Whether a session is changing a monitor.
static MATCHING: AtomicBool = AtomicBool::new(false);

/// A monitor's modes and settings.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct MonitorState {
    /// The modes it offers.
    pub(super) modes: Vec<MonitorMode>,
    /// Its settings.
    pub(super) current: MonitorSettings,
}

/// The compositor's display settings.
enum DisplaySettings {
    /// KDE Plasma's.
    Kde(KdeDisplays),
}

impl DisplaySettings {
    /// Connects to the compositor's display settings.
    fn open() -> Result<Self, String> {
        if let Some(kde) = KdeDisplays::open()? {
            return Ok(Self::Kde(kde));
        }
        Err(
            "the host cannot change this desktop's display settings yet; it can KDE Plasma's"
                .to_owned(),
        )
    }

    /// Returns the modes and settings of the monitor named `name`.
    fn monitor(&mut self, name: &str) -> Result<MonitorState, String> {
        match self {
            Self::Kde(kde) => kde.monitor(name),
        }
    }

    /// Gives the monitor named `name` `settings`.
    fn apply(&mut self, name: &str, settings: &MonitorSettings) -> Result<(), String> {
        match self {
            Self::Kde(kde) => kde.apply(name, settings),
        }
    }
}

/// Changes one monitor's mode and interface scale to the client's display,
/// and puts back its own when dropped.
#[derive(Debug)]
pub(crate) struct MonitorMatching {
    /// The monitor's name, as the compositor knows it.
    monitor: String,
    /// Where its own settings are saved while they are changed.
    saved_path: PathBuf,
    /// Its own settings, once changed.
    original: Option<MonitorSettings>,
}

impl MonitorMatching {
    /// Matches the monitor named `monitor`, saving its own settings at
    /// `saved_path` while they are changed. Fails while another session
    /// matches one.
    pub(crate) fn new(monitor: String, saved_path: PathBuf) -> Result<Self, String> {
        if MATCHING.swap(true, Ordering::AcqRel) {
            return Err("another session has the monitor take its client's display".to_owned());
        }
        Ok(Self {
            monitor,
            saved_path,
            original: None,
        })
    }

    /// Brings the monitor as close to `wanted` as its modes allow, and
    /// returns its settings after.
    pub(crate) fn apply(&mut self, wanted: &WantedDisplay) -> Result<MonitorSettings, String> {
        let mut displays = DisplaySettings::open()?;
        let state = displays.monitor(&self.monitor)?;
        let target = matching_settings(wanted, &state.modes, &state.current)
            .ok_or_else(|| format!("{} offers no modes", self.monitor))?;
        if target.same_as(&state.current) {
            return Ok(state.current);
        }
        if self.original.is_none() {
            let saved = SavedSettings {
                monitor: self.monitor.clone(),
                settings: state.current,
            };
            if let Err(error) = std::fs::write(&self.saved_path, saved.to_string()) {
                warn!(
                    %error,
                    path = %self.saved_path.display(),
                    "cannot save the monitor's own settings; they are put back only if the host \
                     stops cleanly"
                );
            }
            self.original = Some(state.current);
        }
        displays.apply(&self.monitor, &target)?;
        info!(
            monitor = self.monitor,
            width = target.mode.width,
            height = target.mode.height,
            refresh_millihertz = target.mode.refresh_millihertz,
            scale = target.scale,
            "the monitor takes the client's display"
        );
        Ok(target)
    }
}

impl Drop for MonitorMatching {
    fn drop(&mut self) {
        if let Some(original) = self.original.take() {
            let saved = SavedSettings {
                monitor: self.monitor.clone(),
                settings: original,
            };
            put_back(&saved, &self.saved_path);
        }
        MATCHING.store(false, Ordering::Release);
    }
}

/// Puts back the settings that a host which stopped without putting them
/// back saved at `saved_path`, if any.
pub(crate) fn restore_saved(saved_path: &Path) {
    let Ok(text) = std::fs::read_to_string(saved_path) else {
        return;
    };
    match text.parse::<SavedSettings>() {
        Ok(saved) => {
            info!(
                monitor = saved.monitor,
                "putting back the monitor settings an earlier session changed"
            );
            put_back(&saved, saved_path);
        }
        Err(error) => {
            warn!(%error, path = %saved_path.display(), "ignoring unreadable saved monitor settings");
            let _ = std::fs::remove_file(saved_path);
        }
    }
}

/// Gives the monitor `saved` names its saved settings, and removes the file
/// at `saved_path` that kept them once they are back.
fn put_back(saved: &SavedSettings, saved_path: &Path) {
    let restored = DisplaySettings::open()
        .and_then(|mut displays| displays.apply(&saved.monitor, &saved.settings));
    match restored {
        Ok(()) => {
            info!(
                monitor = saved.monitor,
                "the monitor's own settings are back"
            );
            let _ = std::fs::remove_file(saved_path);
        }
        // The file stays, so that the next start tries again.
        Err(error) => warn!(
            %error,
            monitor = saved.monitor,
            "the monitor's own settings cannot be put back"
        ),
    }
}
