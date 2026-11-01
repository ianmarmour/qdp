//! KDE Plasma's display settings, through KWin's output management protocol,
//! as System Settings changes them: each monitor's modes, its mode and
//! interface scale, and new ones applied.
//!
//! KWin 6.7 and later announce the monitors through an output device
//! registry; earlier versions as one global each.

use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

use super::kde_protocols::output_device::kde_output_device_mode_v2::{self, KdeOutputDeviceModeV2};
use super::kde_protocols::output_device::kde_output_device_registry_v2::{
    self, KdeOutputDeviceRegistryV2,
};
use super::kde_protocols::output_device::kde_output_device_v2::{self, KdeOutputDeviceV2};
use super::kde_protocols::output_management::kde_output_configuration_v2::{
    self, KdeOutputConfigurationV2,
};
use super::kde_protocols::output_management::kde_output_management_v2::KdeOutputManagementV2;
use super::monitor_matching::MonitorState;
use crate::monitor_modes::{ModeRange, MonitorMode, MonitorSettings};
use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, event_created_child};

/// The versions of the monitors announced as globals: 2 names each monitor.
const DEVICE_VERSIONS: RangeInclusive<u32> = 2..=26;

/// The versions of the registry that announces the monitors instead, from
/// KWin 6.7.
const REGISTRY_VERSIONS: RangeInclusive<u32> = 21..=26;

/// The output management versions used.
const MANAGEMENT_VERSIONS: RangeInclusive<u32> = 1..=22;

/// How long KWin may take to apply settings.
const APPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// How long to wait between asking KWin whether it applied them.
const APPLY_POLL: Duration = Duration::from_millis(10);

/// What KWin has said about its monitors and the settings applied.
#[derive(Debug, Default)]
struct Outputs {
    /// The monitors, in the order KWin announced them.
    devices: Vec<Device>,
    /// Whether the settings applied were taken, once KWin said.
    applied: Option<Result<(), String>>,
    /// Why KWin refused the settings, if it said before refusing.
    failure_reason: Option<String>,
}

/// One monitor, as KWin describes it.
#[derive(Debug)]
struct Device {
    /// The monitor's object.
    proxy: KdeOutputDeviceV2,
    /// Its name, such as its connector.
    name: String,
    /// The modes it offers.
    modes: Vec<Mode>,
    /// Its current mode, once told.
    current: Option<ObjectId>,
    /// Its interface scale.
    scale: f64,
    /// Whether KWin removed it.
    removed: bool,
}

impl Device {
    /// Returns a monitor KWin has yet to describe.
    fn new(proxy: KdeOutputDeviceV2) -> Self {
        Self {
            proxy,
            name: String::new(),
            modes: Vec::new(),
            current: None,
            scale: 1.0,
            removed: false,
        }
    }
}

/// One mode of a monitor, as KWin describes it.
#[derive(Debug)]
struct Mode {
    /// The mode's object.
    proxy: KdeOutputDeviceModeV2,
    /// Its size in pixels, once told.
    size: Option<(i32, i32)>,
    /// Its refresh rate in millihertz, once told.
    refresh: Option<i32>,
    /// Whether the monitor no longer offers it.
    removed: bool,
}

impl Mode {
    /// Returns the mode, if KWin described it and still offers it.
    fn mode(&self) -> Option<MonitorMode> {
        let (width, height) = self.size?;
        (!self.removed).then_some(())?;
        Some(MonitorMode {
            width: u32::try_from(width).ok()?,
            height: u32::try_from(height).ok()?,
            refresh_millihertz: u32::try_from(self.refresh?).ok()?,
        })
    }
}

impl Outputs {
    /// Returns the monitor `id`.
    fn device_mut(&mut self, id: &ObjectId) -> Option<&mut Device> {
        self.devices
            .iter_mut()
            .find(|device| device.proxy.id() == *id)
    }

    /// Returns the mode `id` of any monitor.
    fn mode_mut(&mut self, id: &ObjectId) -> Option<&mut Mode> {
        self.devices
            .iter_mut()
            .flat_map(|device| device.modes.iter_mut())
            .find(|mode| mode.proxy.id() == *id)
    }

    /// Returns the monitor named `name`.
    fn device(&self, name: &str) -> Result<&Device, String> {
        self.devices
            .iter()
            .find(|device| device.name == name && !device.removed)
            .ok_or_else(|| {
                let known: Vec<&str> = self
                    .devices
                    .iter()
                    .map(|device| device.name.as_str())
                    .collect();
                format!("KWin has no monitor named {name}; it has {known:?}")
            })
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

impl Dispatch<KdeOutputManagementV2, ()> for Outputs {
    fn event(
        _state: &mut Self,
        _manager: &KdeOutputManagementV2,
        _event: <KdeOutputManagementV2 as Proxy>::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<KdeOutputDeviceRegistryV2, ()> for Outputs {
    fn event(
        state: &mut Self,
        _registry: &KdeOutputDeviceRegistryV2,
        event: kde_output_device_registry_v2::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        if let kde_output_device_registry_v2::Event::Output { output } = event {
            state.devices.push(Device::new(output));
        }
    }

    event_created_child!(Outputs, KdeOutputDeviceRegistryV2, [
        kde_output_device_registry_v2::EVT_OUTPUT_OPCODE => (KdeOutputDeviceV2, ()),
    ]);
}

impl Dispatch<KdeOutputDeviceV2, ()> for Outputs {
    fn event(
        state: &mut Self,
        device: &KdeOutputDeviceV2,
        event: kde_output_device_v2::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        let Some(device) = state.device_mut(&device.id()) else {
            return;
        };
        match event {
            kde_output_device_v2::Event::Mode { mode } => device.modes.push(Mode {
                proxy: mode,
                size: None,
                refresh: None,
                removed: false,
            }),
            kde_output_device_v2::Event::CurrentMode { mode } => device.current = Some(mode.id()),
            kde_output_device_v2::Event::Scale { factor } => device.scale = factor,
            kde_output_device_v2::Event::Name { name } => device.name = name,
            kde_output_device_v2::Event::Removed => device.removed = true,
            _ => {}
        }
    }

    event_created_child!(Outputs, KdeOutputDeviceV2, [
        kde_output_device_v2::EVT_MODE_OPCODE => (KdeOutputDeviceModeV2, ()),
    ]);
}

impl Dispatch<KdeOutputDeviceModeV2, ()> for Outputs {
    fn event(
        state: &mut Self,
        mode: &KdeOutputDeviceModeV2,
        event: kde_output_device_mode_v2::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        let Some(mode) = state.mode_mut(&mode.id()) else {
            return;
        };
        match event {
            kde_output_device_mode_v2::Event::Size { width, height } => {
                mode.size = Some((width, height));
            }
            kde_output_device_mode_v2::Event::Refresh { refresh } => mode.refresh = Some(refresh),
            kde_output_device_mode_v2::Event::Removed => mode.removed = true,
            _ => {}
        }
    }
}

impl Dispatch<KdeOutputConfigurationV2, ()> for Outputs {
    fn event(
        state: &mut Self,
        _configuration: &KdeOutputConfigurationV2,
        event: kde_output_configuration_v2::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        match event {
            kde_output_configuration_v2::Event::Applied => state.applied = Some(Ok(())),
            kde_output_configuration_v2::Event::Failed => {
                let reason = state
                    .failure_reason
                    .take()
                    .unwrap_or_else(|| "KWin refused the display settings".to_owned());
                state.applied = Some(Err(reason));
            }
            kde_output_configuration_v2::Event::FailureReason { reason } => {
                state.failure_reason = Some(reason);
            }
        }
    }
}

/// A connection to KWin's display settings, with its monitors described.
pub(super) struct KdeDisplays {
    /// The connection.
    connection: Connection,
    /// The queue KWin's events arrive on.
    queue: EventQueue<Outputs>,
    /// The interface that applies settings.
    manager: KdeOutputManagementV2,
    /// What KWin has said.
    outputs: Outputs,
}

impl KdeDisplays {
    /// Connects to the compositor and reads its monitors, or returns `None`
    /// if it is not KWin.
    pub(super) fn open() -> Result<Option<Self>, String> {
        let connection = Connection::connect_to_env()
            .map_err(|error| format!("cannot reach the Wayland compositor: {error}"))?;
        let (globals, mut queue) = registry_queue_init::<Outputs>(&connection)
            .map_err(|error| format!("cannot list the compositor's interfaces: {error}"))?;
        let handle = queue.handle();
        let Ok(manager) =
            globals.bind::<KdeOutputManagementV2, _, _>(&handle, MANAGEMENT_VERSIONS, ())
        else {
            return Ok(None);
        };
        let mut outputs = Outputs::default();
        // KWin 6.7 and later announce the monitors through the registry;
        // earlier versions as one global each.
        if globals
            .bind::<KdeOutputDeviceRegistryV2, _, _>(&handle, REGISTRY_VERSIONS, ())
            .is_err()
        {
            let device_interface = KdeOutputDeviceV2::interface().name;
            globals.contents().with_list(|list| {
                for global in list.iter().filter(|global| {
                    global.interface == device_interface
                        && global.version >= *DEVICE_VERSIONS.start()
                }) {
                    let proxy = globals.registry().bind::<KdeOutputDeviceV2, _, _>(
                        global.name,
                        global.version.min(*DEVICE_VERSIONS.end()),
                        &handle,
                        (),
                    );
                    outputs.devices.push(Device::new(proxy));
                }
            });
        }
        // The round trips announce the monitors, describe them, and describe
        // the modes they announced.
        for _ in 0..3 {
            queue
                .roundtrip(&mut outputs)
                .map_err(|error| format!("lost the connection to KWin: {error}"))?;
        }
        Ok(Some(Self {
            connection,
            queue,
            manager,
            outputs,
        }))
    }

    /// Returns the modes and settings of the monitor named `name`.
    pub(super) fn monitor(&self, name: &str) -> Result<MonitorState, String> {
        let device = self.outputs.device(name)?;
        let current = device
            .modes
            .iter()
            .find(|mode| Some(mode.proxy.id()) == device.current)
            .and_then(Mode::mode)
            .ok_or_else(|| format!("KWin did not say {name}'s mode"))?;
        Ok(MonitorState {
            modes: device.modes.iter().filter_map(Mode::mode).collect(),
            range: ModeRange::Listed,
            current: MonitorSettings {
                mode: current,
                scale: device.scale,
            },
        })
    }

    /// Gives the monitor named `name` `settings`, which must be of a mode it
    /// offers.
    pub(super) fn apply(&mut self, name: &str, settings: &MonitorSettings) -> Result<(), String> {
        let device = self.outputs.device(name)?;
        let mode = device
            .modes
            .iter()
            .find(|mode| mode.mode() == Some(settings.mode))
            .ok_or_else(|| format!("{name} does not offer the mode {:?}", settings.mode))?;
        let configuration = self.manager.create_configuration(&self.queue.handle(), ());
        configuration.mode(&device.proxy, &mode.proxy);
        configuration.scale(&device.proxy, settings.scale);
        configuration.apply();
        self.outputs.applied = None;
        self.outputs.failure_reason = None;
        let deadline = Instant::now() + APPLY_TIMEOUT;
        let applied = loop {
            if let Some(applied) = self.outputs.applied.take() {
                break applied;
            }
            if Instant::now() >= deadline {
                break Err("KWin did not apply the display settings in time".to_owned());
            }
            std::thread::sleep(APPLY_POLL);
            self.queue
                .roundtrip(&mut self.outputs)
                .map_err(|error| format!("lost the connection to KWin: {error}"))?;
        };
        configuration.destroy();
        let _ = self.connection.flush();
        applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against a running KDE Plasma session: each monitor's settings are
    /// read, a new scale is applied and read back, and the old one put back.
    #[test]
    #[ignore = "needs a KDE Plasma session; run with --ignored under one"]
    fn changes_kwin_display_settings() {
        let mut displays = KdeDisplays::open()
            .unwrap()
            .expect("the compositor is KWin");
        // KWin's names are the ones xdg-output gives, which the capture
        // finds its monitor by.
        let names: Vec<String> = super::super::desktop_layout::outputs()
            .unwrap()
            .into_iter()
            .map(|output| output.name)
            .collect();
        assert!(!names.is_empty());
        for name in &names {
            displays.monitor(name).unwrap();
        }
        let name = names[0].clone();
        let state = displays.monitor(&name).unwrap();
        assert!(state.modes.contains(&state.current.mode));
        let scaled = MonitorSettings {
            scale: if state.current.scale == 1.0 { 2.0 } else { 1.0 },
            ..state.current
        };
        displays.apply(&name, &scaled).unwrap();
        let reopened = KdeDisplays::open().unwrap().unwrap();
        assert!(reopened.monitor(&name).unwrap().current.same_as(&scaled));
        displays.apply(&name, &state.current).unwrap();
        let reopened = KdeDisplays::open().unwrap().unwrap();
        assert!(
            reopened
                .monitor(&name)
                .unwrap()
                .current
                .same_as(&state.current)
        );
    }
}
