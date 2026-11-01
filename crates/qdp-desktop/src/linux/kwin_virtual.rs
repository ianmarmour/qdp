//! A virtual monitor on KDE Plasma: KWin adds a monitor of the size the
//! client asks for to the desktop and streams it through PipeWire, so that
//! the host streams the client's display without scaling it (Section 6.3).
//!
//! KWin's screencast protocol makes the monitor, which lasts as long as its
//! stream object: a new size is a new monitor, made before the old one is
//! closed, so that the stream never stops. The monitor's frames are read as
//! the screen's are, with the pointer as metadata.

use std::time::{Duration, Instant};

use qdp_codec::Yuv420Frame;
use qdp_runtime::{
    CursorState, DesktopScale, FrameSource, MonitorPlacement, RuntimeError, WantedDisplay,
};
use tracing::{info, warn};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_plasma::screencast::v1::client::zkde_screencast_stream_unstable_v1::{
    self, ZkdeScreencastStreamUnstableV1,
};
use wayland_protocols_plasma::screencast::v1::client::zkde_screencast_unstable_v1::ZkdeScreencastUnstableV1;

use super::desktop_layout::{self, OutputLayout};
use super::screencast::ScreenCapture;

/// The name KWin gives the monitor.
const OUTPUT_NAME: &str = "QDP";

/// The description the desktop's display settings show for the monitor.
const OUTPUT_DESCRIPTION: &str = "QDP virtual monitor";

/// The first version of the screencast protocol that makes virtual monitors.
const VIRTUAL_OUTPUT_VERSION: u32 = 2;

/// The version that gives them a description.
const DESCRIPTION_VERSION: u32 = 4;

/// The pointer mode that reports the pointer as metadata, apart from the
/// frames.
const POINTER_METADATA: u32 = 4;

/// How long KWin may take to make a monitor and start its stream.
const CREATE_TIMEOUT: Duration = Duration::from_secs(5);

/// The display a virtual monitor has until the client asks for one.
const FIRST_DISPLAY: WantedDisplay = WantedDisplay {
    width: 1920,
    height: 1080,
    refresh_millihertz: 60_000,
    scale_percent: 100,
};

/// Percent in a scale of one.
const UNIT_PERCENT: f64 = 100.0;

/// What the stream of the monitor being made has said.
#[derive(Debug, Default)]
struct StreamEvents {
    /// The PipeWire node of the monitor's stream, or why KWin could not
    /// make it, once it said.
    node: Option<Result<u32, String>>,
}

impl Dispatch<WlRegistry, GlobalListContents> for StreamEvents {
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

impl Dispatch<ZkdeScreencastUnstableV1, ()> for StreamEvents {
    fn event(
        _state: &mut Self,
        _manager: &ZkdeScreencastUnstableV1,
        _event: <ZkdeScreencastUnstableV1 as Proxy>::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZkdeScreencastStreamUnstableV1, ()> for StreamEvents {
    fn event(
        state: &mut Self,
        _stream: &ZkdeScreencastStreamUnstableV1,
        event: zkde_screencast_stream_unstable_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        match event {
            zkde_screencast_stream_unstable_v1::Event::Created { node } => {
                state.node = Some(Ok(node));
            }
            zkde_screencast_stream_unstable_v1::Event::Failed { error } => {
                state.node = Some(Err(error));
            }
            zkde_screencast_stream_unstable_v1::Event::Closed => {
                state
                    .node
                    .get_or_insert(Err("KWin closed the stream".to_owned()));
            }
            _ => {}
        }
    }
}

/// One virtual monitor and the capture of its frames.
struct VirtualOutput {
    /// The stream that keeps the monitor; closing it removes the monitor.
    stream: ZkdeScreencastStreamUnstableV1,
    /// The monitor's frames.
    capture: ScreenCapture,
    /// The display it was made for.
    display: WantedDisplay,
    /// Its interface scale.
    scale: DesktopScale,
    /// Where it is in the desktop, if the compositor says.
    placement: Option<MonitorPlacement>,
}

/// A virtual monitor KWin makes at the client's display.
pub struct VirtualMonitor {
    /// The connection to KWin.
    connection: Connection,
    /// The queue the stream's events arrive on.
    queue: EventQueue<StreamEvents>,
    /// KWin's screencast manager.
    manager: ZkdeScreencastUnstableV1,
    /// The current monitor, once made.
    output: Option<VirtualOutput>,
}

impl std::fmt::Debug for VirtualMonitor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VirtualMonitor")
            .field(
                "display",
                &self.output.as_ref().map(|output| output.display),
            )
            .finish_non_exhaustive()
    }
}

impl VirtualMonitor {
    /// Connects to KWin and checks that it makes virtual monitors. The first
    /// monitor is made when the client's display is known, or at the first
    /// capture.
    pub fn open() -> Result<Self, String> {
        let connection = Connection::connect_to_env()
            .map_err(|error| format!("cannot reach the Wayland compositor: {error}"))?;
        let (globals, queue) = registry_queue_init::<StreamEvents>(&connection)
            .map_err(|error| format!("cannot list the compositor's interfaces: {error}"))?;
        let manager = globals
            .bind::<ZkdeScreencastUnstableV1, _, _>(
                &queue.handle(),
                VIRTUAL_OUTPUT_VERSION..=DESCRIPTION_VERSION,
                (),
            )
            .map_err(|error| {
                format!("the compositor does not make virtual monitors, as KWin does: {error}")
            })?;
        Ok(Self {
            connection,
            queue,
            manager,
            output: None,
        })
    }

    /// Makes a monitor for `wanted` and starts reading it.
    fn create(&mut self, wanted: &WantedDisplay) -> Result<VirtualOutput, String> {
        let scale = if wanted.scale_percent == 0 {
            1.0
        } else {
            f64::from(wanted.scale_percent) / UNIT_PERCENT
        };
        // KWin sizes monitors in the desktop's units, which the scale makes
        // larger than pixels.
        let logical = |pixels: u32| ((f64::from(pixels) / scale).round() as i32).max(1);
        let (width, height) = (logical(wanted.width), logical(wanted.height));
        // The monitor's place in the desktop is that of the one KWin adds.
        let before = desktop_layout::outputs().unwrap_or_default();
        let handle = self.queue.handle();
        let stream = if self.manager.version() >= DESCRIPTION_VERSION {
            self.manager.stream_virtual_output_with_description(
                OUTPUT_NAME.to_owned(),
                OUTPUT_DESCRIPTION.to_owned(),
                width,
                height,
                scale,
                POINTER_METADATA,
                &handle,
                (),
            )
        } else {
            self.manager.stream_virtual_output(
                OUTPUT_NAME.to_owned(),
                width,
                height,
                scale,
                POINTER_METADATA,
                &handle,
                (),
            )
        };
        let mut events = StreamEvents::default();
        let deadline = Instant::now() + CREATE_TIMEOUT;
        let node = loop {
            if let Some(node) = events.node.take() {
                break node;
            }
            if Instant::now() >= deadline {
                break Err("KWin did not start the monitor's stream in time".to_owned());
            }
            self.queue
                .roundtrip(&mut events)
                .map_err(|error| format!("lost the connection to KWin: {error}"))?;
        };
        let node = match node {
            Ok(node) => node,
            Err(error) => {
                stream.close();
                let _ = self.connection.flush();
                return Err(error);
            }
        };
        let mut capture = match ScreenCapture::from_pipewire_node(node) {
            Ok(capture) => capture,
            Err(error) => {
                stream.close();
                let _ = self.connection.flush();
                return Err(error);
            }
        };
        let desktop_size = (width, height);
        capture.set_desktop_size(desktop_size);
        let scale = capture.desktop_scale().unwrap_or_default();
        let placement = desktop_layout::outputs()
            .inspect_err(|error| warn!(%error, "the desktop's monitors cannot be listed"))
            .ok()
            .and_then(|after| {
                let added = added_output(&before, &after, desktop_size)?;
                desktop_layout::placement(added.rect, &after)
            });
        if placement.is_none() {
            warn!("the virtual monitor's place in the desktop is not known");
        }
        info!(
            width = wanted.width,
            height = wanted.height,
            desktop_width = width,
            desktop_height = height,
            scale_percent = scale.percent(),
            ?placement,
            "virtual monitor made"
        );
        Ok(VirtualOutput {
            stream,
            capture,
            display: *wanted,
            scale,
            placement,
        })
    }

    /// Replaces the monitor with one for `display`, keeping the old one if
    /// the new one cannot be made.
    fn replace(&mut self, display: &WantedDisplay) -> Result<(), String> {
        let output = self.create(display)?;
        if let Some(old) = self.output.replace(output) {
            close(&self.connection, old);
        }
        Ok(())
    }

    /// Returns the current monitor, making one at the first display if there
    /// is none yet.
    fn output(&mut self) -> Result<&mut VirtualOutput, RuntimeError> {
        if self.output.is_none() {
            self.replace(&FIRST_DISPLAY)
                .map_err(RuntimeError::Capture)?;
        }
        self.output
            .as_mut()
            .ok_or_else(|| RuntimeError::Capture("no virtual monitor".to_owned()))
    }
}

/// Returns the monitor KWin added, of `size` in the desktop's units: the one
/// `after` lists that `before` did not, preferring one named or described as
/// this host's, then one of that size.
fn added_output<'a>(
    before: &[OutputLayout],
    after: &'a [OutputLayout],
    size: (i32, i32),
) -> Option<&'a OutputLayout> {
    let added: Vec<&OutputLayout> = after
        .iter()
        .filter(|output| before.iter().all(|old| old.name != output.name))
        .collect();
    let ours = |output: &&&OutputLayout| {
        output.name.contains(OUTPUT_NAME) || output.description.contains(OUTPUT_DESCRIPTION)
    };
    let sized = |output: &&&OutputLayout| {
        (i64::from(output.rect.width), i64::from(output.rect.height))
            == (i64::from(size.0), i64::from(size.1))
    };
    added
        .iter()
        .find(ours)
        .or_else(|| added.iter().find(sized))
        .or_else(|| (added.len() == 1).then(|| &added[0]))
        .copied()
}

/// Closes `output`'s stream, which removes the monitor from the desktop.
fn close(connection: &Connection, output: VirtualOutput) {
    drop(output.capture);
    output.stream.close();
    let _ = connection.flush();
}

impl Drop for VirtualMonitor {
    fn drop(&mut self) {
        if let Some(output) = self.output.take() {
            close(&self.connection, output);
        }
    }
}

impl FrameSource for VirtualMonitor {
    fn size(&self) -> (usize, usize) {
        self.output.as_ref().map_or(
            (FIRST_DISPLAY.width as usize, FIRST_DISPLAY.height as usize),
            |output| output.capture.size(),
        )
    }

    fn capture(&mut self) -> Result<std::sync::Arc<Yuv420Frame>, RuntimeError> {
        self.output()?.capture.capture()
    }

    fn cursor(&mut self) -> Option<CursorState> {
        self.output.as_mut()?.capture.cursor()
    }

    fn desktop_scale(&self) -> Option<DesktopScale> {
        self.output.as_ref().map(|output| output.scale)
    }

    fn monitor_placement(&self) -> Option<MonitorPlacement> {
        self.output.as_ref()?.placement
    }

    fn match_display(&mut self, display: &WantedDisplay) -> bool {
        if self
            .output
            .as_ref()
            .is_some_and(|output| output.display == *display)
        {
            return true;
        }
        match self.replace(display) {
            Ok(()) => true,
            Err(error) => {
                warn!(%error, "the virtual monitor cannot take the client's display");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp_runtime::LayoutRect;

    /// Returns a monitor named `name` at `x`, `width` wide.
    fn output(name: &str, x: i32, width: u32) -> OutputLayout {
        OutputLayout {
            name: name.to_owned(),
            description: String::new(),
            rect: LayoutRect {
                x,
                y: 0,
                width,
                height: 1080,
            },
        }
    }

    /// The monitor KWin added is the new one, ours by name if several are
    /// new.
    #[test]
    fn finds_the_added_monitor() {
        let before = [output("DP-1", 0, 3413)];
        let after = [output("DP-1", 0, 3413), output("Virtual-QDP", 3413, 1920)];
        let added = added_output(&before, &after, (1920, 1080)).unwrap();
        assert_eq!(added.name, "Virtual-QDP");
        // Named otherwise, the one of our size.
        let after = [
            output("DP-1", 0, 3413),
            output("HDMI-A-1", 3413, 1280),
            output("Virtual-1", 4693, 1920),
        ];
        let added = added_output(&before, &after, (1920, 1080)).unwrap();
        assert_eq!(added.name, "Virtual-1");
        // Nothing new, nothing found.
        assert!(added_output(&before, &before, (1920, 1080)).is_none());
    }
}
