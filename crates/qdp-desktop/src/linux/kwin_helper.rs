//! `qdp-kwin-monitor`, the helper program that makes KWin's virtual monitors
//! for the host.
//!
//! KWin offers the interface that makes virtual monitors only to programs a
//! desktop entry lists it for, and checks each connection a listed program
//! makes by hashing its executable on the compositor's thread. The host is
//! large and connects often, as its clipboard does; the helper is small and
//! connects once. So the host lists the helper rather than itself, runs it
//! with pipes, and asks it for monitors.
//!
//! The helper reads requests on its input and writes replies on its output,
//! one per line. Its first reply says whether KWin offers it the interface.
//! Its monitors last as long as it does, which is until its input closes,
//! so they go when the host does, however the host stops.

use std::collections::HashMap;
use std::fmt;
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::str::FromStr;
use std::time::{Duration, Instant};

use tracing::{debug, warn};
use wayland_client::globals::{BindError, GlobalList, GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_plasma::screencast::v1::client::zkde_screencast_stream_unstable_v1::{
    self, ZkdeScreencastStreamUnstableV1,
};
use wayland_protocols_plasma::screencast::v1::client::zkde_screencast_unstable_v1::ZkdeScreencastUnstableV1;

/// The helper program's file name, beside the host's.
pub(super) const PROGRAM: &str = "qdp-kwin-monitor";

/// The name KWin gives the monitors.
pub(super) const OUTPUT_NAME: &str = "QDP";

/// The description the desktop's display settings show for the monitors.
pub(super) const OUTPUT_DESCRIPTION: &str = "QDP virtual monitor";

/// The first version of the screencast protocol that makes virtual monitors.
const VIRTUAL_OUTPUT_VERSION: u32 = 2;

/// The version that gives them a description.
const DESCRIPTION_VERSION: u32 = 4;

/// The pointer mode that reports the pointer as metadata, apart from the
/// frames.
const POINTER_METADATA: u32 = 4;

/// How long KWin may take to make a monitor and start its stream.
pub(super) const CREATE_TIMEOUT: Duration = Duration::from_secs(5);

/// Interfaces only KWin offers, which tell it from other compositors when
/// it keeps the virtual monitor interface back: Plasma 6's and Plasma 5's
/// monitor descriptions.
const KWIN_INTERFACES: [&str; 2] = ["kde_output_device_v2", "org_kde_kwin_outputdevice"];

/// The words that begin each request and reply.
mod words {
    /// [`super::Request::Create`].
    pub(super) const CREATE: &str = "create";
    /// [`super::Request::Close`].
    pub(super) const CLOSE: &str = "close";
    /// [`super::Reply::Ready`].
    pub(super) const READY: &str = "ready";
    /// [`super::Reply::Withheld`].
    pub(super) const WITHHELD: &str = "withheld";
    /// [`super::Reply::Unavailable`].
    pub(super) const UNAVAILABLE: &str = "unavailable";
    /// [`super::Reply::Created`].
    pub(super) const CREATED: &str = "created";
    /// [`super::Reply::Failed`].
    pub(super) const FAILED: &str = "failed";
}

/// Returns the name of the interface KWin keeps back, which a desktop entry
/// must list for the helper.
pub(super) fn interface_name() -> &'static str {
    ZkdeScreencastUnstableV1::interface().name
}

/// Names one monitor the helper makes, chosen by the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct OutputId(pub(super) u32);

impl fmt::Display for OutputId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for OutputId {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.parse()
            .map(Self)
            .map_err(|error| format!("bad monitor number {text:?}: {error}"))
    }
}

/// What the host asks of the helper.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Request {
    /// Make a monitor of `size`, in the desktop's units, at interface
    /// `scale`.
    Create {
        /// The monitor's number.
        id: OutputId,
        /// Its width and height in the desktop's units.
        size: (i32, i32),
        /// Its interface scale, 1 for none.
        scale: f64,
    },
    /// Remove a monitor.
    Close {
        /// The monitor's number.
        id: OutputId,
    },
}

impl fmt::Display for Request {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Create {
                id,
                size: (width, height),
                scale,
            } => write!(formatter, "{} {id} {width} {height} {scale}", words::CREATE),
            Self::Close { id } => write!(formatter, "{} {id}", words::CLOSE),
        }
    }
}

impl FromStr for Request {
    type Err = String;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let number = |text: &str| {
            text.parse::<i32>()
                .map_err(|error| format!("bad number {text:?}: {error}"))
        };
        match fields.as_slice() {
            [words::CREATE, id, width, height, scale] => Ok(Self::Create {
                id: id.parse()?,
                size: (number(width)?, number(height)?),
                scale: scale
                    .parse()
                    .map_err(|error| format!("bad scale {scale:?}: {error}"))?,
            }),
            [words::CLOSE, id] => Ok(Self::Close { id: id.parse()? }),
            _ => Err(format!("unknown request {line:?}")),
        }
    }
}

/// What the helper tells the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Reply {
    /// KWin offers the helper the interface; it takes requests.
    Ready,
    /// KWin keeps the interface from the helper, as no desktop entry lists
    /// it for the helper.
    Withheld,
    /// The helper cannot make monitors, for this reason.
    Unavailable(String),
    /// A monitor was made, streamed through a PipeWire node.
    Created {
        /// The monitor's number.
        id: OutputId,
        /// The PipeWire node streaming it.
        node: u32,
    },
    /// A monitor could not be made.
    Failed {
        /// The monitor's number.
        id: OutputId,
        /// Why.
        error: String,
    },
}

impl fmt::Display for Reply {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => formatter.write_str(words::READY),
            Self::Withheld => formatter.write_str(words::WITHHELD),
            Self::Unavailable(reason) => {
                write!(formatter, "{} {}", words::UNAVAILABLE, one_line(reason))
            }
            Self::Created { id, node } => write!(formatter, "{} {id} {node}", words::CREATED),
            Self::Failed { id, error } => {
                write!(formatter, "{} {id} {}", words::FAILED, one_line(error))
            }
        }
    }
}

impl FromStr for Reply {
    type Err = String;

    fn from_str(line: &str) -> Result<Self, Self::Err> {
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        match word {
            words::READY => Ok(Self::Ready),
            words::WITHHELD => Ok(Self::Withheld),
            words::UNAVAILABLE => Ok(Self::Unavailable(rest.to_owned())),
            words::CREATED => {
                let (id, node) = rest
                    .split_once(' ')
                    .ok_or_else(|| format!("bad reply {line:?}"))?;
                Ok(Self::Created {
                    id: id.parse()?,
                    node: node
                        .parse()
                        .map_err(|error| format!("bad node {node:?}: {error}"))?,
                })
            }
            words::FAILED => {
                let (id, error) = rest.split_once(' ').unwrap_or((rest, ""));
                Ok(Self::Failed {
                    id: id.parse()?,
                    error: error.to_owned(),
                })
            }
            _ => Err(format!("unknown reply {line:?}")),
        }
    }
}

/// Returns `text` on one line, as a reply's last field must be.
fn one_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

/// Runs the helper program: answers the host's requests until its input
/// closes, then exits, which removes its monitors.
pub fn run() -> ExitCode {
    let mut kwin = match connect() {
        Ok(Connected::Ready(kwin)) => kwin,
        Ok(Connected::Withheld) => return finish(&Reply::Withheld),
        Ok(Connected::Unsupported(error)) => {
            return finish(&Reply::Unavailable(format!(
                "the compositor does not make virtual monitors, as KWin does: {error}"
            )));
        }
        Err(error) => return finish(&Reply::Unavailable(error)),
    };
    if !send(&Reply::Ready) {
        return ExitCode::FAILURE;
    }
    let mut streams = HashMap::new();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else {
            break;
        };
        match line.parse::<Request>() {
            Ok(Request::Create { id, size, scale }) => {
                let reply = match kwin.create(id, size, scale) {
                    Ok((stream, node)) => {
                        streams.insert(id, stream);
                        Reply::Created { id, node }
                    }
                    Err(error) => Reply::Failed { id, error },
                };
                if !send(&reply) {
                    break;
                }
            }
            Ok(Request::Close { id }) => {
                if let Some(stream) = streams.remove(&id) {
                    stream.close();
                    let _ = kwin.connection.flush();
                }
            }
            Err(error) => warn!(%error, "the host asked something unexpected"),
        }
    }
    ExitCode::SUCCESS
}

/// Sends `reply`, the helper's last, and returns the exit code for it.
fn finish(reply: &Reply) -> ExitCode {
    send(reply);
    ExitCode::FAILURE
}

/// Sends `reply` to the host, and returns whether the host is still there.
fn send(reply: &Reply) -> bool {
    let mut output = std::io::stdout().lock();
    writeln!(output, "{reply}").and_then(|()| output.flush()).is_ok()
}

/// What the stream of the monitor being made has said.
#[derive(Debug)]
struct StreamEvents {
    /// The monitor being made.
    awaited: OutputId,
    /// The PipeWire node of its stream, or why KWin could not make it, once
    /// it said.
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

impl Dispatch<ZkdeScreencastStreamUnstableV1, OutputId> for StreamEvents {
    fn event(
        state: &mut Self,
        _stream: &ZkdeScreencastStreamUnstableV1,
        event: zkde_screencast_stream_unstable_v1::Event,
        id: &OutputId,
        _connection: &Connection,
        _queue: &QueueHandle<Self>,
    ) {
        if *id != state.awaited {
            debug!(%id, ?event, "an earlier monitor's stream changed");
            return;
        }
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

/// A connection to KWin, with the interface that makes virtual monitors.
struct Kwin {
    /// The connection.
    connection: Connection,
    /// Its event queue.
    queue: EventQueue<StreamEvents>,
    /// The interface.
    manager: ZkdeScreencastUnstableV1,
}

impl Kwin {
    /// Makes monitor `id` of `size`, in the desktop's units, at `scale`, and
    /// returns the stream that keeps it and the PipeWire node streaming it.
    fn create(
        &mut self,
        id: OutputId,
        (width, height): (i32, i32),
        scale: f64,
    ) -> Result<(ZkdeScreencastStreamUnstableV1, u32), String> {
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
                id,
            )
        } else {
            self.manager.stream_virtual_output(
                OUTPUT_NAME.to_owned(),
                width,
                height,
                scale,
                POINTER_METADATA,
                &handle,
                id,
            )
        };
        let mut events = StreamEvents {
            awaited: id,
            node: None,
        };
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
        match node {
            Ok(node) => Ok((stream, node)),
            Err(error) => {
                stream.close();
                let _ = self.connection.flush();
                Err(error)
            }
        }
    }
}

/// A connection to the compositor, and whether it offers the helper the
/// interface that makes virtual monitors.
enum Connected {
    /// It does.
    Ready(Kwin),
    /// The compositor is KWin, which keeps the interface from programs no
    /// desktop entry lists it for.
    Withheld,
    /// The compositor does not offer it, for this reason.
    Unsupported(BindError),
}

/// Connects to the compositor and asks for the interface that makes virtual
/// monitors.
fn connect() -> Result<Connected, String> {
    let connection = Connection::connect_to_env()
        .map_err(|error| format!("cannot reach the Wayland compositor: {error}"))?;
    let (globals, queue) = registry_queue_init::<StreamEvents>(&connection)
        .map_err(|error| format!("cannot list the compositor's interfaces: {error}"))?;
    let bound = globals.bind::<ZkdeScreencastUnstableV1, _, _>(
        &queue.handle(),
        VIRTUAL_OUTPUT_VERSION..=DESCRIPTION_VERSION,
        (),
    );
    Ok(match bound {
        Ok(manager) => Connected::Ready(Kwin {
            connection,
            queue,
            manager,
        }),
        Err(BindError::NotPresent) if is_kwin(&globals) => Connected::Withheld,
        Err(error) => Connected::Unsupported(error),
    })
}

/// Returns whether the compositor listing `globals` is KWin.
fn is_kwin(globals: &GlobalList) -> bool {
    globals.contents().with_list(|list| {
        list.iter()
            .any(|global| KWIN_INTERFACES.contains(&global.interface.as_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Requests and replies read back as they were written, messages with
    /// spaces included and line breaks taken out.
    #[test]
    fn reads_back_requests_and_replies() {
        let requests = [
            Request::Create {
                id: OutputId(3),
                size: (1280, 720),
                scale: 1.5,
            },
            Request::Close { id: OutputId(3) },
        ];
        for request in requests {
            assert_eq!(request.to_string().parse::<Request>(), Ok(request));
        }
        let replies = [
            Reply::Ready,
            Reply::Withheld,
            Reply::Unavailable("cannot reach the Wayland compositor".to_owned()),
            Reply::Created {
                id: OutputId(7),
                node: 81,
            },
            Reply::Failed {
                id: OutputId(7),
                error: "Could not find output".to_owned(),
            },
        ];
        for reply in replies {
            assert_eq!(reply.to_string().parse::<Reply>(), Ok(reply));
        }
        let broken = Reply::Failed {
            id: OutputId(1),
            error: "two\nlines".to_owned(),
        };
        assert_eq!(broken.to_string(), "failed 1 two lines");
        assert!("create 1 2".parse::<Request>().is_err());
        assert!("hello".parse::<Reply>().is_err());
    }
}
