//! Sharing a GNOME desktop through Mutter's own remote desktop and screen
//! cast interfaces (`org.gnome.Mutter.RemoteDesktop` and
//! `org.gnome.Mutter.ScreenCast`) rather than the portal. Nobody is asked to
//! choose or allow anything, so a headless GNOME session, which has nobody
//! to ask, can be shared; who may share it is whoever may use the session's
//! D-Bus.
//!
//! A remote desktop session, with a screen cast linked to it, records the
//! primary monitor with the pointer as metadata. Its PipeWire stream is read
//! as any other screen cast's, and the client's input goes through the same
//! session, from a thread of its own, without waiting for replies.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use futures_lite::StreamExt;
use qdp_runtime::{InputAction, InputInjector, PointerButton, TouchPhase};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tracing::{debug, info, warn};
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, Value};

use super::keycodes::{consumer_key, keyboard_key};
use super::screencast::ScreenCapture;
use crate::{ScreenOptions, SharedDesktop};

/// The name of the thread that holds the Mutter sessions.
const MUTTER_THREAD: &str = "qdp-mutter";

/// How long Mutter may take to start the screen cast's PipeWire stream.
const STREAM_TIMEOUT: Duration = Duration::from_secs(10);

/// How long applications are given to take the session's new pointer before
/// it is moved for them to see.
const POINTER_SETTLE: Duration = Duration::from_millis(250);

/// The connector that asks Mutter for the primary monitor.
const PRIMARY_MONITOR: &str = "";

/// The screen cast property naming the remote desktop session it belongs to.
const REMOTE_DESKTOP_SESSION_ID: &str = "remote-desktop-session-id";

/// The screen cast property choosing how the pointer is sent.
const CURSOR_MODE: &str = "cursor-mode";

/// The cursor mode that sends the pointer as metadata beside the frames.
const CURSOR_METADATA: u32 = 2;

/// QDP's units in a pixel, for positions, and in a device count, for motion.
const UNITS_PER_PIXEL: f64 = 256.0;

/// QDP's scroll units in a wheel detent.
const UNITS_PER_DETENT: i32 = 120;

/// Mutter's vertical scroll axis.
const VERTICAL_AXIS: u32 = 0;

/// Mutter's horizontal scroll axis.
const HORIZONTAL_AXIS: u32 = 1;

#[zbus::proxy(
    interface = "org.gnome.Mutter.RemoteDesktop",
    default_service = "org.gnome.Mutter.RemoteDesktop",
    default_path = "/org/gnome/Mutter/RemoteDesktop",
    gen_blocking = false
)]
trait RemoteDesktop {
    /// Creates a remote desktop session.
    fn create_session(&self) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.RemoteDesktop.Session",
    default_service = "org.gnome.Mutter.RemoteDesktop",
    gen_blocking = false
)]
trait RemoteDesktopSession {
    /// The session's ID, which a screen cast session links to.
    #[zbus(property)]
    fn session_id(&self) -> zbus::Result<String>;

    /// Starts the session, and its screen casts with it.
    fn start(&self) -> zbus::Result<()>;

    /// Stops the session.
    fn stop(&self) -> zbus::Result<()>;

    /// Presses or releases a key, by its evdev code.
    #[zbus(no_reply)]
    fn notify_keyboard_keycode(&self, keycode: u32, state: bool) -> zbus::Result<()>;

    /// Presses or releases a button, by its evdev code.
    #[zbus(no_reply)]
    fn notify_pointer_button(&self, button: i32, state: bool) -> zbus::Result<()>;

    /// Scrolls by whole steps on an axis.
    #[zbus(no_reply)]
    fn notify_pointer_axis_discrete(&self, axis: u32, steps: i32) -> zbus::Result<()>;

    /// Moves the pointer by a distance.
    #[zbus(no_reply)]
    fn notify_pointer_motion_relative(&self, dx: f64, dy: f64) -> zbus::Result<()>;

    /// Moves the pointer to a position in a screen cast stream.
    #[zbus(no_reply)]
    fn notify_pointer_motion_absolute(&self, stream: &str, x: f64, y: f64) -> zbus::Result<()>;

    /// Puts a finger down at a position in a screen cast stream.
    #[zbus(no_reply)]
    fn notify_touch_down(&self, stream: &str, slot: u32, x: f64, y: f64) -> zbus::Result<()>;

    /// Moves a finger to a position in a screen cast stream.
    #[zbus(no_reply)]
    fn notify_touch_motion(&self, stream: &str, slot: u32, x: f64, y: f64) -> zbus::Result<()>;

    /// Lifts a finger.
    #[zbus(no_reply)]
    fn notify_touch_up(&self, slot: u32) -> zbus::Result<()>;

    /// Sent when Mutter ends the session.
    #[zbus(signal)]
    fn closed(&self) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast",
    default_service = "org.gnome.Mutter.ScreenCast",
    default_path = "/org/gnome/Mutter/ScreenCast",
    gen_blocking = false
)]
trait ScreenCast {
    /// Creates a screen cast session.
    fn create_session(&self, properties: HashMap<&str, Value<'_>>)
    -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast.Session",
    default_service = "org.gnome.Mutter.ScreenCast",
    gen_blocking = false
)]
trait ScreenCastSession {
    /// Records a monitor, by its connector, the primary one for an empty
    /// connector.
    fn record_monitor(
        &self,
        connector: &str,
        properties: HashMap<&str, Value<'_>>,
    ) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.gnome.Mutter.ScreenCast.Stream",
    default_service = "org.gnome.Mutter.ScreenCast",
    gen_blocking = false
)]
trait ScreenCastStream {
    /// Sent once the stream's PipeWire node exists.
    #[zbus(signal, name = "PipeWireStreamAdded")]
    fn pipe_wire_stream_added(&self, node_id: u32) -> zbus::Result<()>;
}

/// Starts sharing the GNOME desktop: its primary monitor, delivering frames
/// as `options` asks, and an injector performing the client's input on it.
/// The sharing lasts while either is kept. Blocks until the first frame
/// arrives.
pub fn open_gnome(options: ScreenOptions) -> Result<SharedDesktop, String> {
    let session = MutterSession::open()?;
    let mut capture = ScreenCapture::from_pipewire_node(session.node_id, options.frames)?;
    let injector = MutterInjector::new(session.commands.clone(), session.stream.clone());
    capture.keep_alive(Box::new(session));
    info!("sharing the GNOME desktop through Mutter's remote desktop session");
    Ok((Box::new(capture), Box::new(injector)))
}

/// Returns an injector accepting what [`open_gnome`]'s does, which performs
/// nothing, for finding out what the host accepts before a session opens.
pub fn probe_injector() -> MutterInjector {
    let (commands, _) = unbounded_channel();
    MutterInjector::new(commands, String::new())
}

/// What the Mutter thread is asked to do.
#[derive(Debug, Clone, PartialEq)]
enum Command {
    /// Presses or releases a key.
    Key {
        /// The key's evdev code.
        code: u32,
        /// Whether it is pressed.
        pressed: bool,
    },
    /// Presses or releases a button.
    Button {
        /// The button's evdev code.
        code: i32,
        /// Whether it is pressed.
        pressed: bool,
    },
    /// Moves the pointer to a position in the stream.
    MoveTo {
        /// The horizontal position, in pixels.
        x: f64,
        /// The vertical position, in pixels.
        y: f64,
    },
    /// Moves the pointer by a distance.
    MoveBy {
        /// The horizontal motion.
        dx: f64,
        /// The vertical motion.
        dy: f64,
    },
    /// Scrolls by whole steps on an axis.
    Scroll {
        /// Mutter's axis.
        axis: u32,
        /// The steps, positive down or right.
        steps: i32,
    },
    /// Puts a finger down.
    TouchDown {
        /// The finger's slot.
        slot: u32,
        /// The horizontal position, in pixels.
        x: f64,
        /// The vertical position, in pixels.
        y: f64,
    },
    /// Moves a finger.
    TouchMotion {
        /// The finger's slot.
        slot: u32,
        /// The horizontal position, in pixels.
        x: f64,
        /// The vertical position, in pixels.
        y: f64,
    },
    /// Lifts a finger.
    TouchUp {
        /// The finger's slot.
        slot: u32,
    },
}

/// A Mutter remote desktop session, with its screen cast, held by a thread
/// of its own until every sender of its commands is dropped.
#[derive(Debug)]
struct MutterSession {
    /// The screen cast stream's PipeWire node.
    node_id: u32,
    /// The screen cast stream's object path, which absolute positions name.
    stream: String,
    /// Sends the thread input to perform.
    commands: UnboundedSender<Command>,
}

impl MutterSession {
    /// Opens the sessions and starts the screen cast. Blocks until its
    /// PipeWire stream exists.
    fn open() -> Result<Self, String> {
        let (started, started_receiver) = mpsc::channel();
        let (commands, command_receiver) = unbounded_channel();
        std::thread::Builder::new()
            .name(MUTTER_THREAD.to_owned())
            .spawn(move || run_session(started, command_receiver))
            .map_err(|error| error.to_string())?;
        let (node_id, stream) = started_receiver
            .recv()
            .map_err(|_| "the Mutter thread stopped".to_owned())??;
        Ok(Self {
            node_id,
            stream,
            commands,
        })
    }
}

/// Runs the Mutter sessions: starts them, reports the stream to `started`,
/// then performs `commands` until they end or Mutter closes the session.
fn run_session(
    started: mpsc::Sender<Result<(u32, String), String>>,
    mut commands: UnboundedReceiver<Command>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = started.send(Err(format!("cannot start the Mutter thread: {error}")));
            return;
        }
    };
    runtime.block_on(async move {
        let (session, stream) = match start_sessions().await {
            Ok(sessions) => sessions,
            Err(error) => {
                let _ = started.send(Err(error));
                return;
            }
        };
        let mut closed = match session.receive_closed().await {
            Ok(closed) => closed,
            Err(error) => {
                let _ = started.send(Err(format!("cannot follow the Mutter session: {error}")));
                return;
            }
        };
        let _ = started.send(Ok((stream.node_id, stream.path.clone())));
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => perform(&session, &stream.path, command).await,
                    None => break,
                },
                _ = closed.next() => {
                    warn!("Mutter closed the remote desktop session");
                    return;
                }
            }
        }
        if let Err(error) = session.stop().await {
            debug!(%error, "stopping the Mutter session failed");
        }
    });
}

/// The screen cast's stream once it started.
struct StartedStream {
    /// Its object path.
    path: String,
    /// Its PipeWire node.
    node_id: u32,
}

/// Creates a remote desktop session and a screen cast of the primary
/// monitor linked to it, starts them, and waits for the stream's PipeWire
/// node.
async fn start_sessions() -> Result<(RemoteDesktopSessionProxy<'static>, StartedStream), String> {
    let failed = |what: &str, error: zbus::Error| format!("{what}: {error}");
    let connection = Connection::session()
        .await
        .map_err(|error| failed("no D-Bus session", error))?;
    let remote_desktop = RemoteDesktopProxy::new(&connection)
        .await
        .map_err(|error| failed("no Mutter remote desktop", error))?;
    let session_path = remote_desktop
        .create_session()
        .await
        .map_err(|error| failed("cannot create a Mutter remote desktop session", error))?;
    let session = RemoteDesktopSessionProxy::builder(&connection)
        .path(session_path)
        .map_err(|error| failed("bad session path", error))?
        .build()
        .await
        .map_err(|error| failed("cannot reach the remote desktop session", error))?;
    let session_id = session
        .session_id()
        .await
        .map_err(|error| failed("no remote desktop session ID", error))?;

    let screen_cast = ScreenCastProxy::new(&connection)
        .await
        .map_err(|error| failed("no Mutter screen cast", error))?;
    let cast_path = screen_cast
        .create_session(HashMap::from([(
            REMOTE_DESKTOP_SESSION_ID,
            Value::from(session_id.as_str()),
        )]))
        .await
        .map_err(|error| failed("cannot create a Mutter screen cast session", error))?;
    let cast = ScreenCastSessionProxy::builder(&connection)
        .path(cast_path)
        .map_err(|error| failed("bad screen cast path", error))?
        .build()
        .await
        .map_err(|error| failed("cannot reach the screen cast session", error))?;
    let stream_path = cast
        .record_monitor(
            PRIMARY_MONITOR,
            HashMap::from([(CURSOR_MODE, Value::from(CURSOR_METADATA))]),
        )
        .await
        .map_err(|error| failed("cannot record the primary monitor", error))?;
    let stream = ScreenCastStreamProxy::builder(&connection)
        .path(stream_path.clone())
        .map_err(|error| failed("bad stream path", error))?
        .build()
        .await
        .map_err(|error| failed("cannot reach the screen cast stream", error))?;
    let mut added = stream
        .receive_pipe_wire_stream_added()
        .await
        .map_err(|error| failed("cannot follow the screen cast stream", error))?;

    session
        .start()
        .await
        .map_err(|error| failed("cannot start the Mutter session", error))?;
    let signal = tokio::time::timeout(STREAM_TIMEOUT, added.next())
        .await
        .map_err(|_| "Mutter did not start the screen cast's PipeWire stream".to_owned())?
        .ok_or("the screen cast stream went away")?;
    let node_id = signal
        .args()
        .map_err(|error| failed("bad PipeWire stream signal", error))?
        .node_id;
    debug!(node_id, stream = %stream_path.as_str(), "Mutter screen cast started");
    create_pointer(&session).await;
    Ok((
        session,
        StartedStream {
            path: stream_path.as_str().to_owned(),
            node_id,
        },
    ))
}

/// Has Mutter create the session's pointer now, rather than on the client's
/// first input. Where the desktop had no pointer, as a headless one has
/// not, applications take the new one a moment after it appears: the event
/// that created it, and so the client's first move or click, never reached
/// them, and they went on unaware of the pointer above them until it moved
/// again. A pointer that has not moved since is shown as hidden, too, which
/// the host would take for a game's. So the pointer is created, left a
/// moment for applications to take, and moved by nothing, which tells the
/// one under it.
async fn create_pointer(session: &RemoteDesktopSessionProxy<'_>) {
    for _ in 0..2 {
        if let Err(error) = session.notify_pointer_motion_relative(0.0, 0.0).await {
            debug!(%error, "Mutter refused to create the pointer");
            return;
        }
        tokio::time::sleep(POINTER_SETTLE).await;
    }
}

/// Performs one command through the remote desktop session.
async fn perform(session: &RemoteDesktopSessionProxy<'_>, stream: &str, command: Command) {
    let result = match command {
        Command::Key { code, pressed } => session.notify_keyboard_keycode(code, pressed).await,
        Command::Button { code, pressed } => session.notify_pointer_button(code, pressed).await,
        Command::MoveTo { x, y } => session.notify_pointer_motion_absolute(stream, x, y).await,
        Command::MoveBy { dx, dy } => session.notify_pointer_motion_relative(dx, dy).await,
        Command::Scroll { axis, steps } => session.notify_pointer_axis_discrete(axis, steps).await,
        Command::TouchDown { slot, x, y } => session.notify_touch_down(stream, slot, x, y).await,
        Command::TouchMotion { slot, x, y } => {
            session.notify_touch_motion(stream, slot, x, y).await
        }
        Command::TouchUp { slot } => session.notify_touch_up(slot).await,
    };
    if let Err(error) = result {
        debug!(%error, "Mutter refused input");
    }
}

/// Performs the client's input through a Mutter remote desktop session.
#[derive(Debug)]
pub struct MutterInjector {
    /// Sends the session's thread input to perform.
    commands: UnboundedSender<Command>,
    /// The scrolling not yet a whole detent: vertical, then horizontal.
    scroll_remainder: (i32, i32),
    /// The stream's object path, for errors.
    stream: String,
}

impl MutterInjector {
    /// Returns an injector sending `commands` to the session streaming
    /// `stream`.
    fn new(commands: UnboundedSender<Command>, stream: String) -> Self {
        Self {
            commands,
            scroll_remainder: (0, 0),
            stream,
        }
    }

    /// Sends one command.
    fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .send(command)
            .map_err(|_| format!("the Mutter session for {} ended", self.stream))
    }

    /// Returns the commands for `action`, keeping partial scrolling for the
    /// next.
    fn commands(&mut self, action: &InputAction) -> Result<Vec<Command>, String> {
        let key = |code: Option<u16>, pressed: bool| {
            code.map(|code| Command::Key {
                code: u32::from(code),
                pressed,
            })
        };
        Ok(match action {
            InputAction::KeyDown(usage) => key(keyboard_key(*usage), true).into_iter().collect(),
            InputAction::KeyUp(usage) => key(keyboard_key(*usage), false).into_iter().collect(),
            InputAction::ConsumerKeyDown(usage) => {
                key(consumer_key(*usage), true).into_iter().collect()
            }
            InputAction::ConsumerKeyUp(usage) => {
                key(consumer_key(*usage), false).into_iter().collect()
            }
            InputAction::PointerMoveTo { x, y } => vec![Command::MoveTo {
                x: pixels(*x),
                y: pixels(*y),
            }],
            InputAction::MouseMoveBy { dx, dy } => vec![Command::MoveBy {
                dx: pixels(*dx),
                dy: pixels(*dy),
            }],
            InputAction::ButtonDown(button) => vec![Command::Button {
                code: button_code(*button),
                pressed: true,
            }],
            InputAction::ButtonUp(button) => vec![Command::Button {
                code: button_code(*button),
                pressed: false,
            }],
            InputAction::Scroll {
                vertical,
                horizontal,
            } => self.scroll(*vertical, *horizontal),
            InputAction::Touch(points) => points
                .iter()
                .map(|point| {
                    let slot = u32::from(point.id);
                    let (x, y) = (pixels(point.x), pixels(point.y));
                    match point.phase {
                        TouchPhase::Down => Command::TouchDown { slot, x, y },
                        TouchPhase::Move => Command::TouchMotion { slot, x, y },
                        TouchPhase::Up | TouchPhase::Cancel => Command::TouchUp { slot },
                    }
                })
                .collect(),
            // The host accepts none of these from this injector.
            InputAction::Text(_)
            | InputAction::Gamepad { .. }
            | InputAction::GamepadRemoved(_)
            | InputAction::Pen(_)
            | InputAction::PassthroughReport { .. }
            | InputAction::PassthroughRemoved(_)
            | InputAction::Touchpad { .. }
            | InputAction::TouchpadRemoved(_) => {
                return Err("Mutter's remote desktop cannot perform this input".to_owned());
            }
        })
    }

    /// Returns whole detents of scrolling, Mutter's positive steps scrolling
    /// down and right, and keeps the rest for the next scroll.
    fn scroll(&mut self, vertical: i32, horizontal: i32) -> Vec<Command> {
        let (vertical_rest, horizontal_rest) = &mut self.scroll_remainder;
        let mut commands = Vec::new();
        for (axis, delta, rest, sign) in [
            (VERTICAL_AXIS, vertical, vertical_rest, -1),
            (HORIZONTAL_AXIS, horizontal, horizontal_rest, 1),
        ] {
            *rest = rest.saturating_add(delta);
            let steps = *rest / UNITS_PER_DETENT;
            if steps != 0 {
                *rest -= steps * UNITS_PER_DETENT;
                commands.push(Command::Scroll {
                    axis,
                    steps: steps * sign,
                });
            }
        }
        commands
    }
}

impl InputInjector for MutterInjector {
    fn inject(&mut self, action: &InputAction) -> Result<(), String> {
        for command in self.commands(action)? {
            self.send(command)?;
        }
        Ok(())
    }

    fn description(&self) -> &str {
        "Mutter remote desktop"
    }

    fn supports_touch(&self) -> bool {
        true
    }
}

/// Returns a position or motion in QDP's units in pixels.
fn pixels(units: i32) -> f64 {
    f64::from(units) / UNITS_PER_PIXEL
}

/// Returns a button's evdev code.
fn button_code(button: PointerButton) -> i32 {
    match button {
        PointerButton::Left => 0x110,    // BTN_LEFT
        PointerButton::Right => 0x111,   // BTN_RIGHT
        PointerButton::Middle => 0x112,  // BTN_MIDDLE
        PointerButton::Back => 0x113,    // BTN_SIDE
        PointerButton::Forward => 0x114, // BTN_EXTRA
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qdp_runtime::TouchPoint;

    /// Returns an injector whose commands are kept for the test to read.
    fn injector() -> (MutterInjector, UnboundedReceiver<Command>) {
        let (sender, receiver) = unbounded_channel();
        (MutterInjector::new(sender, "/stream".to_owned()), receiver)
    }

    /// Returns every command the injector sent.
    fn sent(receiver: &mut UnboundedReceiver<Command>) -> Vec<Command> {
        std::iter::from_fn(|| receiver.try_recv().ok()).collect()
    }

    /// Keys go by their evdev codes, and keys without one are dropped.
    #[test]
    fn presses_keys_by_evdev_code() {
        let (mut injector, mut receiver) = injector();
        injector.inject(&InputAction::KeyDown(0x04)).unwrap();
        injector.inject(&InputAction::KeyUp(0x04)).unwrap();
        injector.inject(&InputAction::KeyDown(0x00)).unwrap();
        assert_eq!(
            sent(&mut receiver),
            [
                Command::Key {
                    code: 30,
                    pressed: true
                },
                Command::Key {
                    code: 30,
                    pressed: false
                },
            ]
        );
    }

    /// Scrolling goes as whole detents, down and right positive, with the
    /// rest kept for the next scroll.
    #[test]
    fn scrolls_whole_detents() {
        let (mut injector, mut receiver) = injector();
        let scroll = |vertical, horizontal| InputAction::Scroll {
            vertical,
            horizontal,
        };
        injector.inject(&scroll(60, 0)).unwrap();
        assert!(sent(&mut receiver).is_empty());
        injector.inject(&scroll(60, 240)).unwrap();
        assert_eq!(
            sent(&mut receiver),
            [
                Command::Scroll {
                    axis: VERTICAL_AXIS,
                    steps: -1
                },
                Command::Scroll {
                    axis: HORIZONTAL_AXIS,
                    steps: 2
                },
            ]
        );
    }

    /// Pointer moves, buttons and touches name the stream's positions.
    #[test]
    fn moves_clicks_and_touches() {
        let (mut injector, mut receiver) = injector();
        injector
            .inject(&InputAction::PointerMoveTo {
                x: 10 * 256,
                y: 20 * 256,
            })
            .unwrap();
        injector
            .inject(&InputAction::ButtonDown(PointerButton::Left))
            .unwrap();
        let touch = |phase| TouchPoint {
            id: 3,
            phase,
            x: 5 * 256,
            y: 6 * 256,
            pressure: 0,
        };
        injector
            .inject(&InputAction::Touch(vec![touch(TouchPhase::Down)]))
            .unwrap();
        injector
            .inject(&InputAction::Touch(vec![touch(TouchPhase::Cancel)]))
            .unwrap();
        assert_eq!(
            sent(&mut receiver),
            [
                Command::MoveTo { x: 10.0, y: 20.0 },
                Command::Button {
                    code: 0x110,
                    pressed: true
                },
                Command::TouchDown {
                    slot: 3,
                    x: 5.0,
                    y: 6.0
                },
                Command::TouchUp { slot: 3 },
            ]
        );
    }

    /// Input the session cannot perform is refused.
    #[test]
    fn refuses_text() {
        let (mut injector, _receiver) = injector();
        assert!(injector.inject(&InputAction::Text("a".to_owned())).is_err());
    }
}
