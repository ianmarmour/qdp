//! The stream window: a top-level window of the core's own that shows a
//! session's video, drawn with Direct3D 11 on a render thread, and reads the
//! user's input for it, with the host's cursor as its pointer (Section 9).
//!
//! winit allows one event loop a process, so every stream window runs on one
//! thread, started with the first session and kept for the next: the app
//! asks it for windows with [`send`]. Each session has a pump thread that
//! reads its events, passing those of the app's to its [`EventQueue`] and
//! those of the window's back to the window thread.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use qdp::message::{CursorImage, DesktopRect, PointerKind, PointerMode};
use qdp_app::cursor::ScaledCursor;
use qdp_app::events::{Ending, Event};
use qdp_app::placement::confined;
use qdp_app::session::{Config, EventReader, Session};
use qdp_app::window_input::{Scroller, button_input, display_size, key_input, touch_phase};
use qdp_desktop::{D3d11Video, GamepadCapture, SyncMode, WindowPen};
use qdp_runtime::{Focus, Microphone, PenInput, UserInput, WarpKind};
use tracing::{debug, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, Touch, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::monitor::MonitorHandle;
use winit::platform::windows::{
    EventLoopBuilderExtWindows, IconExtWindows, MonitorHandleExtWindows, WindowAttributesExtWindows,
};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::{
    CursorGrabMode, CustomCursor, Fullscreen, Icon, Theme, Window, WindowAttributes, WindowId,
};

use crate::events::{EventQueue, WindowLine};
use crate::stage::{FrameId, Presentations, SharedStage, Stage, WindowsPresenter, lock, reporter};

/// The name of the thread the stream windows run on.
const WINDOW_THREAD: &str = "qdp-window";

/// The name of the thread that reads a session's events.
const PUMP_THREAD: &str = "qdp-events";

/// How long the pump waits for an event before looking whether the session
/// ended without telling.
const PUMP_WAIT: Duration = Duration::from_millis(250);

/// The share of the monitor's width and height a window, not full screen,
/// takes at most when it opens.
const WINDOWED_SHARE: f64 = 0.8;

/// The shape of a window, not full screen, that opens without a resolution
/// chosen: 16:9.
const WINDOWED_ASPECT: f64 = 16.0 / 9.0;

/// The shortest time between two warps of the user's pointer: at most 60 a
/// second (Section 9.4).
const MIN_WARP_INTERVAL: Duration = Duration::from_micros(16_667);

/// The interface scale, in percent, of a cursor image drawn for no scaling.
const DEFAULT_IMAGE_SCALE: f64 = 100.0;

/// What the window does itself for a key held with Ctrl, Alt and Shift,
/// which it never sends to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LocalAction {
    /// Releases the pointer and the keys held (Section 8.9): Z.
    Release,
    /// Turns the microphone on or off (Section 7.7): M.
    ToggleMicrophone,
    /// Goes full screen, or back to a window: X.
    ToggleFullScreen,
    /// Ends the session: Q.
    Disconnect,
}

impl LocalAction {
    /// Returns the action of a key held with Ctrl, Alt and Shift, if it has
    /// one.
    fn of(key: KeyCode) -> Option<Self> {
        match key {
            KeyCode::KeyZ => Some(Self::Release),
            KeyCode::KeyM => Some(Self::ToggleMicrophone),
            KeyCode::KeyX => Some(Self::ToggleFullScreen),
            KeyCode::KeyQ => Some(Self::Disconnect),
            _ => None,
        }
    }
}

/// The video size the user chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Resolution {
    /// The window's own size, pixel for pixel.
    Window,
    /// This many rows, in the window's shape, scaled to fit it; or the
    /// window's own size where it has no more rows.
    Rows(u32),
    /// Exactly this width and height, scaled to fit the window.
    Exact(u32, u32),
}

impl Resolution {
    /// Returns the video size to ask for in a window of `window` pixels, or
    /// `None` for the window's own.
    fn in_window(self, window: PhysicalSize<u32>) -> Option<(u32, u32)> {
        match self {
            Self::Window => None,
            Self::Rows(rows) if rows >= window.height || window.width == 0 => None,
            Self::Rows(rows) => {
                let width = f64::from(rows) * f64::from(window.width) / f64::from(window.height);
                // Encoders want even sides.
                Some((((width / 2.0).round() as u32 * 2).max(2), rows))
            }
            Self::Exact(width, height) => Some((width, height)),
        }
    }

    /// Returns the shape a window, not full screen, opens in, if the
    /// resolution has one of its own.
    fn shape(self) -> Option<(u32, u32)> {
        match self {
            Self::Window | Self::Rows(_) => None,
            Self::Exact(width, height) => Some((width, height)),
        }
    }
}

/// How the stream window shows the video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Screen {
    /// In a window the user may move and resize.
    Windowed,
    /// Over the whole monitor, without borders.
    FullScreen,
}

/// What the user chose for the stream window.
#[derive(Debug, Clone)]
pub(crate) struct WindowChoices {
    /// The window's title, to which the window adds whether the microphone
    /// is on and the session is reconnecting.
    pub(crate) title: String,
    /// How the window shows the video as it opens.
    pub(crate) screen: Screen,
    /// How the video meets the display's refresh.
    pub(crate) sync: SyncMode,
    /// The video size to ask for: the picture is scaled to fit the window.
    pub(crate) resolution: Resolution,
    /// The app's window, whose monitor the stream window opens on, if
    /// given.
    pub(crate) owner: Option<isize>,
    /// The icon file the window shows in its title bar and the taskbar, if
    /// given.
    pub(crate) icon: Option<PathBuf>,
}

/// A session for a stream window to start.
pub(crate) struct Start {
    /// The session's number, which the app's messages for it carry.
    pub(crate) serial: u64,
    /// What the session connects with; its display is the window's.
    pub(crate) config: Config,
    /// What the user chose for the window.
    pub(crate) choices: WindowChoices,
    /// Where the app's lines go.
    pub(crate) queue: Arc<EventQueue>,
}

/// What the window thread is asked to do.
pub(crate) enum Command {
    /// Opens a stream window and starts its session, ending any other.
    Start(Box<Start>),
    /// Something for the session with this number.
    Session(u64, SessionMessage),
}

/// Something for a session's window.
#[derive(Debug)]
pub(crate) enum SessionMessage {
    /// One of the session's events, from its pump.
    Event(Event),
    /// The pen changed over the window, at a position in its pixels.
    Pen(PenInput),
    /// The user's answer to the request for a pairing code: the code, or
    /// `None` if they declined.
    PairingCode(Option<String>),
    /// The app ends the session.
    Stop,
}

/// The window thread, once started.
static WINDOW_THREAD_PROXY: Mutex<Option<EventLoopProxy<Command>>> = Mutex::new(None);

/// Asks the window thread to do `command`, starting it the first time.
pub(crate) fn send(command: Command) -> Result<(), String> {
    let mut proxy = WINDOW_THREAD_PROXY
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if proxy.is_none() {
        *proxy = Some(start_thread()?);
    }
    let Some(running) = proxy.as_ref() else {
        return Err("the window thread is not running".to_owned());
    };
    if running.send_event(command).is_err() {
        // The thread ended; the next command starts it again.
        *proxy = None;
        return Err("the window thread has ended".to_owned());
    }
    Ok(())
}

/// Starts the thread the stream windows run on, returning what wakes its
/// event loop.
fn start_thread() -> Result<EventLoopProxy<Command>, String> {
    let (started, proxy) = mpsc::channel();
    std::thread::Builder::new()
        .name(WINDOW_THREAD.to_owned())
        .spawn(move || {
            let event_loop = EventLoop::<Command>::with_user_event()
                .with_any_thread(true)
                .build();
            let event_loop = match event_loop {
                Ok(event_loop) => event_loop,
                Err(error) => {
                    let _ = started.send(Err(format!("cannot start the window system: {error}")));
                    return;
                }
            };
            let proxy = event_loop.create_proxy();
            let _ = started.send(Ok(proxy.clone()));
            let mut windows = Windows {
                proxy,
                stream: None,
            };
            if let Err(error) = event_loop.run_app(&mut windows) {
                warn!(%error, "the window thread's event loop failed");
            }
            if let Some(stream) = windows.stream.take() {
                stream.close();
            }
        })
        .map_err(|error| format!("cannot start the window thread: {error}"))?;
    proxy
        .recv()
        .map_err(|_| "the window thread did not start".to_owned())?
}

/// The window thread's application: the stream window, while a session
/// runs.
struct Windows {
    /// Wakes the event loop, for the pump threads and the pen.
    proxy: EventLoopProxy<Command>,
    /// The session's window, while one runs.
    stream: Option<StreamWindow>,
}

impl ApplicationHandler<Command> for Windows {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn user_event(&mut self, event_loop: &ActiveEventLoop, command: Command) {
        match command {
            Command::Start(start) => {
                if let Some(stream) = self.stream.take() {
                    stream.close();
                }
                let queue = Arc::clone(&start.queue);
                match StreamWindow::open(event_loop, *start, &self.proxy) {
                    Ok(stream) => self.stream = Some(stream),
                    Err(error) => {
                        warn!(error, "the stream window cannot open");
                        queue.finish(&Ending::Failed(error));
                    }
                }
            }
            Command::Session(serial, message) => {
                let Some(stream) = self
                    .stream
                    .as_mut()
                    .filter(|stream| stream.serial == serial)
                else {
                    debug!(serial, ?message, "a message for a session that ended");
                    return;
                };
                if stream.message(message) == Continue::Close {
                    if let Some(stream) = self.stream.take() {
                        stream.close();
                    }
                }
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window: WindowId, event: WindowEvent) {
        let Some(stream) = self
            .stream
            .as_mut()
            .filter(|stream| stream.window.id() == window)
        else {
            return;
        };
        if stream.window_event(event_loop, event) == Continue::Close {
            if let Some(stream) = self.stream.take() {
                stream.close();
            }
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device: DeviceId,
        event: DeviceEvent,
    ) {
        if let Some(stream) = &self.stream {
            stream.device_event(event);
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let control_flow = match &mut self.stream {
            Some(stream) => stream.about_to_wait(event_loop),
            None => ControlFlow::Wait,
        };
        event_loop.set_control_flow(control_flow);
    }
}

/// Whether a stream window stays open after an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Continue {
    /// It stays open.
    Open,
    /// It closes, ending its session.
    Close,
}

/// Whether the session is connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Link {
    /// Connected, or connecting for the first time.
    Up,
    /// The connection was lost and is being made again.
    Reconnecting,
}

/// A session's window and what it reads the user's input with.
///
/// The fields are dropped in order: the pen and the render thread before
/// the window they use.
struct StreamWindow {
    /// The session's number.
    serial: u64,
    /// Where the app's lines go.
    queue: Arc<EventQueue>,
    /// The pen over the window, read until dropped.
    _pen: Option<WindowPen>,
    /// The user's gamepads, if they can be watched.
    gamepads: Option<GamepadCapture>,
    /// The session, until the window closes.
    session: Option<Session>,
    /// The render thread, which draws until dropped.
    _video: D3d11Video<FrameId>,
    /// What the render thread draws.
    stage: SharedStage,
    /// The window.
    window: Window,
    /// What the user chose for the window.
    choices: WindowChoices,
    /// Whether the window shows the video yet: it opens hidden, and shows
    /// once the first picture arrives.
    shown: bool,
    /// Whether the session is connected.
    link: Link,
    /// Whether the microphone is on.
    microphone: Microphone,
    /// The host's cursor image, or `None` while the host hides it.
    cursor: Option<CursorImage>,
    /// The cursor at the size it is drawn, with the scale it was made for.
    scaled_cursor: Option<(f64, ScaledCursor)>,
    /// Whether the window's pointer must be set again: the image or the
    /// picture's scale changed.
    cursor_stale: bool,
    /// Where on the host's monitor the window draws the cursor itself, in
    /// pixels: while a device the client does not know moves it, or after a
    /// warp the window's pointer could not follow, until the user's pointer
    /// moves.
    follow: Option<(f64, f64)>,
    /// A warp waiting for the warp rate limit, to apply once it allows.
    pending_warp: Option<(f64, f64)>,
    /// When the user's pointer was last warped.
    last_warp: Option<Instant>,
    /// Whether the window has keyboard focus.
    focused: bool,
    /// The host's pointer mode.
    pointer_mode: PointerKind,
    /// Whether the window has captured the pointer for relative mode.
    captured: bool,
    /// The area of the host's monitor its applications confined the pointer
    /// to, in absolute mode, in positions (Section 8.9).
    confinement: Option<DesktopRect>,
    /// Whether the user released the confinement with the local release
    /// action, until the host's next pointer mode.
    confinement_released: bool,
    /// The modifier keys held.
    modifiers: ModifiersState,
    /// The fingers touching the window.
    fingers: HashSet<u64>,
    /// The pointer buttons held, as the window saw them.
    held_buttons: HashSet<MouseButton>,
    /// Scrolling not yet sent, below one unit.
    scroller: Scroller,
}

impl StreamWindow {
    /// Opens a hidden stream window for `start` and starts its session,
    /// with its pump.
    fn open(
        event_loop: &ActiveEventLoop,
        start: Start,
        proxy: &EventLoopProxy<Command>,
    ) -> Result<Self, String> {
        let Start {
            serial,
            mut config,
            choices,
            queue,
        } = start;
        let microphone = config.microphone;
        let monitor = monitor(event_loop, choices.owner);
        let window = event_loop
            .create_window(attributes(&choices, microphone, monitor.as_ref()))
            .map_err(|error| format!("cannot open the stream window: {error}"))?;
        let hwnd = hwnd(&window).ok_or("the stream window has no handle")?;
        let presentations = Presentations::default();
        // SAFETY: the window outlives the render thread, which is dropped
        // first.
        #[allow(unsafe_code)]
        let video =
            unsafe { D3d11Video::attach(hwnd, choices.sync, reporter(Arc::clone(&presentations))) }
                .map_err(|error| format!("cannot draw the video with Direct3D: {error}"))?;
        let size = window.inner_size();
        let stage: SharedStage = Arc::new(Mutex::new(Stage::new(
            video.submitter(),
            (size.width, size.height),
        )));
        config.display = display_size(&window, choices.resolution.in_window(size));
        info!(
            width = config.display.width,
            height = config.display.height,
            refresh_millihertz = config.display.refresh_millihertz,
            scale_percent = config.display.scale_percent,
            "opening the stream"
        );
        let session = Session::start(config, {
            let stage = Arc::clone(&stage);
            move || WindowsPresenter::new(stage, presentations)
        })
        .map_err(|error| format!("cannot start the session: {error}"))?;
        start_pump(session.events(), serial, proxy.clone(), Arc::clone(&queue))?;
        let gamepads = GamepadCapture::start(session.input())
            .inspect_err(|error| warn!(error, "gamepads are not sent"))
            .ok();
        let pen = attach_pen(hwnd, serial, proxy.clone());
        Ok(Self {
            serial,
            queue,
            _pen: pen,
            gamepads,
            session: Some(session),
            _video: video,
            stage,
            window,
            choices,
            shown: false,
            link: Link::Up,
            microphone,
            cursor: None,
            scaled_cursor: None,
            cursor_stale: false,
            follow: None,
            pending_warp: None,
            last_warp: None,
            focused: false,
            pointer_mode: PointerKind::Absolute,
            captured: false,
            confinement: None,
            confinement_released: false,
            modifiers: ModifiersState::empty(),
            fingers: HashSet::new(),
            held_buttons: HashSet::new(),
            scroller: Scroller::default(),
        })
    }

    /// Closes the window at once and ends its session on a thread of its
    /// own, which tells the app the session ended unless the session did.
    fn close(mut self) {
        self.window.set_visible(false);
        let session = self.session.take();
        let queue = Arc::clone(&self.queue);
        // The pen, the gamepads and the render thread go before the window.
        drop(self);
        let ending = std::thread::Builder::new()
            .name(PUMP_THREAD.to_owned())
            .spawn({
                let queue = Arc::clone(&queue);
                move || {
                    if let Some(session) = session {
                        session.stop();
                    }
                    queue.finish(&Ending::Normal);
                }
            });
        if let Err(error) = ending {
            warn!(%error, "cannot end the session on a thread of its own");
            queue.finish(&Ending::Normal);
        }
    }

    /// Sends one of the user's inputs to the session.
    fn send(&self, input: UserInput) {
        if let Some(session) = &self.session {
            session.send(input);
        }
    }

    /// Handles a message for the session's window.
    fn message(&mut self, message: SessionMessage) -> Continue {
        match message {
            SessionMessage::Event(event) => return self.event(event),
            SessionMessage::Pen(pen) => self.pen(pen),
            SessionMessage::PairingCode(code) => {
                if let Some(session) = &self.session {
                    session.answer_pairing(code);
                }
            }
            SessionMessage::Stop => return Continue::Close,
        }
        Continue::Open
    }

    /// Handles one of the session's events that concerns the window.
    fn event(&mut self, event: Event) -> Continue {
        match event {
            Event::Video { .. } => {
                self.cursor_stale = true;
                if !self.shown {
                    self.shown = true;
                    self.window.set_visible(true);
                    self.window.focus_window();
                    self.queue.push(WindowLine::Shown.line());
                }
            }
            Event::Cursor => {
                self.cursor = self.session.as_ref().and_then(Session::cursor);
                self.cursor_stale = true;
            }
            Event::Warp { x, y, kind } => self.warp((x, y), kind),
            Event::PointerMode(mode) => self.set_pointer_mode(mode),
            Event::Rumble { slot, low, high } => {
                if let Some(gamepads) = &self.gamepads {
                    gamepads.rumble(slot, low, high);
                }
            }
            Event::Connected => self.set_link(Link::Up),
            Event::Reconnecting { .. } => self.set_link(Link::Reconnecting),
            Event::Ended(_) => return Continue::Close,
            _ => {}
        }
        Continue::Open
    }

    /// Notes whether the session is connected, which the title shows.
    fn set_link(&mut self, link: Link) {
        if self.link != link {
            self.link = link;
            self.window
                .set_title(&title(&self.choices.title, self.microphone, link));
        }
    }

    /// Captures the pointer for relative mode: hides it and confines it to
    /// the window, which Windows allows where it cannot lock it in place.
    fn capture_pointer(&mut self) {
        let grabbed = self
            .window
            .set_cursor_grab(CursorGrabMode::Locked)
            .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined));
        if let Err(error) = grabbed {
            debug!(%error, "the pointer cannot be captured");
        }
        self.window.set_cursor_visible(false);
        self.captured = true;
        self.pending_warp = None;
        self.set_follow(None);
    }

    /// Releases the pointer from capture, showing the host's cursor again.
    fn release_pointer(&mut self) {
        if let Err(error) = self.window.set_cursor_grab(CursorGrabMode::None) {
            debug!(%error, "the pointer cannot be released");
        }
        self.window
            .set_cursor_visible(self.cursor.is_some() && self.follow.is_none());
        self.captured = false;
    }

    /// Follows the host's pointer mode: relative mode captures the pointer
    /// while the window has focus.
    fn set_pointer_mode(&mut self, mode: PointerMode) {
        self.pointer_mode = mode.mode;
        self.confinement = mode.confinement;
        self.confinement_released = false;
        match mode.mode {
            PointerKind::Relative if self.focused => self.capture_pointer(),
            PointerKind::Relative => {}
            PointerKind::Absolute => {
                self.release_pointer();
                self.confine_pointer();
            }
        }
    }

    /// Returns the confinement that applies now: the host's, in absolute
    /// mode, while the window has focus and the user has not released it.
    fn active_confinement(&self) -> Option<DesktopRect> {
        self.confinement.filter(|_| {
            self.pointer_mode == PointerKind::Absolute && self.focused && !self.confinement_released
        })
    }

    /// Keeps the pointer in the window while the host confines it.
    fn confine_pointer(&self) {
        if self.active_confinement().is_none() {
            return;
        }
        if let Err(error) = self.window.set_cursor_grab(CursorGrabMode::Confined) {
            debug!(%error, "the pointer cannot be confined");
        }
    }

    /// Returns the window's own action for a key event, which is not sent
    /// to the host, if it is one.
    fn local_action(&self, event: &KeyEvent) -> Option<LocalAction> {
        if !(self.modifiers.control_key() && self.modifiers.alt_key() && self.modifiers.shift_key())
        {
            return None;
        }
        match event.physical_key {
            PhysicalKey::Code(code) => LocalAction::of(code),
            PhysicalKey::Unidentified(_) => None,
        }
    }

    /// Does a local action.
    fn act(&mut self, action: LocalAction) -> Continue {
        match action {
            LocalAction::Release => {
                self.release_pointer();
                self.confinement_released = true;
                self.held_buttons.clear();
                self.send(UserInput::ReleaseAll);
            }
            LocalAction::ToggleMicrophone => {
                self.microphone = match self.microphone {
                    Microphone::On => Microphone::Off,
                    Microphone::Off => Microphone::On,
                };
                self.send(UserInput::Microphone(self.microphone));
                self.window
                    .set_title(&title(&self.choices.title, self.microphone, self.link));
                self.queue
                    .push(WindowLine::Microphone(self.microphone).line());
            }
            LocalAction::ToggleFullScreen => {
                let full_screen = self.window.fullscreen().is_some();
                self.window
                    .set_fullscreen((!full_screen).then_some(Fullscreen::Borderless(None)));
            }
            LocalAction::Disconnect => return Continue::Close,
        }
        Continue::Open
    }

    /// Returns where the picture sits in the window and its size, once one
    /// arrived.
    fn placement(&self) -> Option<(qdp_app::placement::Rect, (usize, usize))> {
        lock(&self.stage).placement()
    }

    /// Returns where a position in the window falls on the host's monitor,
    /// in its pixels, or `None` while no picture is shown.
    fn monitor_position(&self, position: PhysicalPosition<f64>) -> Option<(f64, f64)> {
        let (rect, video) = self.placement()?;
        Some(rect.to_video(video, (position.x, position.y)))
    }

    /// Sets the window's pointer to the host's cursor, scaled with the
    /// picture, if it changed.
    fn update_cursor(&mut self, event_loop: &ActiveEventLoop) {
        if !self.cursor_stale {
            return;
        }
        self.cursor_stale = false;
        // The image keeps its size relative to the picture, at the interface
        // scale it was drawn for (Section 9.2).
        let picture = self
            .placement()
            .map_or(1.0, |(rect, video)| rect.width / video.0.max(1) as f64);
        let image_scale = self.cursor.as_ref().map_or(DEFAULT_IMAGE_SCALE, |image| {
            image.scale.into_inner().max(1) as f64
        });
        let factor = picture * DEFAULT_IMAGE_SCALE / image_scale;
        self.scaled_cursor = self
            .cursor
            .as_ref()
            .map(|image| (factor, ScaledCursor::new(image, factor)));
        // The cursor the window draws itself is redrawn at its new size.
        self.set_follow(self.follow);
        let Some((_, scaled)) = &self.scaled_cursor else {
            self.window.set_cursor_visible(false);
            return;
        };
        let narrow = |value: usize| u16::try_from(value).unwrap_or(u16::MAX);
        match CustomCursor::from_rgba(
            scaled.rgba.to_vec(),
            narrow(scaled.width),
            narrow(scaled.height),
            narrow(scaled.hotspot.0),
            narrow(scaled.hotspot.1),
        ) {
            Ok(source) => {
                self.window
                    .set_cursor(event_loop.create_custom_cursor(source));
                self.window
                    .set_cursor_visible(self.follow.is_none() && !self.captured);
            }
            Err(error) => warn!(%error, "the host's cursor cannot be shown"),
        }
    }

    /// Draws the cursor at `follow` on the host's monitor, or stops drawing
    /// it with `None`.
    fn set_follow(&mut self, follow: Option<(f64, f64)>) {
        self.follow = follow;
        let cursor = follow.zip(
            self.scaled_cursor
                .as_ref()
                .map(|(_, cursor)| cursor.clone()),
        );
        lock(&self.stage).draw_cursor(cursor);
    }

    /// Follows a move of the host's pointer that the user did not make, in
    /// absolute mode: while the pointer is captured it stays where it is.
    fn warp(&mut self, position: (f64, f64), kind: WarpKind) {
        if self.captured {
            debug!(
                x = position.0,
                y = position.1,
                "a warp while the pointer is captured is ignored"
            );
            return;
        }
        match kind {
            WarpKind::Follow => {
                self.window.set_cursor_visible(false);
                self.set_follow(Some(position));
            }
            WarpKind::Move => {
                // Warps beyond the rate limit are coalesced: the last one is
                // applied once the limit allows (Section 9.4).
                self.pending_warp = Some(position);
                self.apply_pending_warp(Instant::now());
            }
        }
    }

    /// Returns when the pending warp may be applied.
    fn warp_allowed_at(&self) -> Option<Instant> {
        self.pending_warp?;
        Some(
            self.last_warp
                .map_or_else(Instant::now, |last| last + MIN_WARP_INTERVAL),
        )
    }

    /// Applies the pending warp if the rate limit allows: the window's
    /// pointer moves there while the window has focus and the position is on
    /// the picture, and otherwise the window draws the cursor there itself
    /// (Section 9.4).
    fn apply_pending_warp(&mut self, now: Instant) {
        if self.warp_allowed_at().is_none_or(|allowed| now < allowed) {
            return;
        }
        let Some((x, y)) = self.pending_warp.take() else {
            return;
        };
        self.last_warp = Some(now);
        let moved = match self.placement() {
            Some((rect, video)) if self.focused => {
                let (window_x, window_y) = rect.to_layer(video, (x, y));
                rect.contains((window_x, window_y))
                    && self
                        .window
                        .set_cursor_position(PhysicalPosition::new(window_x, window_y))
                        .inspect_err(|error| debug!(%error, "the pointer cannot be moved"))
                        .is_ok()
            }
            _ => false,
        };
        debug!(x, y, moved, "the host warped the pointer");
        if moved {
            self.stop_following();
        } else {
            self.window.set_cursor_visible(false);
            self.set_follow(Some((x, y)));
        }
    }

    /// Stops drawing the cursor where a device the client does not know put
    /// it, because the user's pointer moved.
    fn stop_following(&mut self) {
        if self.follow.is_some() {
            self.window
                .set_cursor_visible(self.cursor.is_some() && !self.captured);
            self.set_follow(None);
        }
    }

    /// Sends the pointer's position on the monitor, if a picture is shown,
    /// kept within the host's confinement while it applies.
    fn pointer_moved(&self, position: PhysicalPosition<f64>) {
        let Some((x, y)) = self.monitor_position(position) else {
            return;
        };
        let (x, y) = match self.active_confinement() {
            Some(area) => confined(area, x, y),
            None => (x, y),
        };
        self.send(UserInput::PointerMoved { x, y });
    }

    /// Sends a finger touching, moving or lifting. While fingers touch, the
    /// pointer events Windows makes from them are not sent.
    fn touch(&mut self, touch: Touch) {
        let phase = touch_phase(touch.phase);
        match touch.phase {
            winit::event::TouchPhase::Started => {
                self.fingers.insert(touch.id);
            }
            winit::event::TouchPhase::Ended | winit::event::TouchPhase::Cancelled => {
                self.fingers.remove(&touch.id);
            }
            winit::event::TouchPhase::Moved => {}
        }
        let Some((x, y)) = self.monitor_position(touch.location) else {
            return;
        };
        self.send(UserInput::Touch {
            finger: touch.id,
            phase,
            x,
            y,
            pressure: touch.force.map(|force| force.normalized()),
        });
    }

    /// Sends the pen's state, at its position on the host's monitor.
    fn pen(&self, pen: PenInput) {
        let Some((x, y)) = self.monitor_position(PhysicalPosition::new(pen.x, pen.y)) else {
            return;
        };
        self.send(UserInput::Pen(PenInput { x, y, ..pen }));
    }

    /// Sends a button press or release.
    fn button(&mut self, button: MouseButton, state: ElementState) {
        match state {
            ElementState::Pressed => self.held_buttons.insert(button),
            ElementState::Released => self.held_buttons.remove(&button),
        };
        if let Some(input) = button_input(button, state) {
            self.send(input);
        }
    }

    /// Tells the session the window's display changed: its size, its scale
    /// or the monitor it is on. The session asks for a matching stream. A
    /// minimized window has no display, and keeps the one it had.
    fn display_changed(&self) {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.send(UserInput::Display(display_size(
            &self.window,
            self.choices.resolution.in_window(size),
        )));
    }

    /// Follows the window's focus: input goes to the host only while the
    /// window has it.
    fn set_focused(&mut self, focused: bool) {
        debug!(focused, "window focus");
        self.focused = focused;
        self.send(UserInput::Focus(if focused {
            Focus::Focused
        } else {
            Focus::Unfocused
        }));
        if let Some(gamepads) = &self.gamepads {
            gamepads.set_focused(focused);
        }
        if focused {
            if self.pointer_mode == PointerKind::Relative {
                self.capture_pointer();
            }
            self.confine_pointer();
            // The host's lock keys follow the client's (Section 8.4.1).
            if let Some(lock_keys) = qdp_desktop::lock_keys() {
                self.send(UserInput::LockKeys(lock_keys));
            }
        } else {
            self.release_pointer();
            self.fingers.clear();
            self.held_buttons.clear();
            self.send(UserInput::ReleaseAll);
        }
    }

    /// Handles an event of the window.
    fn window_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) -> Continue {
        match event {
            WindowEvent::CloseRequested => return Continue::Close,
            WindowEvent::Resized(size) => {
                self.cursor_stale = true;
                lock(&self.stage).resize((size.width, size.height));
                self.display_changed();
            }
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Moved(_) => {
                self.display_changed();
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            // On gaining focus the window library makes up presses for the
            // keys already held, which the user did not press in the
            // session.
            WindowEvent::KeyboardInput {
                is_synthetic: true, ..
            } => {}
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(action) = self.local_action(&event) {
                    if event.state == ElementState::Pressed && !event.repeat {
                        return self.act(action);
                    }
                } else if let Some(input) = key_input(&event) {
                    self.send(input);
                }
            }
            WindowEvent::Touch(touch) => self.touch(touch),
            // Pointer events made from touches are not sent: the touches are.
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. }
                if !self.fingers.is_empty() => {}
            WindowEvent::CursorMoved { position, .. } if !self.captured => {
                self.stop_following();
                self.pointer_moved(position);
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } if self.pointer_mode == PointerKind::Relative && !self.captured => {
                // A click on the window captures the pointer again; it is
                // not sent.
                self.capture_pointer();
            }
            WindowEvent::MouseInput { state, button, .. } => self.button(button, state),
            WindowEvent::MouseWheel { delta, .. } => {
                if let Some(input) = self.scroller.scroll(delta, self.window.scale_factor()) {
                    self.send(input);
                }
            }
            WindowEvent::Focused(focused) => self.set_focused(focused),
            WindowEvent::RedrawRequested => lock(&self.stage).submit(None),
            _ => {}
        }
        self.update_cursor(event_loop);
        Continue::Open
    }

    /// Handles raw input: unaccelerated motion drives the mouse in relative
    /// mode.
    fn device_event(&self, event: DeviceEvent) {
        if let (DeviceEvent::MouseMotion { delta: (dx, dy) }, true) = (event, self.captured) {
            self.send(UserInput::MouseMoved { dx, dy });
        }
    }

    /// Applies what waited for the event loop to go idle, returning how long
    /// it may wait.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) -> ControlFlow {
        self.update_cursor(event_loop);
        self.apply_pending_warp(Instant::now());
        match self.warp_allowed_at() {
            Some(allowed) => ControlFlow::WaitUntil(allowed),
            None => ControlFlow::Wait,
        }
    }
}

/// Returns the window's title: the session's, saying whether the microphone
/// is on (Section 7.7) and the session is reconnecting.
fn title(base: &str, microphone: Microphone, link: Link) -> String {
    let mut title = base.to_owned();
    if microphone == Microphone::On {
        title.push_str(" (microphone on)");
    }
    if link == Link::Reconnecting {
        title.push_str(" (reconnecting)");
    }
    title
}

/// Returns the monitor the window opens on: the app's window's, if given,
/// or the primary one.
#[allow(unsafe_code)]
fn monitor(event_loop: &ActiveEventLoop, owner: Option<isize>) -> Option<MonitorHandle> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};

    let owner_monitor = owner.map(|hwnd| {
        // SAFETY: the call has no contract; a window that no longer exists
        // gives the nearest monitor, or the primary one.
        unsafe { MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST) }
    });
    owner_monitor
        .and_then(|wanted| {
            event_loop
                .available_monitors()
                .find(|monitor| monitor.hmonitor() == wanted.0 as isize)
        })
        .or_else(|| event_loop.primary_monitor())
}

/// Returns the attributes the window opens with: hidden, dark, on
/// `monitor`, full screen or centred on it as the user chose.
fn attributes(
    choices: &WindowChoices,
    microphone: Microphone,
    monitor: Option<&MonitorHandle>,
) -> WindowAttributes {
    let mut attributes = Window::default_attributes()
        .with_title(title(&choices.title, microphone, Link::Up))
        .with_visible(false)
        .with_theme(Some(Theme::Dark));
    if let Some(icon) = choices.icon.as_ref().and_then(|path| {
        Icon::from_path(path, None)
            .inspect_err(|error| warn!(%error, "the window's icon cannot be read"))
            .ok()
    }) {
        attributes = attributes
            .with_window_icon(Some(icon.clone()))
            .with_taskbar_icon(Some(icon));
    }
    let Some(monitor) = monitor else {
        return attributes;
    };
    if choices.screen == Screen::FullScreen {
        return attributes.with_fullscreen(Some(Fullscreen::Borderless(Some(monitor.clone()))));
    }
    let area = monitor.size();
    let size = windowed_size((area.width, area.height), choices.resolution.shape());
    let origin = monitor.position();
    attributes
        .with_inner_size(PhysicalSize::new(size.0, size.1))
        .with_position(PhysicalPosition::new(
            origin.x + (i64::from(area.width) - i64::from(size.0)).max(0) as i32 / 2,
            origin.y + (i64::from(area.height) - i64::from(size.1)).max(0) as i32 / 2,
        ))
}

/// Returns the size a window, not full screen, opens at on a `monitor` of
/// that many pixels: the shape of the `resolution` chosen, or 16:9, as
/// large as fits in [`WINDOWED_SHARE`] of the monitor, and never larger than
/// the resolution chosen.
fn windowed_size(monitor: (u32, u32), resolution: Option<(u32, u32)>) -> (u32, u32) {
    let (most_width, most_height) = (
        f64::from(monitor.0) * WINDOWED_SHARE,
        f64::from(monitor.1) * WINDOWED_SHARE,
    );
    let (width, height) = match resolution {
        Some((width, height)) => (f64::from(width.max(1)), f64::from(height.max(1))),
        None => (most_width, most_width / WINDOWED_ASPECT),
    };
    let scale = (most_width / width).min(most_height / height).min(1.0);
    (
        ((width * scale).round() as u32).max(1),
        ((height * scale).round() as u32).max(1),
    )
}

/// Returns a window's handle.
fn hwnd(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Starts reading the pen over the window `hwnd`, sending its input to the
/// session `serial`'s window.
fn attach_pen(hwnd: isize, serial: u64, proxy: EventLoopProxy<Command>) -> Option<WindowPen> {
    WindowPen::attach(hwnd, move |pen| {
        let _ = proxy.send_event(Command::Session(serial, SessionMessage::Pen(pen)));
    })
    .inspect_err(|error| warn!(error, "the pen is not sent"))
    .ok()
}

/// Returns whether the window handles `event`, besides the app.
fn for_window(event: &Event) -> bool {
    matches!(
        event,
        Event::Video { .. }
            | Event::Cursor
            | Event::Warp { .. }
            | Event::PointerMode(_)
            | Event::Rumble { .. }
            | Event::Connected
            | Event::Reconnecting { .. }
            | Event::Ended(_)
    )
}

/// Starts the thread that reads the session `serial`'s `events`, queueing
/// the app's in `queue` and sending the window's through `proxy`, until the
/// session ends.
fn start_pump(
    events: EventReader,
    serial: u64,
    proxy: EventLoopProxy<Command>,
    queue: Arc<EventQueue>,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name(PUMP_THREAD.to_owned())
        .spawn(move || {
            loop {
                let Some(event) = events.next(PUMP_WAIT) else {
                    if queue.finished() {
                        return;
                    }
                    continue;
                };
                // The app hears of the end before the window closes, so that
                // the window's own ending does not replace it.
                queue.push_event(&event);
                let ended = matches!(event, Event::Ended(_));
                if for_window(&event) {
                    let _ =
                        proxy.send_event(Command::Session(serial, SessionMessage::Event(event)));
                }
                if ended {
                    return;
                }
            }
        })
        .map(drop)
        .map_err(|error| format!("cannot start the session's event thread: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window opens at most at 80 % of the monitor, in the chosen
    /// resolution's shape, never larger than it.
    #[test]
    fn sizes_windows() {
        assert_eq!(windowed_size((1920, 1080), None), (1536, 864));
        assert_eq!(
            windowed_size((3840, 2160), Some((1920, 1080))),
            (1920, 1080)
        );
        assert_eq!(windowed_size((1920, 1200), Some((3840, 2160))), (1536, 864));
        assert_eq!(windowed_size((1080, 1920), None), (864, 486));
    }

    /// Rows keep the window's shape and never exceed its own; exact sizes
    /// are asked for whatever the window.
    #[test]
    fn asks_for_resolutions() {
        let ultrawide = PhysicalSize::new(3440, 1440);
        assert_eq!(Resolution::Window.in_window(ultrawide), None);
        assert_eq!(
            Resolution::Rows(1080).in_window(ultrawide),
            Some((2580, 1080))
        );
        assert_eq!(Resolution::Rows(1440).in_window(ultrawide), None);
        assert_eq!(Resolution::Rows(2160).in_window(ultrawide), None);
        assert_eq!(
            Resolution::Rows(720).in_window(PhysicalSize::new(1920, 1200)),
            Some((1152, 720))
        );
        assert_eq!(
            Resolution::Exact(1920, 1080).in_window(ultrawide),
            Some((1920, 1080))
        );
        assert_eq!(
            Resolution::Rows(720).in_window(PhysicalSize::new(0, 0)),
            None
        );
    }
}
