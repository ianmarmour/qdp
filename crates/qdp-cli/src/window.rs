//! The client's window: shows the newest decoded frame, scaled to fit, with
//! the host's cursor as its pointer (Section 9).
//!
//! The window's event loop must run on the main thread, which macOS requires,
//! so the session runs on another thread. It hands frames over through a
//! [`FrameSlot`] and wakes the event loop with an [`EventLoopProxy`]; the window
//! tells the session it closed through a [`CloseSignal`].

use std::collections::HashSet;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qdp::message::{CursorImage, DesktopRect, FrameNumber, PointerKind, PointerMode, StreamId};
use qdp_codec::Yuv420Frame;
use qdp_desktop::{DeviceSelector, Microphone, PassthroughCapture, TouchpadCapture};
use qdp_runtime::{
    CursorWarp, DisplaySize, DisplayTiming, Focus, FrameInfo, FrameSink, InputHandle, PenInput,
    PointerButton, Presentation, PresentationTiming, SinkControl, TouchPhase, UserInput, WarpKind,
};
use softbuffer::{Context, Surface};
use tokio::sync::{Notify, oneshot};
use tracing::{debug, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, Touch,
    WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorGrabMode, CustomCursor, Window, WindowId};

use crate::args::TouchpadMode;
use crate::color::{Placement, RgbFrame, scale_into, yuv420_to_rgb};
use crate::cursor::ScaledCursor;
use crate::keymap::{HidKey, hid_key};
use crate::sink::CliSink;

/// The window's initial size, in logical pixels.
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 720.0);

/// Scroll units, 1/120 of a detent, per line of scrolling (Section 8.4.2).
const UNITS_PER_LINE: f64 = 120.0;

/// Scroll units per logical pixel of precise scrolling, as from a touchpad
/// (Section 8.4.2).
const UNITS_PER_PIXEL: f64 = 3.0;

/// The shortest time between two warps of the user's pointer: at most 60 a
/// second (Section 9.4).
const MIN_WARP_INTERVAL: Duration = Duration::from_micros(16_667);

/// Positions in pixels (Section 1.5.6).
const POSITION_UNITS: f64 = 256.0;

/// The interface scale, in percent, of an image drawn for no scaling.
const DEFAULT_IMAGE_SCALE: f64 = 100.0;

/// The key that, with Ctrl, Alt and Shift held, releases the pointer from
/// capture (Section 8.9). It is never sent to the host.
const RELEASE_KEY: KeyCode = KeyCode::KeyZ;

/// The key that, with Ctrl, Alt and Shift held, turns the microphone on or
/// off (Section 7.7). It is never sent to the host.
const MICROPHONE_KEY: KeyCode = KeyCode::KeyM;

/// What the window does itself for a key held with Ctrl, Alt and Shift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LocalAction {
    /// Releases the pointer and the keys held.
    Release,
    /// Turns the microphone on or off.
    ToggleMicrophone,
}

/// What the session thread tells the window.
#[derive(Debug, Clone)]
pub enum WindowMessage {
    /// A new frame is in the slot.
    Frame,
    /// The host's cursor changed: an image to draw at the pointer, or `None`
    /// to hide it.
    Cursor(Option<Arc<CursorImage>>),
    /// The host's pointer moved without the user's input.
    Warp(CursorWarp),
    /// The host's pointer mode changed.
    PointerMode(PointerMode),
    /// The pen changed over the window, at a position in the window's
    /// physical pixels. The window library reports pens only as touches, so
    /// the pen is read apart from it where the platform allows.
    #[cfg_attr(not(windows), allow(dead_code))]
    Pen(PenInput),
    /// The session ended, so the window closes.
    SessionEnded,
}

/// A frame of a stream, as presentations name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FrameId {
    /// The frame's stream.
    stream_id: StreamId,
    /// The frame's number in its stream.
    frame_number: FrameNumber,
}

/// What the session and the window share about the frames.
#[derive(Debug, Default)]
struct SlotState {
    /// The newest frame, waiting to be drawn.
    waiting: Option<(Yuv420Frame, FrameId)>,
    /// When the frames drawn or replaced since the session last asked
    /// reached the screen.
    presentations: Vec<Presentation>,
    /// The display's refresh timing, if the window knows it.
    display_timing: Option<DisplayTiming>,
}

/// The newest decoded frame, waiting to be drawn, and what became of the
/// frames before it. A newer frame replaces one not drawn yet, so a window
/// that draws slowly shows the newest picture instead of falling behind,
/// and frames never drawn are never converted.
#[derive(Debug, Clone, Default)]
pub struct FrameSlot(Arc<Mutex<SlotState>>);

impl FrameSlot {
    /// Locks the shared state.
    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Puts a frame in the slot, replacing one not drawn yet.
    fn put(&self, frame: Yuv420Frame, info: &FrameInfo) {
        let id = FrameId {
            stream_id: info.stream_id,
            frame_number: info.frame_number,
        };
        let mut state = self.lock();
        if let Some((_, replaced)) = state.waiting.replace((frame, id)) {
            state.presentations.push(presentation(replaced, None));
        }
    }

    /// Takes the frame waiting to be drawn.
    fn take(&self) -> Option<(Yuv420Frame, FrameId)> {
        self.lock().waiting.take()
    }

    /// Records when a frame taken from the slot reached the screen, or that
    /// it never did.
    fn shown(&self, id: FrameId, presented_at: Option<Instant>) {
        self.lock()
            .presentations
            .push(presentation(id, presented_at));
    }

    /// Returns the presentations recorded since the last call.
    fn take_presentations(&self) -> Vec<Presentation> {
        std::mem::take(&mut self.lock().presentations)
    }

    /// Notes the display's refresh timing.
    fn set_display_timing(&self, timing: Option<DisplayTiming>) {
        self.lock().display_timing = timing;
    }

    /// Returns the display's refresh timing, if known.
    fn display_timing(&self) -> Option<DisplayTiming> {
        self.lock().display_timing
    }
}

/// Returns the presentation of frame `id`.
fn presentation(id: FrameId, presented_at: Option<Instant>) -> Presentation {
    Presentation {
        stream_id: id.stream_id,
        frame_number: id.frame_number,
        presented_at,
    }
}

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
        self.slot.put(frame.clone(), &info);
        if self.proxy.send_event(WindowMessage::Frame).is_err() {
            // The event loop has ended, so the window is gone.
            return SinkControl::Stop;
        }
        control
    }

    fn presentation_timing(&self) -> PresentationTiming {
        PresentationTiming::Deferred
    }

    fn take_presentations(&mut self) -> Vec<Presentation> {
        self.slot.take_presentations()
    }

    fn display_timing(&mut self) -> Option<DisplayTiming> {
        self.slot.display_timing()
    }

    fn set_cursor(&mut self, cursor: Option<&CursorImage>) {
        let cursor = cursor.map(|image| Arc::new(image.clone()));
        let _ = self.proxy.send_event(WindowMessage::Cursor(cursor));
    }

    fn warp_cursor(&mut self, warp: CursorWarp) {
        let _ = self.proxy.send_event(WindowMessage::Warp(warp));
    }

    fn set_pointer_mode(&mut self, mode: &PointerMode) {
        let _ = self.proxy.send_event(WindowMessage::PointerMode(*mode));
    }
}

/// The window and its drawing surface.
struct Display {
    /// The pen over the window, read until dropped, before the window is.
    #[cfg(windows)]
    _pen: Option<qdp_desktop::WindowPen>,
    /// The window.
    window: Rc<Window>,
    /// The surface pixels are drawn into.
    surface: Surface<Rc<Window>, Rc<Window>>,
}

/// The application the window's event loop runs.
pub struct WindowApp {
    /// Whether the microphone is on.
    microphone: Microphone,
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
    /// Wakes the event loop with input read apart from the window library.
    #[cfg_attr(not(windows), allow(dead_code))]
    proxy: EventLoopProxy<WindowMessage>,
    /// Scrolling not yet sent, below one unit.
    scroll_remainder: (f64, f64),
    /// The host's cursor image, or `None` while the host hides it.
    cursor: Option<Arc<CursorImage>>,
    /// The cursor at the size it is drawn, with the scale it was made for.
    scaled_cursor: Option<(f64, ScaledCursor)>,
    /// Whether the window's pointer must be set again: the image or the
    /// picture's scale changed.
    cursor_stale: bool,
    /// Where on the host's monitor to draw the cursor itself, in pixels:
    /// while a device the client does not know moves it, or after a warp the
    /// window's pointer could not follow, until the user's pointer moves.
    follow: Option<(f64, f64)>,
    /// A warp waiting for the warp rate limit, to apply once it allows.
    pending_warp: Option<(f64, f64)>,
    /// Whether the window has keyboard focus.
    focused: bool,
    /// When the user's pointer was last warped.
    last_warp: Option<Instant>,
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
    /// The user's gamepads, if they can be watched.
    #[cfg(feature = "gamepad")]
    gamepads: Option<qdp_desktop::GamepadCapture>,
    /// The devices the user chose to pass through, if any.
    passthrough: Option<PassthroughCapture>,
    /// The touchpad, if its raw contacts are sent.
    touchpad: Option<TouchpadCapture>,
    /// Tells the session the display it is shown on, once the window has
    /// opened and measured it.
    display_sender: Option<oneshot::Sender<DisplaySize>>,
}

/// What the user chose for the window's session.
#[derive(Debug, Clone)]
pub struct WindowChoices<'a> {
    /// The window's title, to which the window adds whether the microphone
    /// is on.
    pub title: String,
    /// Whether the microphone is on as the session starts.
    pub microphone: Microphone,
    /// The devices to pass through.
    pub passthrough: &'a [DeviceSelector],
    /// How the touchpad is sent.
    pub touchpad: TouchpadMode,
}

impl WindowApp {
    /// Creates the application.
    pub fn new(
        choices: WindowChoices<'_>,
        slot: FrameSlot,
        close: Arc<CloseSignal>,
        input: InputHandle,
        proxy: EventLoopProxy<WindowMessage>,
        display_sender: oneshot::Sender<DisplaySize>,
    ) -> Self {
        let WindowChoices {
            title,
            microphone,
            passthrough,
            touchpad,
        } = choices;
        Self {
            microphone,
            display_sender: Some(display_sender),
            title,
            display: None,
            slot,
            current: None,
            close,
            error: None,
            scroll_remainder: (0.0, 0.0),
            cursor: None,
            scaled_cursor: None,
            cursor_stale: false,
            follow: None,
            pending_warp: None,
            focused: false,
            last_warp: None,
            pointer_mode: PointerKind::Absolute,
            captured: false,
            confinement: None,
            confinement_released: false,
            modifiers: ModifiersState::empty(),
            fingers: HashSet::new(),
            proxy,
            #[cfg(feature = "gamepad")]
            gamepads: qdp_desktop::GamepadCapture::start(input.clone())
                .inspect_err(|error| warn!(error, "gamepads are not sent"))
                .ok(),
            passthrough: (!passthrough.is_empty())
                .then(|| PassthroughCapture::start(&input, passthrough)),
            touchpad: match touchpad {
                TouchpadMode::Pointer => None,
                TouchpadMode::Raw => TouchpadCapture::start(&input)
                    .inspect_err(|error| warn!(error, "the touchpad's contacts are not sent"))
                    .ok(),
            },
            input,
        }
    }

    /// Captures the pointer for relative mode: hides it and locks it in
    /// place, or confines it to the window where it cannot be locked.
    fn capture_pointer(&mut self) {
        let Some(display) = &self.display else {
            return;
        };
        let grabbed = display
            .window
            .set_cursor_grab(CursorGrabMode::Locked)
            .or_else(|_| display.window.set_cursor_grab(CursorGrabMode::Confined));
        if let Err(error) = grabbed {
            debug!(%error, "the pointer cannot be captured");
        }
        display.window.set_cursor_visible(false);
        self.captured = true;
    }

    /// Releases the pointer from capture, showing the host's cursor again.
    fn release_pointer(&mut self) {
        let Some(display) = &self.display else {
            return;
        };
        if let Err(error) = display.window.set_cursor_grab(CursorGrabMode::None) {
            debug!(%error, "the pointer cannot be released");
        }
        display
            .window
            .set_cursor_visible(self.cursor.is_some() && self.follow.is_none());
        self.captured = false;
    }

    /// Follows the host's pointer mode: relative mode captures the pointer
    /// while the window has focus.
    fn set_pointer_mode(&mut self, mode: &PointerMode) {
        self.pointer_mode = mode.mode;
        self.confinement = mode.confinement;
        self.confinement_released = false;
        match mode.mode {
            PointerKind::Relative if self.focused => self.capture_pointer(),
            PointerKind::Relative => {}
            _ => {
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

    /// Keeps the pointer in the window while the host confines it: the
    /// positions sent are kept within the host's area too.
    fn confine_pointer(&self) {
        let (Some(display), Some(_)) = (&self.display, self.active_confinement()) else {
            return;
        };
        if let Err(error) = display.window.set_cursor_grab(CursorGrabMode::Confined) {
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
            PhysicalKey::Code(RELEASE_KEY) => Some(LocalAction::Release),
            PhysicalKey::Code(MICROPHONE_KEY) => Some(LocalAction::ToggleMicrophone),
            _ => None,
        }
    }

    /// Returns the window's title, which shows while the microphone is on
    /// (Section 7.7).
    fn window_title(&self) -> String {
        match self.microphone {
            Microphone::On => format!("{} (microphone on)", self.title),
            Microphone::Off => self.title.clone(),
        }
    }

    /// Turns the microphone on or off.
    fn toggle_microphone(&mut self) {
        self.microphone = match self.microphone {
            Microphone::On => Microphone::Off,
            Microphone::Off => Microphone::On,
        };
        self.input.send(UserInput::Microphone(self.microphone));
        if let Some(display) = &self.display {
            display.window.set_title(&self.window_title());
        }
    }

    /// Returns where the picture sits in the window and the picture's size,
    /// once a frame has arrived.
    fn placement(&self) -> Option<(Placement, usize, usize)> {
        let (display, frame) = (self.display.as_ref()?, self.current.as_ref()?);
        let size = display.window.inner_size();
        let placement = Placement::fit(
            frame.width,
            frame.height,
            size.width as usize,
            size.height as usize,
        )?;
        Some((placement, frame.width, frame.height))
    }

    /// Sets the window's pointer to the host's cursor, scaled with the
    /// picture, if it changed.
    fn update_cursor(&mut self, event_loop: &ActiveEventLoop) {
        if !self.cursor_stale || self.display.is_none() {
            return;
        }
        self.cursor_stale = false;
        // The image keeps its size relative to the picture, at the interface
        // scale it was drawn for (Section 9.2).
        let picture = self
            .placement()
            .map_or(1.0, |(placement, width, _)| placement.scale(width));
        let image_scale = self.cursor.as_ref().map_or(DEFAULT_IMAGE_SCALE, |image| {
            image.scale.into_inner().max(1) as f64
        });
        let factor = picture * DEFAULT_IMAGE_SCALE / image_scale;
        self.scaled_cursor = self
            .cursor
            .as_ref()
            .map(|image| (factor, ScaledCursor::new(image, factor)));
        let Some(display) = &self.display else {
            return;
        };
        let Some((_, scaled)) = &self.scaled_cursor else {
            display.window.set_cursor_visible(false);
            return;
        };
        let narrow = |value: usize| u16::try_from(value).unwrap_or(u16::MAX);
        match CustomCursor::from_rgba(
            scaled.rgba.clone(),
            narrow(scaled.width),
            narrow(scaled.height),
            narrow(scaled.hotspot.0),
            narrow(scaled.hotspot.1),
        ) {
            Ok(source) => {
                display
                    .window
                    .set_cursor(event_loop.create_custom_cursor(source));
                display.window.set_cursor_visible(self.follow.is_none());
            }
            Err(error) => warn!(%error, "the host's cursor cannot be shown"),
        }
    }

    /// Follows a move of the host's pointer that the user did not make.
    fn warp(&mut self, warp: CursorWarp) {
        match warp.kind {
            WarpKind::Follow => {
                self.follow = Some(warp.position);
                if let Some(display) = &self.display {
                    display.window.set_cursor_visible(false);
                }
                self.request_redraw();
            }
            WarpKind::Move => {
                // Warps beyond the rate limit are coalesced: the last one is
                // applied once the limit allows (Section 9.4).
                self.pending_warp = Some(warp.position);
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
    /// the picture, and otherwise the window draws the cursor there itself,
    /// so that the drawn cursor always moves (Section 9.4).
    fn apply_pending_warp(&mut self, now: Instant) {
        if self.warp_allowed_at().is_none_or(|allowed| now < allowed) {
            return;
        }
        let Some((x, y)) = self.pending_warp.take() else {
            return;
        };
        self.last_warp = Some(now);
        let moved = match (&self.display, self.placement()) {
            (Some(display), Some((placement, width, height))) if self.focused => {
                let (window_x, window_y) = placement.to_window(x, y, width, height);
                placement.contains(window_x, window_y)
                    && display
                        .window
                        .set_cursor_position(PhysicalPosition::new(window_x, window_y))
                        .inspect_err(|error| debug!(%error, "the pointer cannot be moved"))
                        .is_ok()
            }
            _ => false,
        };
        if moved {
            self.stop_following();
        } else {
            self.follow = Some((x, y));
            if let Some(display) = &self.display {
                display.window.set_cursor_visible(false);
            }
            self.request_redraw();
        }
    }

    /// Stops drawing the cursor where a device the client does not know put
    /// it, because the user's pointer moved.
    fn stop_following(&mut self) {
        if self.follow.take().is_some() {
            if let Some(display) = &self.display {
                display.window.set_cursor_visible(self.cursor.is_some());
            }
            self.request_redraw();
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

    /// Sends the pointer's position on the monitor, if it is over the
    /// picture, kept within the host's confinement while it applies.
    fn pointer_moved(&self, position: PhysicalPosition<f64>) {
        if let Some((x, y)) = self.monitor_position(position) {
            let (x, y) = match self.active_confinement() {
                Some(area) => confined(area, x, y),
                None => (x, y),
            };
            self.input.send(UserInput::PointerMoved { x, y });
        }
    }

    /// Returns where a position in the window falls on the host's monitor, in
    /// its pixels, or `None` while no picture is shown.
    fn monitor_position(&self, position: PhysicalPosition<f64>) -> Option<(f64, f64)> {
        let (Some(display), Some(frame)) = (&self.display, &self.current) else {
            return None;
        };
        let size = display.window.inner_size();
        let placement = Placement::fit(
            frame.width,
            frame.height,
            size.width as usize,
            size.height as usize,
        )?;
        Some(placement.to_frame(position.x, position.y, frame.width, frame.height))
    }

    /// Sends a finger touching, moving or lifting. While fingers touch, the
    /// pointer events some platforms make from them are not sent.
    fn touch(&mut self, touch: Touch) {
        let phase = match touch.phase {
            winit::event::TouchPhase::Started => TouchPhase::Down,
            winit::event::TouchPhase::Moved => TouchPhase::Move,
            winit::event::TouchPhase::Ended => TouchPhase::Up,
            winit::event::TouchPhase::Cancelled => TouchPhase::Cancel,
        };
        match phase {
            TouchPhase::Down => {
                self.fingers.insert(touch.id);
            }
            TouchPhase::Up | TouchPhase::Cancel => {
                self.fingers.remove(&touch.id);
            }
            TouchPhase::Move => {}
        }
        let Some((x, y)) = self.monitor_position(touch.location) else {
            return;
        };
        self.input.send(UserInput::Touch {
            finger: touch.id,
            phase,
            x,
            y,
            pressure: touch.force.map(|force| force.normalized()),
        });
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
            .with_title(self.window_title())
            .with_inner_size(INITIAL_SIZE);
        let window = event_loop
            .create_window(attributes)
            .map(Rc::new)
            .map_err(|error| format!("cannot open a window: {error}"))?;
        let surface = Context::new(Rc::clone(&window))
            .and_then(|context| Surface::new(&context, Rc::clone(&window)))
            .map_err(|error| format!("cannot draw in the window: {error}"))?;
        Ok(Display {
            #[cfg(windows)]
            _pen: attach_pen(&window, self.proxy.clone()),
            window,
            surface,
        })
    }

    /// Sends the pen's state, at its position on the host's monitor.
    fn pen(&self, pen: PenInput) {
        let Some((x, y)) = self.monitor_position(PhysicalPosition::new(pen.x, pen.y)) else {
            return;
        };
        self.input.send(UserInput::Pen(PenInput { x, y, ..pen }));
    }

    /// Tells the session the window's display changed: its size, its scale
    /// or the monitor it is on. The session requests a matching stream.
    fn display_changed(&self) {
        if let Some(display) = &self.display {
            self.input
                .send(UserInput::Display(display_size(&display.window)));
        }
    }

    /// Asks the window system to redraw the window.
    fn request_redraw(&self) {
        if let Some(display) = &self.display {
            display.window.request_redraw();
        }
    }

    /// Draws the newest frame into the window, and the cursor where it
    /// follows a device the client does not know.
    fn draw(&mut self) {
        let newest = self.slot.take();
        if let Some((frame, _)) = &newest {
            let resized = self.current.as_ref().is_none_or(|current| {
                (current.width, current.height) != (frame.width(), frame.height())
            });
            self.current = Some(yuv420_to_rgb(frame));
            if resized {
                self.cursor_stale = true;
            }
        }
        let presented_at = self.show();
        if let Some((_, id)) = newest {
            self.slot.shown(id, presented_at);
        }
        self.slot.set_display_timing(qdp_desktop::display_timing());
    }

    /// Draws the current picture, and the cursor where the window draws it,
    /// returning when the picture reached the screen, if it did.
    fn show(&mut self) -> Option<Instant> {
        let follow = match (self.follow, &self.scaled_cursor, self.placement()) {
            (Some((x, y)), Some((_, cursor)), Some((placement, width, height))) => {
                Some((placement.to_window(x, y, width, height), cursor.clone()))
            }
            _ => None,
        };
        let display = self.display.as_mut()?;
        let size = display.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            // A minimized window has nothing to draw.
            return None;
        };
        if let Err(error) = display.surface.resize(width, height) {
            warn!(%error, "cannot resize the window's surface");
            return None;
        }
        let mut buffer = match display.surface.buffer_mut() {
            Ok(buffer) => buffer,
            Err(error) => {
                warn!(%error, "cannot draw the window");
                return None;
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
        if let Some((position, cursor)) = follow {
            cursor.draw_into(&mut buffer, size.width as usize, position);
        }
        match buffer.present() {
            Ok(()) => Some(Instant::now()),
            Err(error) => {
                warn!(%error, "cannot show the window's picture");
                None
            }
        }
    }
}

impl ApplicationHandler<WindowMessage> for WindowApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.display.is_some() {
            return;
        }
        match self.open(event_loop) {
            Ok(display) => {
                if let Some(sender) = self.display_sender.take() {
                    let _ = sender.send(display_size(&display.window));
                }
                self.display = Some(display);
            }
            Err(message) => self.fail(event_loop, message),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, message: WindowMessage) {
        match message {
            WindowMessage::Frame => self.request_redraw(),
            WindowMessage::Cursor(cursor) => {
                self.cursor = cursor;
                self.cursor_stale = true;
                self.request_redraw();
            }
            WindowMessage::Warp(warp) => self.warp(warp),
            WindowMessage::PointerMode(mode) => self.set_pointer_mode(&mode),
            WindowMessage::Pen(pen) => self.pen(pen),
            WindowMessage::SessionEnded => event_loop.exit(),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.update_cursor(event_loop);
        self.apply_pending_warp(Instant::now());
        event_loop.set_control_flow(match self.warp_allowed_at() {
            Some(allowed) => ControlFlow::WaitUntil(allowed),
            None => ControlFlow::Wait,
        });
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device: DeviceId,
        event: DeviceEvent,
    ) {
        // Raw, unaccelerated motion drives the mouse in relative mode.
        if let (DeviceEvent::MouseMotion { delta: (dx, dy) }, true) = (event, self.captured) {
            self.input.send(UserInput::MouseMoved { dx, dy });
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
            WindowEvent::Resized(_) => {
                self.cursor_stale = true;
                self.display_changed();
                self.request_redraw();
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
            WindowEvent::KeyboardInput { event, .. } if self.local_action(&event).is_some() => {
                if event.state != ElementState::Pressed || event.repeat {
                    return;
                }
                match self.local_action(&event) {
                    Some(LocalAction::Release) => {
                        self.release_pointer();
                        self.confinement_released = true;
                        self.input.send(UserInput::ReleaseAll);
                    }
                    Some(LocalAction::ToggleMicrophone) => self.toggle_microphone(),
                    None => {}
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(&event),
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
            WindowEvent::MouseWheel { delta, .. } => self.scroll(delta),
            WindowEvent::Focused(focused) => {
                self.focused = focused;
                self.input.send(UserInput::Focus(if focused {
                    Focus::Focused
                } else {
                    Focus::Unfocused
                }));
                #[cfg(feature = "gamepad")]
                if let Some(gamepads) = &self.gamepads {
                    gamepads.set_focused(focused);
                }
                if let Some(passthrough) = &self.passthrough {
                    passthrough.set_focused(focused);
                }
                if let Some(touchpad) = &self.touchpad {
                    touchpad.set_focused(focused);
                }
                if focused {
                    self.confine_pointer();
                    // The host's lock keys follow the client's (Section 8.4.1).
                    if let Some(lock_keys) = qdp_desktop::lock_keys() {
                        self.input.send(UserInput::LockKeys(lock_keys));
                    }
                } else {
                    self.release_pointer();
                    self.fingers.clear();
                    self.input.send(UserInput::ReleaseAll);
                }
            }
            _ => {}
        }
    }
}

/// Starts reading the pen over `window`, sending its input through `proxy`.
#[cfg(windows)]
fn attach_pen(
    window: &Window,
    proxy: EventLoopProxy<WindowMessage>,
) -> Option<qdp_desktop::WindowPen> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    qdp_desktop::WindowPen::attach(handle.hwnd.get(), move |pen| {
        // The event loop may have ended already.
        let _ = proxy.send_event(WindowMessage::Pen(pen));
    })
    .inspect(|_| debug!("reading the pen over the window"))
    .inspect_err(|error| warn!(error, "the pen is not sent"))
    .ok()
}

/// Returns a point of the host's monitor, in pixels, moved into `area`, in
/// positions: 1/256 pixel (Section 1.5.6).
fn confined(area: DesktopRect, x: f64, y: f64) -> (f64, f64) {
    let pixels = |position: f64| position / POSITION_UNITS;
    let left = pixels(area.x.into_inner() as f64);
    let top = pixels(area.y.into_inner() as f64);
    // The last position inside, a 1/256 pixel short of the far edges.
    let right = pixels((area.x.into_inner() + area.width.into_inner() as i64 - 1) as f64);
    let bottom = pixels((area.y.into_inner() + area.height.into_inner() as i64 - 1) as f64);
    (
        x.clamp(left, right.max(left)),
        y.clamp(top, bottom.max(top)),
    )
}

/// Returns the display the session is shown on: the window's content area,
/// at the refresh rate of the monitor it is on (Section 6.2).
fn display_size(window: &Window) -> DisplaySize {
    let monitor = window
        .current_monitor()
        .or_else(|| window.primary_monitor());
    let size = window.inner_size();
    DisplaySize {
        width: size.width,
        height: size.height,
        refresh_millihertz: monitor
            .and_then(|monitor| monitor.refresh_rate_millihertz())
            .unwrap_or(0),
        scale_percent: (window.scale_factor() * f64::from(DisplaySize::UNSCALED)).round() as u32,
    }
}

/// Creates the event loop the window runs on. It must be created on the main
/// thread.
pub fn event_loop() -> Result<EventLoop<WindowMessage>, String> {
    EventLoop::with_user_event()
        .build()
        .map_err(|error| format!("cannot start the window system: {error}"))
}
