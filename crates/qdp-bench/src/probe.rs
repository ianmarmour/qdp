//! The probe: a green window filling the host's screen that shows, in every
//! frame, the time it drew it, and a marker that is white while a pointer
//! button is held over it. It redraws as often as the compositor shows frames, and
//! tells the compositor that only the grid and, when it changed, the marker
//! changed, as an application drawing little would.

use std::num::NonZeroU32;
use std::rc::Rc;

use softbuffer::{Context, Rect, Surface};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Fullscreen, Window, WindowId};

use crate::clock::now_micros;
use crate::layout::{self, Area, GRID, MARKER, Pixels};

/// The colour of the background, which the measurer finds the probe by.
const GREEN: u32 = 0x00_FF_00;

/// The colour of black cells and of the dark marker.
const BLACK: u32 = 0x00_00_00;

/// The colour of white cells and of the lit marker.
const WHITE: u32 = 0xFF_FF_FF;

/// Shows the probe until its window is closed.
pub fn run() -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|error| format!("no display: {error}"))?;
    let mut probe = Probe::default();
    event_loop
        .run_app(&mut probe)
        .map_err(|error| error.to_string())?;
    probe.error.map_or(Ok(()), Err)
}

/// The probe's window and the surface it draws in.
type Display = (Rc<Window>, Surface<Rc<Window>, Rc<Window>>);

/// The probe's window and what it shows.
#[derive(Default)]
struct Probe {
    /// The window and its surface, once the event loop started.
    display: Option<Display>,
    /// Whether a pointer button is held.
    pressed: bool,
    /// Whether the marker drawn last was lit.
    shown_pressed: Option<bool>,
    /// Why the probe stopped, if it failed.
    error: Option<String>,
}

impl Probe {
    /// Opens the window and its surface.
    fn open(event_loop: &ActiveEventLoop) -> Result<Display, String> {
        let attributes = Window::default_attributes()
            .with_title("QDP benchmark probe")
            .with_decorations(false)
            .with_fullscreen(Some(Fullscreen::Borderless(None)));
        let window = event_loop
            .create_window(attributes)
            .map(Rc::new)
            .map_err(|error| format!("cannot open a window: {error}"))?;
        let surface = Context::new(Rc::clone(&window))
            .and_then(|context| Surface::new(&context, Rc::clone(&window)))
            .map_err(|error| format!("cannot draw in the window: {error}"))?;
        Ok((window, surface))
    }

    /// Draws a frame: the time now, and the marker.
    fn draw(&mut self) -> Result<(), String> {
        let Some((window, surface)) = &mut self.display else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        surface
            .resize(width, height)
            .map_err(|error| error.to_string())?;
        let (width, height) = (width.get() as usize, height.get() as usize);
        let mut buffer = surface.buffer_mut().map_err(|error| error.to_string())?;
        let fresh = buffer.age() == 0;
        if fresh {
            buffer.fill(GREEN);
        }
        let whole = Pixels {
            x: 0,
            y: 0,
            width,
            height,
        };
        let mut fill = |area: Area, colour: u32| {
            let Pixels {
                x,
                y,
                width: w,
                height: h,
            } = area.within(whole);
            for row in y..y + h {
                buffer[row * width + x..row * width + x + w].fill(colour);
            }
        };
        for (cell, white) in layout::cells(now_micros()) {
            fill(cell, if white { WHITE } else { BLACK });
        }
        // Each buffer comes back holding an older frame, so the marker is
        // drawn in every one, though the compositor is told of it only when
        // it changed.
        fill(MARKER, if self.pressed { WHITE } else { BLACK });
        let marker_changed = self.shown_pressed != Some(self.pressed);
        self.shown_pressed = Some(self.pressed);
        let damage = if fresh {
            vec![rect(whole)]
        } else if marker_changed {
            vec![rect(GRID.within(whole)), rect(MARKER.within(whole))]
        } else {
            vec![rect(GRID.within(whole))]
        };
        window.pre_present_notify();
        buffer
            .present_with_damage(&damage)
            .map_err(|error| error.to_string())?;
        window.request_redraw();
        Ok(())
    }
}

/// Returns `area` as a damaged rectangle.
fn rect(area: Pixels) -> Rect {
    let to_u32 = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
    Rect {
        x: to_u32(area.x),
        y: to_u32(area.y),
        width: NonZeroU32::new(to_u32(area.width)).unwrap_or(NonZeroU32::MIN),
        height: NonZeroU32::new(to_u32(area.height)).unwrap_or(NonZeroU32::MIN),
    }
}

impl ApplicationHandler for Probe {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.display.is_some() {
            return;
        }
        match Self::open(event_loop) {
            Ok(display) => {
                display.0.request_redraw();
                self.display = Some(display);
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::RedrawRequested => {
                if let Err(error) = self.draw() {
                    self.error = Some(error);
                    event_loop.exit();
                }
            }
            WindowEvent::MouseInput { state, .. } => {
                self.pressed = state == ElementState::Pressed;
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}
