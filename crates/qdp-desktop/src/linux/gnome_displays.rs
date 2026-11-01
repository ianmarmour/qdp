//! GNOME's display settings, through Mutter's `org.gnome.Mutter.DisplayConfig`
//! D-Bus interface, as GNOME Settings changes them: each monitor's modes,
//! its mode and interface scale, and new ones applied.
//!
//! Mutter disables the monitors a configuration leaves out, so each one
//! carries every logical monitor over, the others where they were, moved
//! only as far as the changed one grew or shrank beside them. The settings
//! are applied for the session only, without GNOME's prompt to keep them.

use std::collections::HashMap;

use ashpd::zbus;
use ashpd::zvariant::{OwnedValue, Value};

use super::monitor_matching::MonitorState;
use crate::monitor_modes::{ModeRange, MonitorMode, MonitorSettings};

/// Mutter's bus name.
const DESTINATION: &str = "org.gnome.Mutter.DisplayConfig";

/// Its object path.
const PATH: &str = "/org/gnome/Mutter/DisplayConfig";

/// Its interface.
const INTERFACE: &str = "org.gnome.Mutter.DisplayConfig";

/// The error D-Bus returns when no program owns a name.
const SERVICE_UNKNOWN: &str = "org.freedesktop.DBus.Error.ServiceUnknown";

/// The configuration method that applies settings for the session only,
/// without asking the user to keep them.
const METHOD_TEMPORARY: u32 = 1;

/// The layout mode in which a monitor's place in the desktop is its size
/// over its scale; in the other, it is its size.
const LAYOUT_LOGICAL: u32 = 1;

/// Millihertz in a hertz.
const MILLIHERTZ: f64 = 1000.0;

/// A monitor's identity: its connector, vendor, product and serial.
type WireSpec = (String, String, String, String);

/// A mode as Mutter describes it: its ID, width, height, refresh rate in
/// hertz, preferred scale, the scales it takes, and properties.
type WireMode = (String, i32, i32, f64, f64, Vec<f64>, HashMap<String, OwnedValue>);

/// A monitor as Mutter describes it: its identity, modes and properties.
type WireMonitor = (WireSpec, Vec<WireMode>, HashMap<String, OwnedValue>);

/// A logical monitor as Mutter describes it: its place, scale, transform,
/// whether it is the primary one, its monitors, and properties.
type WireLogical = (i32, i32, f64, u32, bool, Vec<WireSpec>, HashMap<String, OwnedValue>);

/// Mutter's description of the displays: its serial, the monitors, the
/// logical monitors, and properties.
type WireState = (
    u32,
    Vec<WireMonitor>,
    Vec<WireLogical>,
    HashMap<String, OwnedValue>,
);

/// A monitor in a configuration: its connector, mode ID and properties.
type WireMonitorConfig<'a> = (String, String, HashMap<String, Value<'a>>);

/// A logical monitor in a configuration: its place, scale, transform,
/// whether it is the primary one, and its monitors.
type WireLogicalConfig<'a> = (i32, i32, f64, u32, bool, Vec<WireMonitorConfig<'a>>);

/// One mode of a monitor.
#[derive(Debug, Clone, PartialEq)]
struct Mode {
    /// Mutter's ID for it.
    id: String,
    /// Its size and refresh rate.
    mode: MonitorMode,
    /// The interface scales it takes.
    scales: Vec<f64>,
    /// Whether the monitor has it now.
    current: bool,
}

/// One monitor.
#[derive(Debug, Clone, PartialEq)]
struct Monitor {
    /// Its connector, which names it.
    connector: String,
    /// Its modes.
    modes: Vec<Mode>,
}

impl Monitor {
    /// Returns its current mode.
    fn current(&self) -> Option<&Mode> {
        self.modes.iter().find(|mode| mode.current)
    }
}

/// One logical monitor: a place in the desktop that monitors show.
#[derive(Debug, Clone, PartialEq)]
struct Logical {
    /// Its left edge.
    x: i32,
    /// Its top edge.
    y: i32,
    /// Its interface scale.
    scale: f64,
    /// How it is turned.
    transform: u32,
    /// Whether it is the primary one.
    primary: bool,
    /// The connectors of the monitors that show it.
    connectors: Vec<String>,
}

/// Mutter's displays.
#[derive(Debug, Clone, PartialEq)]
struct Displays {
    /// The serial a configuration must name.
    serial: u32,
    /// The monitors.
    monitors: Vec<Monitor>,
    /// The logical monitors.
    logical: Vec<Logical>,
    /// Whether a monitor's place in the desktop is its size over its scale.
    scaled_layout: bool,
}

impl Displays {
    /// Reads Mutter's description.
    fn from_wire((serial, monitors, logical, properties): WireState) -> Self {
        let flag = |properties: &HashMap<String, OwnedValue>, name: &str| {
            properties
                .get(name)
                .and_then(|value| value.downcast_ref::<bool>().ok())
                .unwrap_or(false)
        };
        let monitors = monitors
            .into_iter()
            .map(|((connector, ..), modes, _)| Monitor {
                connector,
                modes: modes
                    .into_iter()
                    .filter_map(|(id, width, height, refresh, _, scales, properties)| {
                        Some(Mode {
                            current: flag(&properties, "is-current"),
                            id,
                            mode: MonitorMode {
                                width: u32::try_from(width).ok()?,
                                height: u32::try_from(height).ok()?,
                                refresh_millihertz: (refresh * MILLIHERTZ).round() as u32,
                            },
                            scales,
                        })
                    })
                    .collect(),
            })
            .collect();
        let logical = logical
            .into_iter()
            .map(|(x, y, scale, transform, primary, specs, _)| Logical {
                x,
                y,
                scale,
                transform,
                primary,
                connectors: specs.into_iter().map(|(connector, ..)| connector).collect(),
            })
            .collect();
        let layout_mode = properties
            .get("layout-mode")
            .and_then(|value| value.downcast_ref::<u32>().ok());
        Self {
            serial,
            monitors,
            logical,
            scaled_layout: layout_mode.is_none_or(|mode| mode == LAYOUT_LOGICAL),
        }
    }

    /// Returns the monitor named `connector`.
    fn monitor(&self, connector: &str) -> Result<&Monitor, String> {
        self.monitors
            .iter()
            .find(|monitor| monitor.connector == connector)
            .ok_or_else(|| format!("GNOME has no monitor named {connector}"))
    }

    /// Returns the size a monitor of `mode` at `scale`, turned by
    /// `transform`, takes in the desktop.
    fn layout_size(&self, mode: &MonitorMode, scale: f64, transform: u32) -> (i32, i32) {
        // Odd transforms turn the monitor a quarter.
        let (width, height) = if transform % 2 == 1 {
            (mode.height, mode.width)
        } else {
            (mode.width, mode.height)
        };
        let divisor = if self.scaled_layout { scale } else { 1.0 };
        let side = |pixels: u32| (f64::from(pixels) / divisor).round() as i32;
        (side(width), side(height))
    }

    /// Returns the configuration that gives the monitor `connector` the
    /// mode `mode_id` of `mode` at `scale`, with every other logical monitor
    /// as it is, those beyond its right or bottom edge moved by as much as
    /// it grew or shrank.
    fn configuration(
        &self,
        connector: &str,
        mode_id: &str,
        mode: &MonitorMode,
        scale: f64,
    ) -> Result<Vec<WireLogicalConfig<'static>>, String> {
        let target = self
            .logical
            .iter()
            .find(|logical| logical.connectors.iter().any(|name| name == connector))
            .ok_or_else(|| format!("{connector} shows no part of the desktop"))?;
        if target.connectors.len() > 1 {
            return Err(format!("{connector} mirrors another monitor"));
        }
        let old_mode = self
            .monitor(connector)?
            .current()
            .ok_or_else(|| format!("GNOME did not say {connector}'s mode"))?;
        let old = self.layout_size(&old_mode.mode, target.scale, target.transform);
        let new = self.layout_size(mode, scale, target.transform);
        let (grown_x, grown_y) = (new.0 - old.0, new.1 - old.1);
        let (right, bottom) = (target.x + old.0, target.y + old.1);
        self.logical
            .iter()
            .map(|logical| {
                let monitors = logical
                    .connectors
                    .iter()
                    .map(|name| {
                        let id = if name == connector {
                            mode_id.to_owned()
                        } else {
                            self.monitor(name)?
                                .current()
                                .map(|mode| mode.id.clone())
                                .ok_or_else(|| format!("GNOME did not say {name}'s mode"))?
                        };
                        Ok((name.clone(), id, HashMap::new()))
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let ours = logical == target;
                let x = if !ours && logical.x >= right {
                    logical.x + grown_x
                } else {
                    logical.x
                };
                let y = if !ours && logical.y >= bottom {
                    logical.y + grown_y
                } else {
                    logical.y
                };
                let scale = if ours { scale } else { logical.scale };
                Ok((x, y, scale, logical.transform, logical.primary, monitors))
            })
            .collect()
    }
}

/// Returns the scale among `scales` nearest `wanted`, or `wanted` if there
/// are none.
fn nearest_scale(scales: &[f64], wanted: f64) -> f64 {
    scales
        .iter()
        .copied()
        .min_by(|a, b| (a - wanted).abs().total_cmp(&(b - wanted).abs()))
        .unwrap_or(wanted)
}

/// A connection to GNOME's display settings.
pub(super) struct GnomeDisplays {
    /// The runtime the D-Bus calls run on.
    runtime: tokio::runtime::Runtime,
    /// The session bus.
    connection: zbus::Connection,
    /// Mutter's displays, as last read.
    displays: Displays,
}

impl GnomeDisplays {
    /// Connects to Mutter and reads its displays, or returns `None` if the
    /// desktop is not GNOME.
    pub(super) fn open() -> Result<Option<Self>, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("cannot start the D-Bus runtime: {error}"))?;
        let Ok(connection) = runtime.block_on(zbus::Connection::session()) else {
            return Ok(None);
        };
        let displays = match runtime.block_on(read_displays(&connection)) {
            Ok(displays) => displays,
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == SERVICE_UNKNOWN => {
                return Ok(None);
            }
            Err(error) => return Err(format!("cannot read GNOME's displays: {error}")),
        };
        Ok(Some(Self {
            runtime,
            connection,
            displays,
        }))
    }

    /// Returns the modes and settings of the monitor named `name`.
    pub(super) fn monitor(&self, name: &str) -> Result<MonitorState, String> {
        let monitor = self.displays.monitor(name)?;
        let current = monitor
            .current()
            .ok_or_else(|| format!("GNOME did not say {name}'s mode"))?;
        let scale = self
            .displays
            .logical
            .iter()
            .find(|logical| logical.connectors.iter().any(|connector| connector == name))
            .map_or(1.0, |logical| logical.scale);
        Ok(MonitorState {
            modes: monitor.modes.iter().map(|mode| mode.mode).collect(),
            range: ModeRange::Listed,
            current: MonitorSettings {
                mode: current.mode,
                scale,
            },
        })
    }

    /// Gives the monitor named `name` `settings`, which must be of a mode it
    /// offers, at the scale nearest theirs that the mode takes.
    pub(super) fn apply(&mut self, name: &str, settings: &MonitorSettings) -> Result<(), String> {
        self.displays = self
            .runtime
            .block_on(read_displays(&self.connection))
            .map_err(|error| format!("cannot read GNOME's displays: {error}"))?;
        let mode = self
            .displays
            .monitor(name)?
            .modes
            .iter()
            .find(|mode| mode.mode == settings.mode)
            .ok_or_else(|| format!("{name} does not offer the mode {:?}", settings.mode))?;
        let scale = nearest_scale(&mode.scales, settings.scale);
        let configuration = self
            .displays
            .configuration(name, &mode.id, &mode.mode, scale)?;
        let arguments = (
            self.displays.serial,
            METHOD_TEMPORARY,
            configuration,
            HashMap::<String, Value<'static>>::new(),
        );
        self.runtime
            .block_on(self.connection.call_method(
                Some(DESTINATION),
                PATH,
                Some(INTERFACE),
                "ApplyMonitorsConfig",
                &arguments,
            ))
            .map_err(|error| format!("GNOME refused the display settings: {error}"))?;
        Ok(())
    }
}

/// Reads Mutter's displays.
async fn read_displays(connection: &zbus::Connection) -> zbus::Result<Displays> {
    let reply = connection
        .call_method(
            Some(DESTINATION),
            PATH,
            Some(INTERFACE),
            "GetCurrentState",
            &(),
        )
        .await?;
    Ok(Displays::from_wire(reply.body().deserialize::<WireState>()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a mode of `width` by `height` at 60 Hz.
    fn mode(id: &str, width: u32, height: u32, current: bool) -> Mode {
        Mode {
            id: id.to_owned(),
            mode: MonitorMode {
                width,
                height,
                refresh_millihertz: 60_000,
            },
            scales: vec![1.0, 1.25, 2.0],
            current,
        }
    }

    /// Two monitors side by side: a 5120x2160 one at 125 % and a 1920x1080
    /// one to its right.
    fn side_by_side() -> Displays {
        Displays {
            serial: 7,
            monitors: vec![
                Monitor {
                    connector: "DP-1".to_owned(),
                    modes: vec![
                        mode("5120x2160@60", 5120, 2160, true),
                        mode("1920x1080@60", 1920, 1080, false),
                    ],
                },
                Monitor {
                    connector: "HDMI-1".to_owned(),
                    modes: vec![mode("1920x1080@60", 1920, 1080, true)],
                },
            ],
            logical: vec![
                Logical {
                    x: 0,
                    y: 0,
                    scale: 1.25,
                    transform: 0,
                    primary: true,
                    connectors: vec!["DP-1".to_owned()],
                },
                Logical {
                    x: 4096,
                    y: 0,
                    scale: 1.0,
                    transform: 0,
                    primary: false,
                    connectors: vec!["HDMI-1".to_owned()],
                },
            ],
            scaled_layout: true,
        }
    }

    /// The changed monitor takes its new mode and scale, and the monitor to
    /// its right moves to its new edge, keeping its own mode.
    #[test]
    fn carries_the_other_monitors_over() {
        let displays = side_by_side();
        let small = displays.monitors[0].modes[1].clone();
        let configuration = displays
            .configuration("DP-1", &small.id, &small.mode, 1.0)
            .unwrap();
        assert_eq!(configuration.len(), 2);
        let (x, y, scale, _, primary, monitors) = &configuration[0];
        assert_eq!((*x, *y, *scale, *primary), (0, 0, 1.0, true));
        assert_eq!(monitors[0].0, "DP-1");
        assert_eq!(monitors[0].1, "1920x1080@60");
        let (x, _, scale, _, _, monitors) = &configuration[1];
        // 5120 / 1.25 = 4096 before, 1920 after.
        assert_eq!((*x, *scale), (1920, 1.0));
        assert_eq!(monitors[0].1, "1920x1080@60");
    }

    /// Scales are the nearest the mode takes.
    #[test]
    fn takes_the_nearest_scale() {
        assert_eq!(nearest_scale(&[1.0, 1.25, 1.5, 2.0], 1.4), 1.5);
        assert_eq!(nearest_scale(&[1.0, 2.0], 1.4), 1.0);
        assert_eq!(nearest_scale(&[], 1.4), 1.4);
    }
}
