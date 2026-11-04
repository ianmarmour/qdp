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
use qdp_codec::DecodedPicture;
use qdp_desktop::{DeviceSelector, Microphone, PassthroughCapture, TouchpadCapture};
use qdp_runtime::{
    CursorWarp, DisplaySize, DisplayTiming, Focus, FrameInfo, FrameSink, InputHandle, PenInput,
    Presentation, PresentationTiming, SinkControl, TouchPhase, UserInput, WarpKind,
};
use softbuffer::{Context, Surface};
use tokio::sync::{Notify, oneshot};
use tracing::{debug, info, warn};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, KeyEvent, MouseButton, MouseScrollDelta, Touch,
    WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{CursorGrabMode, CustomCursor, Fullscreen, Window, WindowId};

use crate::args::{self, TouchpadMode};
use crate::color::{Placement, RgbFrame, scale_into, yuv420_to_rgb};
use crate::sink::CliSink;
use qdp_app::cursor::ScaledCursor;
use qdp_app::placement::confined;
use qdp_app::window_input::{Scroller, button_input, display_size, key_input, touch_phase};

/// The window's initial size, in logical pixels.
const INITIAL_SIZE: LogicalSize<f64> = LogicalSize::new(1280.0, 720.0);

/// The shortest time between two warps of the user's pointer: at most 60 a
/// second (Section 9.4).
const MIN_WARP_INTERVAL: Duration = Duration::from_micros(16_667);

/// The interface scale, in percent, of an image drawn for no scaling.
const DEFAULT_IMAGE_SCALE: f64 = 100.0;

/// The key that, with Ctrl, Alt and Shift held, releases the pointer from
/// capture (Section 8.9). It is never sent to the host.
const RELEASE_KEY: KeyCode = KeyCode::KeyZ;

/// How often the window logs how fast it draws.
const DRAW_STATS_INTERVAL: Duration = Duration::from_secs(1);

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
    waiting: Option<(DecodedPicture, FrameId)>,
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
    fn put(&self, frame: DecodedPicture, info: &FrameInfo) {
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
    fn take(&self) -> Option<(DecodedPicture, FrameId)> {
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
    fn present(&mut self, frame: DecodedPicture, info: FrameInfo) -> SinkControl {
        if self.close.is_closed() {
            return SinkControl::Stop;
        }
        let control = self.inner.record(&frame, info);
        self.slot.put(frame, &info);
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
    /// The compositor, showing pictures the GPU decoded over the window
    /// where it can, dropped before the window.
    #[cfg(target_os = "linux")]
    compositor: Option<qdp_desktop::WaylandVideo>,
    /// What draws the window's picture, dropped before the window.
    painter: Painter,
    /// The window.
    window: Rc<Window>,
}

/// What draws a window's picture.
enum Painter {
    /// The processor, into a surface softbuffer shows.
    Processor(Surface<Rc<Window>, Rc<Window>>),
    /// The GPU, with Metal.
    #[cfg(target_os = "macos")]
    Metal(Box<qdp_desktop::MetalVideo<FrameId>>),
    /// The GPU, with Direct3D 11.
    #[cfg(windows)]
    D3d11(Box<qdp_desktop::D3d11Video<FrameId>>),
}

impl Painter {
    /// Returns whether the GPU draws, on a render thread of its own.
    fn on_gpu(&self) -> bool {
        match self {
            Self::Processor(_) => false,
            #[cfg(target_os = "macos")]
            Self::Metal(_) => true,
            #[cfg(windows)]
            Self::D3d11(_) => true,
        }
    }
}

/// The application the window's event loop runs.
pub struct WindowApp {
    /// Whether the window fills the screen.
    presentation: args::Presentation,
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
    /// The size of the newest frame, once one arrived.
    picture: Option<(usize, usize)>,
    /// The size the processor last drew the background around the
    /// compositor's picture at, while the compositor shows the pictures.
    #[cfg(target_os = "linux")]
    composited: Option<(u32, u32)>,
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
    scroller: Scroller,
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
    /// Whether the window shows yet: it opens hidden, and shows with the
    /// first frame, so that it covers nothing while the client pairs or
    /// signs in to a gateway, as in an app's dialog.
    shown: bool,
    /// The modifier keys held.
    modifiers: ModifiersState,
    /// The fingers touching the window.
    fingers: HashSet<u64>,
    /// The pointer buttons held, as the window saw them.
    held_buttons: HashSet<MouseButton>,
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
    /// The video size the user chose, in pixels, asked for whatever the
    /// window's size.
    resolution: Option<(u32, u32)>,
    /// How fast the window draws the frames.
    draw_stats: DrawStats,
}

/// How fast the window draws new frames, over the last interval: frames
/// decoded faster than they can be drawn are passed over for the newest.
#[derive(Debug)]
struct DrawStats {
    /// When the interval started.
    started: Instant,
    /// The new frames drawn.
    frames: u32,
    /// The time drawing them took, from converting their colours to
    /// presenting them.
    busy: Duration,
    /// The longest draw.
    longest: Duration,
    /// The time spent waiting for a drawable to draw them into, on Metal's
    /// render thread, which does not hold up the window.
    waited: Duration,
}

impl DrawStats {
    /// Starts an interval at `now`.
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            frames: 0,
            busy: Duration::ZERO,
            longest: Duration::ZERO,
            waited: Duration::ZERO,
        }
    }

    /// Counts a new frame drawn in `took`, after `waited` for a drawable,
    /// and logs the interval once it has ended at `now`.
    fn drew(&mut self, took: Duration, waited: Duration, now: Instant) {
        self.frames += 1;
        self.busy += took;
        self.longest = self.longest.max(took);
        self.waited += waited;
        let elapsed = now.saturating_duration_since(self.started);
        if elapsed < DRAW_STATS_INTERVAL {
            return;
        }
        let milliseconds = |time: Duration| format!("{:.1}", time.as_secs_f64() * 1000.0);
        info!(
            drawn_fps = format!("{:.1}", f64::from(self.frames) / elapsed.as_secs_f64()),
            draw_ms = milliseconds(self.busy / self.frames),
            draw_max_ms = milliseconds(self.longest),
            wait_ms = milliseconds(self.waited / self.frames),
            "drawing"
        );
        *self = Self::new(now);
    }
}

/// What the user chose for the window's session.
#[derive(Debug, Clone)]
pub struct WindowChoices<'a> {
    /// The window's title, to which the window adds whether the microphone
    /// is on.
    pub title: String,
    /// Whether the window fills the screen.
    pub presentation: args::Presentation,
    /// Whether the microphone is on as the session starts.
    pub microphone: Microphone,
    /// The devices to pass through.
    pub passthrough: &'a [DeviceSelector],
    /// How the touchpad is sent.
    pub touchpad: TouchpadMode,
    /// The video size to ask for, in pixels, whatever the window's size, if
    /// the user chose one: the picture is stretched to fill the window.
    pub resolution: Option<(u32, u32)>,
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
            presentation,
            microphone,
            passthrough,
            touchpad,
            resolution,
        } = choices;
        Self {
            resolution,
            presentation,
            draw_stats: DrawStats::new(Instant::now()),
            microphone,
            display_sender: Some(display_sender),
            title,
            display: None,
            slot,
            current: None,
            picture: None,
            #[cfg(target_os = "linux")]
            composited: None,
            close,
            error: None,
            scroller: Scroller::default(),
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
            shown: false,
            modifiers: ModifiersState::empty(),
            fingers: HashSet::new(),
            held_buttons: HashSet::new(),
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
        // Moving the pointer would release it: on macOS the window library
        // reconnects the pointer to the mouse whenever it moves it.
        self.pending_warp = None;
        self.follow = None;
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
        let (display, (width, height)) = (self.display.as_ref()?, self.picture?);
        let size = display.window.inner_size();
        let placement = Placement::fit(width, height, size.width as usize, size.height as usize)?;
        Some((placement, width, height))
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
        // On macOS, winit sizes a cursor's image in points, as many as its
        // pixels, which a Retina display doubles: the window's pointer gets
        // an image that much smaller, while the one drawn into the picture
        // keeps its pixels.
        #[cfg(target_os = "macos")]
        let points = self
            .cursor
            .as_ref()
            .map(|image| ScaledCursor::new(image, factor / display.window.scale_factor()));
        #[cfg(target_os = "macos")]
        let scaled = points.as_ref().unwrap_or(scaled);
        let narrow = |value: usize| u16::try_from(value).unwrap_or(u16::MAX);
        match CustomCursor::from_rgba(
            scaled.rgba.to_vec(),
            narrow(scaled.width),
            narrow(scaled.height),
            narrow(scaled.hotspot.0),
            narrow(scaled.hotspot.1),
        ) {
            Ok(source) => {
                display
                    .window
                    .set_cursor(event_loop.create_custom_cursor(source));
                display
                    .window
                    .set_cursor_visible(self.follow.is_none() && !self.captured);
            }
            Err(error) => warn!(%error, "the host's cursor cannot be shown"),
        }
    }

    /// Follows a move of the host's pointer that the user did not make, in
    /// absolute mode: while the pointer is captured it stays where it is,
    /// and moving it would release it.
    fn warp(&mut self, warp: CursorWarp) {
        if self.captured {
            debug!(
                x = warp.position.0,
                y = warp.position.1,
                "a warp while the pointer is captured is ignored"
            );
            return;
        }
        match warp.kind {
            WarpKind::Follow => {
                debug!(
                    x = warp.position.0,
                    y = warp.position.1,
                    "drawing the cursor where the host's device put it"
                );
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

    /// Draws a new frame: at once on the GPU, which hands it to its render
    /// thread, rather than at the window's next redraw, which may come a
    /// refresh later, after a newer frame replaced it; at the next redraw
    /// otherwise.
    fn frame_ready(&mut self) {
        if !self.shown
            && let Some(display) = &self.display
        {
            // Wayland shows windows from the start.
            display.window.set_visible(true);
            self.shown = true;
        }
        if self.on_gpu() {
            self.draw();
            return;
        }
        self.request_redraw();
    }

    /// Returns whether the GPU draws the window's picture.
    fn on_gpu(&self) -> bool {
        self.display
            .as_ref()
            .is_some_and(|display| display.painter.on_gpu())
    }

    /// Returns when the pending warp may be applied, as of `now`.
    fn warp_allowed_at(&self, now: Instant) -> Option<Instant> {
        self.pending_warp?;
        Some(warp_due(self.last_warp, now))
    }

    /// Applies the pending warp if the rate limit allows: the window's
    /// pointer moves there while the window has focus and the position is on
    /// the picture, and otherwise the window draws the cursor there itself,
    /// so that the drawn cursor always moves (Section 9.4).
    fn apply_pending_warp(&mut self, now: Instant) {
        if self
            .warp_allowed_at(now)
            .is_none_or(|allowed| now < allowed)
        {
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
        debug!(x, y, moved, "the host warped the pointer");
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
                display
                    .window
                    .set_cursor_visible(self.cursor.is_some() && !self.captured);
            }
            self.request_redraw();
        }
    }

    /// Sends a key press or release; key repeats are the host's to generate.
    fn key(&self, event: &KeyEvent) {
        if let Some(input) = key_input(event) {
            self.input.send(input);
        }
    }

    /// Sends the pointer's position on the monitor, if it is over the
    /// picture, kept within the host's confinement while it applies.
    fn pointer_moved(&self, position: PhysicalPosition<f64>) {
        let dragging = !self.held_buttons.is_empty();
        let Some((x, y)) = self.monitor_position(position) else {
            if dragging {
                debug!(
                    window_x = position.x,
                    window_y = position.y,
                    "pointer moved with a button held, but no picture is shown"
                );
            }
            return;
        };
        let (x, y) = match self.active_confinement() {
            Some(area) => confined(area, x, y),
            None => (x, y),
        };
        if dragging {
            debug!(x, y, "pointer moved with a button held");
        }
        self.input.send(UserInput::PointerMoved { x, y });
    }

    /// Returns where a position in the window falls on the host's monitor, in
    /// its pixels, or `None` while no picture is shown.
    fn monitor_position(&self, position: PhysicalPosition<f64>) -> Option<(f64, f64)> {
        let (placement, width, height) = self.placement()?;
        Some(placement.to_frame(position.x, position.y, width, height))
    }

    /// Sends a finger touching, moving or lifting. While fingers touch, the
    /// pointer events some platforms make from them are not sent.
    fn touch(&mut self, touch: Touch) {
        let phase = touch_phase(touch.phase);
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
    fn button(&mut self, button: MouseButton, state: ElementState) {
        debug!(?button, ?state, "pointer button");
        match state {
            ElementState::Pressed => self.held_buttons.insert(button),
            ElementState::Released => self.held_buttons.remove(&button),
        };
        if let Some(input) = button_input(button, state) {
            self.input.send(input);
        }
    }

    /// Sends scrolling, in 1/120 of a detent.
    fn scroll(&mut self, delta: MouseScrollDelta) {
        let scale = self
            .display
            .as_ref()
            .map_or(1.0, |display| display.window.scale_factor());
        if let Some(input) = self.scroller.scroll(delta, scale) {
            self.input.send(input);
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
            .with_inner_size(INITIAL_SIZE)
            .with_fullscreen(
                (self.presentation == args::Presentation::FullScreen)
                    .then_some(Fullscreen::Borderless(None)),
            )
            .with_visible(false);
        let window = event_loop
            .create_window(attributes)
            .map(Rc::new)
            .map_err(|error| format!("cannot open a window: {error}"))?;
        #[cfg(target_os = "macos")]
        if let Some(metal) = attach_metal(&window, self.slot.clone()) {
            return Ok(Display {
                window,
                painter: Painter::Metal(Box::new(metal)),
            });
        }
        #[cfg(windows)]
        if let Some(video) = attach_d3d11(&window, self.slot.clone()) {
            return Ok(Display {
                _pen: attach_pen(&window, self.proxy.clone()),
                window,
                painter: Painter::D3d11(Box::new(video)),
            });
        }
        let surface = Context::new(Rc::clone(&window))
            .and_then(|context| Surface::new(&context, Rc::clone(&window)))
            .map_err(|error| format!("cannot draw in the window: {error}"))?;
        Ok(Display {
            #[cfg(windows)]
            _pen: attach_pen(&window, self.proxy.clone()),
            #[cfg(target_os = "linux")]
            compositor: attach_wayland(&window),
            window,
            painter: Painter::Processor(surface),
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
            self.input.send(UserInput::Display(display_size(
                &display.window,
                self.resolution,
            )));
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
        let started = Instant::now();
        let newest = self.slot.take();
        if let Some((frame, _)) = &newest {
            let size = frame.size();
            if self.picture != Some(size) {
                self.cursor_stale = true;
            }
            self.picture = Some(size);
        }
        // A picture the GPU holds is drawn where it is, on the GPU's render
        // thread, which says when.
        #[cfg(any(target_os = "macos", windows))]
        if self.on_gpu() {
            self.show_on_gpu(newest);
            self.slot.set_display_timing(qdp_desktop::display_timing());
            return;
        }
        #[cfg(target_os = "linux")]
        let Some(newest) = self.show_on_compositor(newest, started) else {
            self.slot.set_display_timing(qdp_desktop::display_timing());
            return;
        };
        let newest = newest.map(|(frame, id)| {
            match frame.with_memory(yuv420_to_rgb) {
                Ok(rgb) => self.current = Some(rgb),
                Err(error) => warn!(%error, "cannot read the decoded picture"),
            }
            id
        });
        let presented_at = self.show();
        if let Some(id) = newest {
            self.slot.shown(id, presented_at);
            if let Some(presented_at) = presented_at {
                self.draw_stats.drew(
                    presented_at.saturating_duration_since(started),
                    Duration::ZERO,
                    presented_at,
                );
            }
        }
        self.slot.set_display_timing(qdp_desktop::display_timing());
    }

    /// Hands `newest`, a picture the GPU decoded, to the compositor to show
    /// over the window, along with the cursor where the window draws it,
    /// while the compositor shows such pictures. Returns `None` once it is
    /// shown, or the picture, for the processor to draw, otherwise.
    #[cfg(target_os = "linux")]
    fn show_on_compositor(
        &mut self,
        newest: Option<(DecodedPicture, FrameId)>,
        started: Instant,
    ) -> Option<Option<(DecodedPicture, FrameId)>> {
        let follow = self.followed_cursor();
        let placement = self.placement().map(|(placement, _, _)| placement);
        let display = self.display.as_mut()?;
        let Some(compositor) = display.compositor.as_mut() else {
            return Some(newest);
        };
        let (picture, id) = match newest {
            Some((DecodedPicture::Vaapi(picture), id)) => (Some(picture), Some(id)),
            None if self.composited.is_some() => (None, None),
            other => {
                // A picture in memory, as software decodes: the processor
                // draws it, over the window's own surface.
                if self.composited.take().is_some() {
                    compositor.hide();
                }
                return Some(other);
            }
        };
        let size = display.window.inner_size();
        // The window's own surface shows the background around the picture,
        // drawn again, before the compositor's commit, as the window's size
        // changes.
        if self.composited != Some((size.width, size.height))
            && let (Some(width), Some(height)) =
                (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        {
            let Painter::Processor(surface) = &mut display.painter;
            draw_on_processor(surface, None, (width, height), None);
            self.composited = Some((size.width, size.height));
        }
        let scene = gpu_scene((size.width, size.height), placement, follow);
        match compositor.show(picture, &scene, display.window.scale_factor()) {
            Ok(shown) => {
                if let Some(id) = id {
                    self.slot.shown(id, shown);
                }
                if let Some(shown) = shown {
                    self.draw_stats.drew(
                        shown.saturating_duration_since(started),
                        Duration::ZERO,
                        shown,
                    );
                }
            }
            Err(error) => {
                warn!(%error, "the processor draws the pictures instead of the compositor");
                display.compositor = None;
                self.composited = None;
                if let Some(id) = id {
                    self.slot.shown(id, None);
                }
                return None;
            }
        }
        None
    }

    /// Returns where the cursor the window draws goes, in the window's
    /// pixels, and its image, if the window draws one.
    fn followed_cursor(&self) -> Option<((f64, f64), ScaledCursor)> {
        match (self.follow, &self.scaled_cursor, self.placement()) {
            (Some((x, y)), Some((_, cursor)), Some((placement, width, height))) => {
                Some((placement.to_window(x, y, width, height), cursor.clone()))
            }
            _ => None,
        }
    }

    /// Draws the current picture on the processor, and the cursor where the
    /// window draws it, returning when the picture reached the screen, if it
    /// did.
    fn show(&mut self) -> Option<Instant> {
        let follow = self.followed_cursor();
        let display = self.display.as_mut()?;
        let size = display.window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            // A minimized window has nothing to draw.
            return None;
        };
        match &mut display.painter {
            Painter::Processor(surface) => {
                draw_on_processor(surface, self.current.as_ref(), (width, height), follow)
            }
            #[cfg(target_os = "macos")]
            Painter::Metal(_) => None,
            #[cfg(windows)]
            Painter::D3d11(_) => None,
        }
    }

    /// Hands `newest`, if there is a new frame, and the cursor where the
    /// window draws it, to the GPU's render thread.
    #[cfg(any(target_os = "macos", windows))]
    fn show_on_gpu(&mut self, newest: Option<(DecodedPicture, FrameId)>) {
        let follow = self.followed_cursor();
        let picture = self.placement().map(|(placement, _, _)| placement);
        let Some(display) = self.display.as_mut() else {
            return;
        };
        let size = display.window.inner_size();
        if size.width == 0 || size.height == 0 {
            // A minimized window has nothing to draw.
            if let Some((_, id)) = newest {
                self.slot.shown(id, None);
            }
            return;
        }
        let scene = gpu_scene((size.width, size.height), picture, follow);
        match &mut display.painter {
            #[cfg(target_os = "macos")]
            Painter::Metal(metal) => metal.show(newest, scene, display.window.scale_factor()),
            #[cfg(windows)]
            Painter::D3d11(video) => video.show(newest, scene),
            Painter::Processor(_) => {}
        }
    }
}

/// Draws `current`, and `follow`'s cursor at its position, into `surface`,
/// of `size` pixels, returning when the picture reached the screen, if it
/// did.
fn draw_on_processor(
    surface: &mut Surface<Rc<Window>, Rc<Window>>,
    current: Option<&RgbFrame>,
    (width, height): (NonZeroU32, NonZeroU32),
    follow: Option<((f64, f64), ScaledCursor)>,
) -> Option<Instant> {
    if let Err(error) = surface.resize(width, height) {
        warn!(%error, "cannot resize the window's surface");
        return None;
    }
    let mut buffer = match surface.buffer_mut() {
        Ok(buffer) => buffer,
        Err(error) => {
            warn!(%error, "cannot draw the window");
            return None;
        }
    };
    let (width, height) = (width.get() as usize, height.get() as usize);
    match current {
        Some(frame) => scale_into(frame, &mut buffer, width, height),
        None => buffer.fill(0),
    }
    if let Some((position, cursor)) = follow {
        cursor.draw_into(&mut buffer, width, position);
    }
    match buffer.present() {
        Ok(()) => Some(Instant::now()),
        Err(error) => {
            warn!(%error, "cannot show the window's picture");
            None
        }
    }
}

/// Returns what the GPU draws in a drawable of `size` pixels: the picture
/// at `picture`, and `follow`'s cursor at its position.
#[cfg(any(target_os = "macos", windows, target_os = "linux"))]
fn gpu_scene(
    size: (u32, u32),
    picture: Option<Placement>,
    follow: Option<((f64, f64), ScaledCursor)>,
) -> qdp_desktop::Scene {
    qdp_desktop::Scene {
        size,
        picture: picture.map(|placement| qdp_desktop::PixelRect {
            left: placement.left as f64,
            top: placement.top as f64,
            width: placement.width as f64,
            height: placement.height as f64,
        }),
        overlay: follow.map(|((x, y), cursor)| qdp_desktop::Overlay {
            width: cursor.width,
            height: cursor.height,
            left: x.round() - cursor.hotspot.0 as f64,
            top: y.round() - cursor.hotspot.1 as f64,
            rgba: cursor.rgba,
        }),
        badge: None,
    }
}

/// Draws the window's picture with Metal, telling `slot` when each frame
/// reached the screen, or returns `None`, saying why, so that the processor
/// draws it instead.
#[cfg(target_os = "macos")]
fn attach_metal(window: &Window, slot: FrameSlot) -> Option<qdp_desktop::MetalVideo<FrameId>> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    let report = Box::new(gpu_report(slot));
    // SAFETY: the view is the window's, which winit runs on the main thread,
    // and the painter is dropped before the window.
    #[allow(unsafe_code)]
    let attached =
        unsafe { qdp_desktop::MetalVideo::attach(handle.ns_view, window.scale_factor(), report) };
    match attached {
        Ok(metal) => {
            info!("drawing the video with Metal");
            Some(metal)
        }
        Err(error) => {
            warn!(%error, "drawing the video on the processor instead of with Metal");
            None
        }
    }
}

/// Has the compositor show the pictures the GPU decodes over the window, if
/// the window is a Wayland one and the compositor can, or returns `None`,
/// saying why, so that the processor draws them instead.
#[cfg(target_os = "linux")]
fn attach_wayland(window: &Window) -> Option<qdp_desktop::WaylandVideo> {
    use winit::raw_window_handle::{
        HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
    };

    let RawDisplayHandle::Wayland(display) = window.display_handle().ok()?.as_raw() else {
        return None;
    };
    let RawWindowHandle::Wayland(surface) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: the display and surface are the window's, and the presenter
    // is dropped before the window.
    #[allow(unsafe_code)]
    let attached = unsafe { qdp_desktop::WaylandVideo::attach(display.display, surface.surface) };
    match attached {
        Ok(video) => Some(video),
        Err(error) => {
            info!(%error, "the processor draws the pictures; the compositor cannot");
            None
        }
    }
}

/// Returns the report a GPU presenter gives each picture's fate: when it
/// reached the screen, for `slot`, and how long drawing it took, for the
/// statistics.
#[cfg(any(target_os = "macos", windows))]
fn gpu_report(slot: FrameSlot) -> impl FnMut(qdp_desktop::Fate<FrameId>) + Send + 'static {
    let mut stats = DrawStats::new(Instant::now());
    move |fate: qdp_desktop::Fate<FrameId>| match fate {
        qdp_desktop::Fate::Shown {
            tag,
            at,
            drew_in,
            waited,
        } => {
            slot.shown(tag, Some(at));
            stats.drew(drew_in, waited, at);
        }
        qdp_desktop::Fate::Unshown(tag) => slot.shown(tag, None),
    }
}

/// Draws the window's picture with Direct3D 11, telling `slot` when each
/// frame reached the screen, or returns `None`, saying why, so that the
/// processor draws it instead.
#[cfg(windows)]
fn attach_d3d11(window: &Window, slot: FrameSlot) -> Option<qdp_desktop::D3d11Video<FrameId>> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: the window is winit's, and the painter is dropped before it.
    #[allow(unsafe_code)]
    let attached = unsafe {
        qdp_desktop::D3d11Video::attach(
            handle.hwnd.get(),
            qdp_desktop::SyncMode::VSync,
            gpu_report(slot),
        )
    };
    match attached {
        Ok(video) => Some(video),
        Err(error) => {
            warn!(%error, "drawing the video on the processor instead of with Direct3D");
            None
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
                    let _ = sender.send(display_size(&display.window, self.resolution));
                }
                self.display = Some(display);
            }
            Err(message) => self.fail(event_loop, message),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, message: WindowMessage) {
        match message {
            WindowMessage::Frame => self.frame_ready(),
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
        let now = Instant::now();
        self.apply_pending_warp(now);
        event_loop.set_control_flow(match self.warp_allowed_at(now) {
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
                        self.held_buttons.clear();
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
                if !self.fingers.is_empty() =>
            {
                debug!("a pointer event made from touches is not sent");
            }
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
                debug!("a click captures the pointer for relative mode and is not sent");
                self.capture_pointer();
            }
            WindowEvent::MouseInput { state, button, .. } => self.button(button, state),
            WindowEvent::MouseWheel { delta, .. } => self.scroll(delta),
            WindowEvent::Focused(focused) => {
                debug!(focused, "window focus");
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
                    self.held_buttons.clear();
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

/// Returns when a warp may be applied, as of `now`, after the one applied
/// at `last`: at once if none was, and the rate limit's interval after it
/// otherwise (Section 9.4).
fn warp_due(last: Option<Instant>, now: Instant) -> Instant {
    last.map_or(now, |last| last + MIN_WARP_INTERVAL)
}

/// Creates the event loop the window runs on. It must be created on the main
/// thread.
pub fn event_loop() -> Result<EventLoop<WindowMessage>, String> {
    EventLoop::with_user_event()
        .build()
        .map_err(|error| format!("cannot start the window system: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first warp applies at once, and each after it once the rate
    /// limit's interval since the last has passed.
    #[test]
    fn warps_are_due_at_once_then_at_the_rate_limit() {
        let now = Instant::now();
        assert_eq!(warp_due(None, now), now);
        assert_eq!(warp_due(Some(now), now), now + MIN_WARP_INTERVAL);
    }
}
