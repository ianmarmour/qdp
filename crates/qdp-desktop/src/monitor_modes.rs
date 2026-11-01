//! Bringing the host's monitor to the client's display, when the host is
//! asked to (`qdp serve --match-display`): the mode and interface scale that
//! come closest, and a record of the settings changed, to put back.

use std::fmt;
use std::str::FromStr;

use qdp_runtime::WantedDisplay;

/// The refresh rate wanted when the client does not say its own.
const DEFAULT_REFRESH_MILLIHERTZ: u32 = 60_000;

/// Percent in a scale of one.
const UNIT_PERCENT: f64 = 100.0;

/// The smallest interface scale set.
const MIN_SCALE: f64 = 0.5;

/// The largest interface scale set.
const MAX_SCALE: f64 = 4.0;

/// Scales closer than this are the same: compositors keep scales in steps of
/// 1/120.
const SCALE_TOLERANCE: f64 = 1.0 / 240.0;

/// The keys of a saved record.
mod keys {
    /// [`super::SavedSettings::monitor`].
    pub(super) const MONITOR: &str = "monitor";
    /// [`super::MonitorMode::width`].
    pub(super) const WIDTH: &str = "width";
    /// [`super::MonitorMode::height`].
    pub(super) const HEIGHT: &str = "height";
    /// [`super::MonitorMode::refresh_millihertz`].
    pub(super) const REFRESH: &str = "refresh_millihertz";
    /// [`super::MonitorSettings::scale`].
    pub(super) const SCALE: &str = "scale";
}

/// One mode a monitor offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct MonitorMode {
    /// The width in pixels.
    pub(crate) width: u32,
    /// The height in pixels.
    pub(crate) height: u32,
    /// The refresh rate in millihertz.
    pub(crate) refresh_millihertz: u32,
}

impl MonitorMode {
    /// Returns the mode's width over its height.
    fn aspect(&self) -> f64 {
        f64::from(self.width) / f64::from(self.height.max(1))
    }

    /// Returns the mode's number of pixels.
    fn area(&self) -> f64 {
        f64::from(self.width) * f64::from(self.height)
    }
}

/// Which modes a monitor takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ModeRange {
    /// Only the ones it lists, as a real monitor.
    Listed,
    /// Any size, as a headless or virtual monitor.
    Any,
}

/// The settings of a monitor that matching changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct MonitorSettings {
    /// Its mode.
    pub(crate) mode: MonitorMode,
    /// Its interface scale, 1 for none.
    pub(crate) scale: f64,
}

impl MonitorSettings {
    /// Returns whether `other` is the same settings, with scales that differ
    /// by less than compositors keep.
    pub(crate) fn same_as(&self, other: &Self) -> bool {
        self.mode == other.mode && (self.scale - other.scale).abs() < SCALE_TOLERANCE
    }
}

/// Returns the settings that bring a monitor offering `modes`, or any mode as
/// `range` says, whose settings are `current`, closest to `wanted`: its size
/// exactly if the monitor takes any, or else the listed mode closest to it,
/// by shape first and then by area, at the refresh rate closest to its own;
/// and its interface scale, or the current one if the client did not say.
/// Returns `None` if the monitor takes only listed modes and lists none.
pub(crate) fn matching_settings(
    wanted: &WantedDisplay,
    modes: &[MonitorMode],
    range: ModeRange,
    current: &MonitorSettings,
) -> Option<MonitorSettings> {
    let target = MonitorMode {
        width: wanted.width.max(1),
        height: wanted.height.max(1),
        refresh_millihertz: if wanted.refresh_millihertz == 0 {
            DEFAULT_REFRESH_MILLIHERTZ
        } else {
            wanted.refresh_millihertz
        },
    };
    let scale = if wanted.scale_percent == 0 {
        current.scale
    } else {
        (f64::from(wanted.scale_percent) / UNIT_PERCENT).clamp(MIN_SCALE, MAX_SCALE)
    };
    if range == ModeRange::Any {
        return Some(MonitorSettings {
            mode: target,
            scale,
        });
    }
    // How far a size is from the wanted one: how much its shape differs,
    // and how much its area does, each as a ratio.
    let size_distance = |mode: &MonitorMode| {
        (mode.aspect() / target.aspect()).ln().abs() + (mode.area() / target.area()).ln().abs()
    };
    let size = modes
        .iter()
        .min_by(|a, b| size_distance(a).total_cmp(&size_distance(b)))?;
    let refresh_distance =
        |mode: &MonitorMode| mode.refresh_millihertz.abs_diff(target.refresh_millihertz);
    let mode = modes
        .iter()
        .filter(|mode| (mode.width, mode.height) == (size.width, size.height))
        .min_by(|a, b| {
            refresh_distance(a)
                .cmp(&refresh_distance(b))
                .then(b.refresh_millihertz.cmp(&a.refresh_millihertz))
        })?;
    Some(MonitorSettings { mode: *mode, scale })
}

/// The settings a monitor had before the host changed them, kept on disk
/// while they are changed, so that a host that stops without putting them
/// back can still.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SavedSettings {
    /// The monitor's name, as the compositor knows it.
    pub(crate) monitor: String,
    /// Its settings before the change.
    pub(crate) settings: MonitorSettings,
}

impl fmt::Display for SavedSettings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mode = &self.settings.mode;
        writeln!(formatter, "{}={}", keys::MONITOR, self.monitor)?;
        writeln!(formatter, "{}={}", keys::WIDTH, mode.width)?;
        writeln!(formatter, "{}={}", keys::HEIGHT, mode.height)?;
        writeln!(formatter, "{}={}", keys::REFRESH, mode.refresh_millihertz)?;
        writeln!(formatter, "{}={}", keys::SCALE, self.settings.scale)
    }
}

impl FromStr for SavedSettings {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let value = |key: &str| {
            text.lines()
                .filter_map(|line| line.split_once('='))
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.trim())
                .ok_or_else(|| format!("the saved monitor settings have no {key}"))
        };
        let number = |key: &str| {
            value(key)?
                .parse::<u32>()
                .map_err(|error| format!("the saved monitor {key} is not a number: {error}"))
        };
        Ok(Self {
            monitor: value(keys::MONITOR)?.to_owned(),
            settings: MonitorSettings {
                mode: MonitorMode {
                    width: number(keys::WIDTH)?,
                    height: number(keys::HEIGHT)?,
                    refresh_millihertz: number(keys::REFRESH)?,
                },
                scale: value(keys::SCALE)?
                    .parse()
                    .map_err(|error| format!("the saved monitor scale is not a number: {error}"))?,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a mode.
    fn mode(width: u32, height: u32, refresh_hertz: u32) -> MonitorMode {
        MonitorMode {
            width,
            height,
            refresh_millihertz: refresh_hertz * 1000,
        }
    }

    /// Returns the display a client wants.
    fn wanted(width: u32, height: u32, refresh_hertz: u32, scale_percent: u32) -> WantedDisplay {
        WantedDisplay {
            width,
            height,
            refresh_millihertz: refresh_hertz * 1000,
            scale_percent,
        }
    }

    /// The modes of a 5120x2160 monitor.
    fn ultrawide() -> Vec<MonitorMode> {
        vec![
            mode(5120, 2160, 60),
            mode(5120, 2160, 50),
            mode(3840, 2160, 60),
            mode(2560, 1080, 60),
            mode(1920, 1080, 60),
            mode(1920, 1080, 50),
            mode(1280, 720, 60),
        ]
    }

    /// The monitor's current settings.
    fn current() -> MonitorSettings {
        MonitorSettings {
            mode: mode(5120, 2160, 60),
            scale: 1.5,
        }
    }

    /// A size the monitor offers is taken exactly, at the refresh rate
    /// closest to the client's, with the client's scale.
    #[test]
    fn takes_the_size_asked_for() {
        let settings = matching_settings(
            &wanted(1920, 1080, 60, 100),
            &ultrawide(),
            ModeRange::Listed,
            &current(),
        )
        .unwrap();
        assert_eq!(settings.mode, mode(1920, 1080, 60));
        assert_eq!(settings.scale, 1.0);
        let settings = matching_settings(
            &wanted(1920, 1080, 50, 200),
            &ultrawide(),
            ModeRange::Listed,
            &current(),
        )
        .unwrap();
        assert_eq!(settings.mode, mode(1920, 1080, 50));
        assert_eq!(settings.scale, 2.0);
    }

    /// Another size is the mode closest in shape and area: a MacBook's
    /// display takes the 16:9 mode of about its size, not the ultrawide
    /// one or a small one of nearer shape.
    #[test]
    fn takes_the_closest_size_otherwise() {
        let settings = matching_settings(
            &wanted(3456, 2234, 120, 200),
            &ultrawide(),
            ModeRange::Listed,
            &current(),
        )
        .unwrap();
        assert_eq!(settings.mode, mode(3840, 2160, 60));
        let settings = matching_settings(
            &wanted(5000, 2100, 0, 0),
            &ultrawide(),
            ModeRange::Listed,
            &current(),
        )
        .unwrap();
        assert_eq!(settings.mode, mode(5120, 2160, 60));
        // An unsaid scale keeps the monitor's own.
        assert_eq!(settings.scale, 1.5);
        assert!(
            matching_settings(
                &wanted(1920, 1080, 60, 100),
                &[],
                ModeRange::Listed,
                &current()
            )
            .is_none()
        );
    }

    /// A monitor that takes any mode, as a headless one, takes the client's
    /// size exactly.
    #[test]
    fn takes_any_size_where_it_can() {
        let settings = matching_settings(
            &wanted(3456, 2234, 120, 200),
            &[],
            ModeRange::Any,
            &current(),
        )
        .unwrap();
        assert_eq!(settings.mode, mode(3456, 2234, 120));
        assert_eq!(settings.scale, 2.0);
    }

    /// Scales are kept within what compositors take and compared as they
    /// keep them.
    #[test]
    fn keeps_scales_in_range() {
        let settings = matching_settings(
            &wanted(1920, 1080, 60, 1000),
            &ultrawide(),
            ModeRange::Listed,
            &current(),
        )
        .unwrap();
        assert_eq!(settings.scale, MAX_SCALE);
        let near = MonitorSettings {
            scale: 1.5 + 1.0 / 1000.0,
            ..current()
        };
        assert!(near.same_as(&current()));
        let other = MonitorSettings {
            scale: 1.25,
            ..current()
        };
        assert!(!other.same_as(&current()));
    }

    /// Saved settings read back as they were written.
    #[test]
    fn reads_back_saved_settings() {
        let saved = SavedSettings {
            monitor: "DP-1".to_owned(),
            settings: MonitorSettings {
                mode: mode(5120, 2160, 60),
                scale: 1.25,
            },
        };
        assert_eq!(saved.to_string().parse::<SavedSettings>(), Ok(saved));
        assert!("monitor=DP-1\n".parse::<SavedSettings>().is_err());
    }
}
