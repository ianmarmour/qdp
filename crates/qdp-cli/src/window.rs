//! The client's window: shows the newest decoded frame, scaled to fit.
//!
//! The window's event loop must run on the main thread, which macOS requires,
//! so the session runs on another thread. It hands frames over through a
//! [`FrameSlot`] and wakes the event loop with an [`EventLoopProxy`]; the window
//! tells the session it closed through a [`CloseSignal`].

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use qdp_codec::Yuv420Frame;
use qdp_runtime::{FrameInfo, FrameSink, InputHandle, PointerButton, SinkControl, UserInput};
use softbuffer::{Context, Surface};
use tokio::sync::Notify;
use tracing::warn;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::PhysicalKey;
use winit::window::{Window, WindowId};

use crate::color::{Placement, RgbFrame, scale_into, yuv420_to_rgb};
use crate::keymap::{HidKey, hid_key};
use crate::sink::CliSink;

/// The window's initial size, in logical pixels.
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 720.0);

/// Scroll units, 1/120 of a detent, per line of scrolling (Section 8.4.2).
const UNITS_PER_LINE: f64 = 120.0;

/// Scroll units per logical pixel of precise scrolling, as from a touchpad
/// (Section 8.4.2).
const UNITS_PER_PIXEL: f64 = 3.0;

/// What the session thread tells the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMessage {
    /// A new frame is in the slot.
    Frame,
    /// The session ended, so the window closes.
    SessionEnded,
}

/// The newest decoded frame, waiting to be drawn. A newer frame replaces one
/// not drawn yet, so a window that draws slowly shows the newest picture
/// instead of falling behind, and frames never drawn are never converted.
pub type FrameSlot = Arc<Mutex<Option<Yuv420Frame>>>;

/// Raised once when the window closes, so that the session stops.
#[derive(Debug, Default)]
pub struct CloseSignal {
    /// Whether the window has closed.
    closed: AtomicBool,
    /// Wakes the session when the window closes.
    notify: Notify,
}

impl CloseSignal {
    /// Marks the window closed and wakes the session.
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.notify.notify_one();
    }

    /// Returns whether the window has closed.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Waits until the window closes. Only one task may wait.
    pub async fn closed(&self) {
        // `notify_one` keeps a permit when nobody waits yet, so a close before
        // this call is not missed.
        self.notify.notified().await;
    }
}

/// A frame sink that hands frames to the window, keeps the command line's
/// statistics, and stops the session once the window closes.
pub struct WindowSink {
    /// Statistics, the output file and the frame limit.
    inner: CliSink,
    /// Where the newest frame goes.
    slot: FrameSlot,
    /// Wakes the window's event loop.
    proxy: EventLoopProxy<WindowMessage>,
    /// Raised when the window closes.
    close: Arc<CloseSignal>,
}

impl WindowSink {
    /// Creates a sink that feeds the window behind `proxy`.
    pub fn new(
        inner: CliSink,
        slot: FrameSlot,
        proxy: EventLoopProxy<WindowMessage>,
        close: Arc<CloseSignal>,
    ) -> Self {
        Self {
            inner,
            slot,
            proxy,
            close,
        }
    }

    /// Returns the wrapped sink, to finish its output.
    pub fn inner_mut(&mut self) -> &mut CliSink {
        &mut self.inner
    }
}

impl FrameSink for WindowSink {
    fn present(&mut self, frame: &Yuv420Frame, info: FrameInfo) -> SinkControl {
        if self.close.is_closed() {
            return SinkControl::Stop;
        }
        let control = self.inner.present(frame, info);
        if let Ok(mut slot) = self.slot.lock() {
            *slot = Some(frame.clone());
        }
        if self.proxy.send_event(WindowMessage::Frame).is_err() {
            // The event loop has ended, so the window is gone.
            return SinkControl::Stop;
        }
        control
    }
}

/// The window and its drawing surface.
struct Display {
    /// The window.
    window: Rc<Window>,
    /// The surface pixels are drawn into.
    surface: Surface<Rc<Window>, Rc<Window>>,
}

/// The application the window's event loop runs.
pub struct WindowApp {
    /// The window's title.
    title: String,
    /// The window, once the event loop has started.
    display: Option<Display>,
    /// The newest frame to draw.
    slot: FrameSlot,
    /// The frame drawn last, kept for redrawing after a resize.
    current: Option<RgbFrame>,
    /// Raised when the window closes.
    close: Arc<CloseSignal>,
    /// Why the window could not be shown, if it could not.
    error: Option<String>,
    /// Sends the user's input to the session.
    input: InputHandle,
    /// Scrolling not yet sent, below one unit.
    scroll_remainder: (f64, f64),
}

impl WindowApp {
    /// Creates the application.
    pub fn new(
        title: String,
        slot: FrameSlot,
        close: Arc<CloseSignal>,
        input: InputHandle,
    ) -> Self {
        Self {
            title,
            display: None,
            slot,
            current: None,
            close,
            error: None,
            input,
            scroll_remainder: (0.0, 0.0),
        }
    }

    /// Sends a key press or release; key repeats are the host's to generate.
    fn key(&self, event: &KeyEvent) {
        if event.repeat {
            return;
        }
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        let pressed = event.state == ElementState::Pressed;
        let input = match hid_key(code) {
            Some(HidKey::Keyboard(usage)) => UserInput::Key { usage, pressed },
            Some(HidKey::Consumer(usage)) => UserInput::ConsumerKey { usage, pressed },
            None => return,
        };
        self.input.send(input);
    }

    /// Sends the pointer's position on the monitor, if it is over the picture.
    fn pointer_moved(&self, position: PhysicalPosition<f64>) {
        let (Some(display), Some(frame)) = (&self.display, &self.current) else {
            return;
        };
        let size = display.window.inner_size();
        let Some(placement) = Placement::fit(
            frame.width,
            frame.height,
            size.width as usize,
            size.height as usize,
        ) else {
            return;
        };
        let (x, y) = placement.to_frame(position.x, position.y, frame.width, frame.height);
        self.input.send(UserInput::PointerMoved { x, y });
    }

    /// Sends a button press or release.
    fn button(&self, button: MouseButton, state: ElementState) {
        let button = match button {
            MouseButton::Left => PointerButton::Left,
            MouseButton::Right => PointerButton::Right,
            MouseButton::Middle => PointerButton::Middle,
            MouseButton::Back => PointerButton::Back,
            MouseButton::Forward => PointerButton::Forward,
            MouseButton::Other(_) => return,
        };
        self.input.send(UserInput::Button {
            button,
            pressed: state == ElementState::Pressed,
        });
    }

    /// Sends scrolling, in 1/120 of a detent. The window system's deltas
    /// already follow the user's scrolling direction preference.
    fn scroll(&mut self, delta: MouseScrollDelta) {
        let (horizontal, vertical) = match delta {
            // Positive deltas move the content right and down: scrolling left
            // and up, away from the user.
            MouseScrollDelta::LineDelta(x, y) => (
                -f64::from(x) * UNITS_PER_LINE,
                f64::from(y) * UNITS_PER_LINE,
            ),
            MouseScrollDelta::PixelDelta(position) => {
                let scale = self
                    .display
                    .as_ref()
                    .map_or(1.0, |display| display.window.scale_factor());
                (
                    -position.x / scale * UNITS_PER_PIXEL,
                    position.y / scale * UNITS_PER_PIXEL,
                )
            }
        };
        let horizontal = self.scroll_remainder.0 + horizontal;
        let vertical = self.scroll_remainder.1 + vertical;
        let (whole_horizontal, whole_vertical) = (horizontal.trunc(), vertical.trunc());
        self.scroll_remainder = (horizontal - whole_horizontal, vertical - whole_vertical);
        if whole_horizontal != 0.0 || whole_vertical != 0.0 {
            self.input.send(UserInput::Scroll {
                vertical: whole_vertical as i32,
                horizontal: whole_horizontal as i32,
            });
        }
    }

    /// Returns why the window could not be shown, if it could not.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Ends the event loop because the window cannot be shown.
    fn fail(&mut self, event_loop: &ActiveEventLoop, message: String) {
        self.error = Some(message);
        self.close.close();
        event_loop.exit();
    }

    /// Opens the window and its drawing surface.
    fn open(&self, event_loop: &ActiveEventLoop) -> Result<Display, String> {
        let attributes = Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(INITIAL_SIZE);
        let window = event_loop
            .create_window(attributes)
            .map(Rc::new)
            .map_err(|error| format!("cannot open a window: {error}"))?;
        let surface = Context::new(Rc::clone(&window))
            .and_then(|context| Surface::new(&context, Rc::clone(&window)))
            .map_err(|error| format!("cannot draw in the window: {error}"))?;
        Ok(Display { window, surface })
    }

    /// Asks the window system to redraw the window.
    fn request_redraw(&self) {
        if let Some(display) = &self.display {
            display.window.request_redraw();
        }
    }

    /// Draws the newest frame into the window.
    fn draw(&mut self) {
        let newest = self.slot.lock().ok().and_then(|mut slot| slot.take());
        if let Some(frame) = newest {
            self.current = Some(yuv420_to_rgb(&frame));
        }
        let Some(display) = &mut self.display else {
            return;
        };
        let size = display.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            // A minimized window has nothing to draw.
            return;
        };
        if let Err(error) = display.surface.resize(width, height) {
            warn!(%error, "cannot resize the window's surface");
            return;
        }
        let mut buffer = match display.surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                warn!(%error, "cannot draw the window");
                return;
            }
        };
        match &self.current {
            Some(frame) => scale_into(
                frame,
                &mut buffer,
                size.width as usize,
                size.height as usize,
            ),
            None => buffer.fill(0),
        }
        if let Err(error) = buffer.present() {
            warn!(%error, "cannot show the window's picture");
        }
    }
}

impl ApplicationHandler<WindowMessage> for WindowApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.display.is_some() {
            return;
        }
        match self.open(event_loop) {
            Ok(display) => self.display = Some(display),
            Err(message) => self.fail(event_loop, message),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, message: WindowMessage) {
        match message {
            WindowMessage::Frame => self.request_redraw(),
            WindowMessage::SessionEnded => event_loop.exit(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                self.close.close();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => self.draw(),
            WindowEvent::Resized(_) => self.request_redraw(),
            WindowEvent::KeyboardInput { event, .. } => self.key(&event),
            WindowEvent::CursorMoved { position, .. } => self.pointer_moved(position),
            WindowEvent::MouseInput { state, button, .. } => self.button(button, state),
            WindowEvent::MouseWheel { delta, .. } => self.scroll(delta),
            WindowEvent::Focused(false) => {
                self.input.send(UserInput::ReleaseAll);
            }
            _ => {}
        }
    }
}

/// Creates the event loop the window runs on. It must be created on the main
/// thread.
pub fn event_loop() -> Result<EventLoop<WindowMessage>, String> {
    EventLoop::with_user_event()
        .build()
        .map_err(|error| format!("cannot start the window system: {error}"))
}
