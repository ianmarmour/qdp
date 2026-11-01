//! The display settings of wlroots compositors and others that share their
//! output management protocol, such as sway, Hyprland, river, Wayfire, niri
//! and COSMIC: each monitor's modes, its mode and interface scale, and new
//! ones applied, as their display tools change them.
//!
//! A configuration covers every monitor: the ones not changed are carried
//! over as they are.

use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1;
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_configuration_v1::{
    self, ZwlrOutputConfigurationV1,
};
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_head_v1::{
    self, ZwlrOutputHeadV1,
};
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_manager_v1::{
    self, ZwlrOutputManagerV1,
};
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_mode_v1::{
    self, ZwlrOutputModeV1,
};

use super::monitor_matching::MonitorState;
use crate::monitor_modes::{ModeRange, MonitorMode, MonitorSettings};

/// The output management versions used.
const MANAGER_VERSIONS: RangeInclusive<u32> = 1..=4;

/// How long the compositor may take to apply settings.
const APPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait between asking the compositor whether it applied them.
const APPLY_POLL: Duration = Duration::from_millis(10);

/// How many times settings are applied again when the monitors changed
/// meanwhile.
const APPLY_ATTEMPTS: u32 = 3;

/// What the compositor has said about its monitors and the settings
/// applied.
#[derive(Debug, Default)]
struct Outputs {
    /// The monitors, in the order the compositor announced them.
    heads: Vec<Head>,
    /// The serial of the monitors' latest description, which a
    /// configuration names.
    serial: Option<u32>,
    /// What became of the settings applied, once the compositor said.
    applied: Option<Applied>,
}

/// What became of settings applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Applied {
    /// They were taken.
    Succeeded,
    /// They were refused.
    Failed,
    /// The monitors changed before they could be, so they must be made
    /// again.
    Cancelled,
}

/// One monitor, as the compositor describes it.
#[derive(Debug)]
struct Head {
    /// The monitor's object.
    proxy: ZwlrOutputHeadV1,
    /// Its name, such as its connector.
    name: String,
    /// Whether it is on.
    enabled: bool,
    /// The modes it offers.
    modes: Vec<Mode>,
    /// Its current mode, if it has one of its modes.
    current: Option<ObjectId>,
    /// Its interface scale.
    scale: f64,
    /// Whether it is gone.
    finished: bool,
}

/// One mode of a monitor, as the compositor describes it.
#[derive(Debug)]
struct Mode {
    /// The mode's object.
    proxy: ZwlrOutputModeV1,
    /// Its size in pixels, once told.
    size: Option<(i32, i32)>,
    /// Its refresh rate in millihertz, if told.
    refresh: Option<i32>,
    /// Whether the monitor no longer offers it.
    finished: bool,
}

impl Mode {
    /// Returns the mode, if the compositor described it and still offers it.
    fn mode(&self) -> Option<MonitorMode> {
        let (width, height) = self.size?;
        (!self.finished).then_some(())?;
        Some(MonitorMode {
            width: u32::try_from(width).ok()?,
            height: u32::try_from(height).ok()?,
            refresh_millihertz: u32::try_from(self.refresh.unwrap_or(0)).unwrap_or(0),
        })
    }
}

impl Outputs {
    /// Returns the monitor `id`.
    fn head_mut(&mut self, id: &ObjectId) -> Option<&mut Head> {
        self.heads.iter_mut().find(|head| head.proxy.id() == *id)
    }

    /// Returns the mode `id` of any monitor.
    fn mode_mut(&mut self, id: &ObjectId) -> Option<&mut Mode> {
        self.heads
            .iter_mut()
            .flat_map(|head| head.modes.iter_mut())
            .find(|mode| mode.proxy.id() == *id)
    }

    /// Returns the monitor named `name`.
    fn head(&self, name: &str) -> Result<&Head, String> {
        self.heads
            .iter()
            .find(|head| head.name == name && !head.finished)
            .ok_or_else(|| format!("the compositor has no monitor named {name}"))
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Outputs {
    fn event(
        _state: &mut Self,
        _registry: &WlRegistry,
        _event: <WlRegistry as Proxy>::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for Outputs {
    fn event(
        state: &mut Self,
        _manager: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => state.heads.push(Head {
                proxy: head,
                name: String::new(),
                enabled: false,
                modes: Vec::new(),
                current: None,
                scale: 1.0,
                finished: false,
            }),
            zwlr_output_manager_v1::Event::Done { serial } => state.serial = Some(serial),
            _ => {}
        }
    }

    event_created_child!(Outputs, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for Outputs {
    fn event(
        state: &mut Self,
        head: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        let Some(head) = state.head_mut(&head.id()) else {
            return;
        };
        match event {
            zwlr_output_head_v1::Event::Name { name } => head.name = name,
            zwlr_output_head_v1::Event::Enabled { enabled } => head.enabled = enabled != 0,
            zwlr_output_head_v1::Event::Mode { mode } => head.modes.push(Mode {
                proxy: mode,
                size: None,
                refresh: None,
                finished: false,
            }),
            zwlr_output_head_v1::Event::CurrentMode { mode } => head.current = Some(mode.id()),
            zwlr_output_head_v1::Event::Scale { scale } => head.scale = scale,
            zwlr_output_head_v1::Event::Finished => head.finished = true,
            _ => {}
        }
    }

    event_created_child!(Outputs, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for Outputs {
    fn event(
        state: &mut Self,
        mode: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        let Some(mode) = state.mode_mut(&mode.id()) else {
            return;
        };
        match event {
            zwlr_output_mode_v1::Event::Size { width, height } => mode.size = Some((width, height)),
            zwlr_output_mode_v1::Event::Refresh { refresh } => mode.refresh = Some(refresh),
            zwlr_output_mode_v1::Event::Finished => mode.finished = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for Outputs {
    fn event(
        state: &mut Self,
        _configuration: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        state.applied = match event {
            zwlr_output_configuration_v1::Event::Succeeded => Some(Applied::Succeeded),
            zwlr_output_configuration_v1::Event::Failed => Some(Applied::Failed),
            zwlr_output_configuration_v1::Event::Cancelled => Some(Applied::Cancelled),
            _ => state.applied,
        };
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ()> for Outputs {
    fn event(
        _state: &mut Self,
        _head: &ZwlrOutputConfigurationHeadV1,
        _event: <ZwlrOutputConfigurationHeadV1 as Proxy>::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

/// A connection to the compositor's display settings, with its monitors
/// described.
pub(super) struct WlrDisplays {
    /// The connection.
    connection: Connection,
    /// The queue the compositor's events arrive on.
    queue: EventQueue<Outputs>,
    /// The interface that describes the monitors and applies settings.
    manager: ZwlrOutputManagerV1,
    /// What the compositor has said.
    outputs: Outputs,
}

impl WlrDisplays {
    /// Connects to the compositor and reads its monitors, or returns `None`
    /// if it does not share the wlroots output management protocol.
    pub(super) fn open() -> Result<Option<Self>, String> {
        let connection = Connection::connect_to_env()
            .map_err(|error| format!("cannot reach the Wayland compositor: {error}"))?;
        let (globals, mut queue) = registry_queue_init::<Outputs>(&connection)
            .map_err(|error| format!("cannot list the compositor's interfaces: {error}"))?;
        let Ok(manager) =
            globals.bind::<ZwlrOutputManagerV1, _, _>(&queue.handle(), MANAGER_VERSIONS, ())
        else {
            return Ok(None);
        };
        let mut outputs = Outputs::default();
        // The first round trip announces the monitors, the second describes
        // them and their modes.
        for _ in 0..2 {
            queue
                .roundtrip(&mut outputs)
                .map_err(|error| format!("lost the connection to the compositor: {error}"))?;
        }
        Ok(Some(Self {
            connection,
            queue,
            manager,
            outputs,
        }))
    }

    /// Returns the modes and settings of the monitor named `name`. A monitor
    /// that lists no modes, as a headless one, takes any.
    pub(super) fn monitor(&self, name: &str) -> Result<MonitorState, String> {
        let head = self.outputs.head(name)?;
        let modes: Vec<MonitorMode> = head.modes.iter().filter_map(Mode::mode).collect();
        let current = head
            .modes
            .iter()
            .find(|mode| Some(mode.proxy.id()) == head.current)
            .and_then(Mode::mode)
            .ok_or_else(|| format!("the compositor did not say {name}'s mode"))?;
        // A monitor of no more than its current mode is headless or
        // virtual, and takes any.
        let range = if modes.len() <= 1 {
            ModeRange::Any
        } else {
            ModeRange::Listed
        };
        Ok(MonitorState {
            modes,
            range,
            current: MonitorSettings {
                mode: current,
                scale: head.scale,
            },
        })
    }

    /// Gives the monitor named `name` `settings`: one of its modes, or any
    /// mode if it lists none of that size.
    pub(super) fn apply(&mut self, name: &str, settings: &MonitorSettings) -> Result<(), String> {
        for _ in 0..APPLY_ATTEMPTS {
            match self.apply_once(name, settings)? {
                Applied::Succeeded => return Ok(()),
                Applied::Failed => {
                    return Err("the compositor refused the display settings".to_owned());
                }
                // The monitors changed: their new description comes first.
                Applied::Cancelled => {
                    self.queue.roundtrip(&mut self.outputs).map_err(|error| {
                        format!("lost the connection to the compositor: {error}")
                    })?;
                }
            }
        }
        Err("the compositor's monitors kept changing while the settings were applied".to_owned())
    }

    /// Applies `settings` to the monitor named `name`, carrying the other
    /// monitors over, and returns what became of them.
    fn apply_once(&mut self, name: &str, settings: &MonitorSettings) -> Result<Applied, String> {
        let serial = self
            .outputs
            .serial
            .ok_or_else(|| "the compositor did not describe its monitors".to_owned())?;
        let target = self.outputs.head(name)?.proxy.id();
        let handle = self.queue.handle();
        let configuration = self.manager.create_configuration(serial, &handle, ());
        for head in self.outputs.heads.iter().filter(|head| !head.finished) {
            if head.proxy.id() != target {
                if head.enabled {
                    configuration.enable_head(&head.proxy, &handle, ());
                } else {
                    configuration.disable_head(&head.proxy);
                }
                continue;
            }
            let configured = configuration.enable_head(&head.proxy, &handle, ());
            let offered = head
                .modes
                .iter()
                .find(|mode| mode.mode() == Some(settings.mode));
            match offered {
                Some(mode) => configured.set_mode(&mode.proxy),
                None => configured.set_custom_mode(
                    i32::try_from(settings.mode.width).unwrap_or(i32::MAX),
                    i32::try_from(settings.mode.height).unwrap_or(i32::MAX),
                    i32::try_from(settings.mode.refresh_millihertz).unwrap_or(0),
                ),
            }
            configured.set_scale(settings.scale);
        }
        configuration.apply();
        self.outputs.applied = None;
        let deadline = Instant::now() + APPLY_TIMEOUT;
        let applied = loop {
            if let Some(applied) = self.outputs.applied.take() {
                break Ok(applied);
            }
            if Instant::now() >= deadline {
                break Err("the compositor did not apply the display settings in time".to_owned());
            }
            std::thread::sleep(APPLY_POLL);
            if let Err(error) = self.queue.roundtrip(&mut self.outputs) {
                break Err(format!("lost the connection to the compositor: {error}"));
            }
        };
        configuration.destroy();
        let _ = self.connection.flush();
        applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against a running wlroots compositor, as headless sway: a monitor's
    /// settings are read, a new size and scale are applied and read back,
    /// and the old ones put back.
    #[test]
    #[ignore = "needs a compositor with wlroots output management; run with --ignored under one"]
    fn changes_wlroots_display_settings() {
        let mut displays = WlrDisplays::open()
            .unwrap()
            .expect("the compositor shares wlroots output management");
        let name = displays.outputs.heads[0].name.clone();
        let state = displays.monitor(&name).unwrap();
        let changed = MonitorSettings {
            mode: MonitorMode {
                width: 1024,
                height: 640,
                refresh_millihertz: state.current.mode.refresh_millihertz,
            },
            scale: 2.0,
        };
        displays.apply(&name, &changed).unwrap();
        let reopened = WlrDisplays::open().unwrap().unwrap();
        let after = reopened.monitor(&name).unwrap().current;
        assert_eq!(
            (after.mode.width, after.mode.height, after.scale),
            (1024, 640, 2.0)
        );
        displays = reopened;
        displays.apply(&name, &state.current).unwrap();
        let reopened = WlrDisplays::open().unwrap().unwrap();
        assert!(
            reopened
                .monitor(&name)
                .unwrap()
                .current
                .same_as(&state.current)
        );
    }
}
