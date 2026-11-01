//! The desktop's monitors as the Wayland compositor lays them out, read
//! through xdg-output: where each one is, in the desktop's own units. The
//! absolute pointer covers them all together, so a position on the captured
//! monitor must be placed within the whole desktop.

use std::ops::RangeInclusive;

use qdp_runtime::{LayoutRect, MonitorPlacement};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{self, ZxdgOutputV1};

/// The xdg-output versions used: 2 names and describes each monitor.
const XDG_OUTPUT_VERSIONS: RangeInclusive<u32> = 2..=3;

/// The newest `wl_output` version bound; its events are not used.
const MAX_OUTPUT_VERSION: u32 = 3;

/// One monitor of the desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OutputLayout {
    /// The compositor's name for it, such as its connector.
    pub(super) name: String,
    /// Its description, as display settings show it.
    pub(super) description: String,
    /// Where it is in the desktop.
    pub(super) rect: LayoutRect,
}

/// A monitor as its xdg-output events describe it so far.
#[derive(Debug, Default)]
struct PartialOutput {
    /// Its name, once told.
    name: String,
    /// Its description, once told.
    description: String,
    /// Its top-left corner, once told.
    position: Option<(i32, i32)>,
    /// Its size, once told.
    size: Option<(i32, i32)>,
}

/// The monitors being described.
#[derive(Debug, Default)]
struct Outputs {
    /// The monitors, in the order their xdg-outputs were asked for.
    found: Vec<PartialOutput>,
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

impl Dispatch<WlOutput, ()> for Outputs {
    fn event(
        _state: &mut Self,
        _output: &WlOutput,
        _event: <WlOutput as Proxy>::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgOutputManagerV1, ()> for Outputs {
    fn event(
        _state: &mut Self,
        _manager: &ZxdgOutputManagerV1,
        _event: <ZxdgOutputManagerV1 as Proxy>::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgOutputV1, usize> for Outputs {
    fn event(
        state: &mut Self,
        _output: &ZxdgOutputV1,
        event: zxdg_output_v1::Event,
        index: &usize,
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        let Some(output) = state.found.get_mut(*index) else {
            return;
        };
        match event {
            zxdg_output_v1::Event::LogicalPosition { x, y } => output.position = Some((x, y)),
            zxdg_output_v1::Event::LogicalSize { width, height } => {
                output.size = Some((width, height));
            }
            zxdg_output_v1::Event::Name { name } => output.name = name,
            zxdg_output_v1::Event::Description { description } => {
                output.description = description;
            }
            _ => {}
        }
    }
}

/// Returns the desktop's monitors, as the compositor lays them out.
pub(super) fn outputs() -> Result<Vec<OutputLayout>, String> {
    let connection = Connection::connect_to_env()
        .map_err(|error| format!("cannot reach the Wayland compositor: {error}"))?;
    let (globals, mut queue) = registry_queue_init::<Outputs>(&connection)
        .map_err(|error| format!("cannot list the compositor's interfaces: {error}"))?;
    let handle = queue.handle();
    let manager = globals
        .bind::<ZxdgOutputManagerV1, _, _>(&handle, XDG_OUTPUT_VERSIONS, ())
        .map_err(|error| format!("the compositor does not describe its monitors: {error}"))?;
    let monitors: Vec<WlOutput> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|global| global.interface == WlOutput::interface().name)
            .map(|global| {
                globals.registry().bind::<WlOutput, _, _>(
                    global.name,
                    global.version.min(MAX_OUTPUT_VERSION),
                    &handle,
                    (),
                )
            })
            .collect()
    });
    let mut state = Outputs::default();
    for (index, monitor) in monitors.iter().enumerate() {
        state.found.push(PartialOutput::default());
        manager.get_xdg_output(monitor, &handle, index);
    }
    queue
        .roundtrip(&mut state)
        .map_err(|error| format!("lost the connection to the compositor: {error}"))?;
    Ok(state
        .found
        .into_iter()
        .filter_map(|output| {
            let (x, y) = output.position?;
            let (width, height) = output.size?;
            Some(OutputLayout {
                name: output.name,
                description: output.description,
                rect: LayoutRect {
                    x,
                    y,
                    width: u32::try_from(width).ok()?,
                    height: u32::try_from(height).ok()?,
                },
            })
        })
        .collect())
}

/// Returns where `monitor` is in the desktop that `outputs` and it make up.
pub(super) fn placement(monitor: LayoutRect, outputs: &[OutputLayout]) -> Option<MonitorPlacement> {
    let desktop = LayoutRect::union(outputs.iter().map(|output| output.rect).chain([monitor]))?;
    Some(MonitorPlacement { monitor, desktop })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The desktop is every monitor together, the captured one included.
    #[test]
    fn places_a_monitor_among_others() {
        let rect = |x: i32, width: u32| LayoutRect {
            x,
            y: 0,
            width,
            height: 1080,
        };
        let outputs = [
            OutputLayout {
                name: "DP-1".to_owned(),
                description: String::new(),
                rect: rect(0, 3413),
            },
            OutputLayout {
                name: "Virtual-QDP".to_owned(),
                description: String::new(),
                rect: rect(3413, 1920),
            },
        ];
        let placement = placement(rect(3413, 1920), &outputs).unwrap();
        assert_eq!(placement.desktop, rect(0, 5333));
        assert_eq!(placement.monitor, rect(3413, 1920));
    }
}
