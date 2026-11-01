//! What the desktop clients' winit windows share: the user's keys, buttons,
//! touches and scrolling as QDP's input (Section 8), and the display a
//! window shows the session on (Section 6.2).

use qdp_runtime::{DisplaySize, PointerButton, TouchPhase, UserInput};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::keyboard::PhysicalKey;
use winit::window::Window;

use crate::keymap::{HidKey, hid_key};

/// Scroll units, 1/120 of a detent, per line of scrolling (Section 8.4.2).
const UNITS_PER_LINE: f64 = 120.0;

/// Scroll units per logical pixel of precise scrolling, as from a touchpad
/// (Section 8.4.2).
const UNITS_PER_PIXEL: f64 = 3.0;

/// The step chosen scales are rounded to, in percent, as desktops offer.
const SCALE_STEP: f64 = 25.0;

/// The scales a chosen resolution may ask for, in percent.
const SCALE_RANGE: (f64, f64) = (100.0, 400.0);

/// Returns the input a key press or release sends, if its key has a usage;
/// key repeats are the host's to generate, so they send nothing.
pub fn key_input(event: &KeyEvent) -> Option<UserInput> {
    if event.repeat {
        return None;
    }
    let PhysicalKey::Code(code) = event.physical_key else {
        return None;
    };
    let pressed = event.state == ElementState::Pressed;
    Some(match hid_key(code)? {
        HidKey::Keyboard(usage) => UserInput::Key { usage, pressed },
        HidKey::Consumer(usage) => UserInput::ConsumerKey { usage, pressed },
    })
}

/// Returns the input a pointer button's press or release sends, if QDP has
/// the button.
pub fn button_input(button: MouseButton, state: ElementState) -> Option<UserInput> {
    let button = match button {
        MouseButton::Left => PointerButton::Left,
        MouseButton::Right => PointerButton::Right,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::Back => PointerButton::Back,
        MouseButton::Forward => PointerButton::Forward,
        MouseButton::Other(_) => return None,
    };
    Some(UserInput::Button {
        button,
        pressed: state == ElementState::Pressed,
    })
}

/// Returns a touch's phase as QDP names it.
pub fn touch_phase(phase: winit::event::TouchPhase) -> TouchPhase {
    match phase {
        winit::event::TouchPhase::Started => TouchPhase::Down,
        winit::event::TouchPhase::Moved => TouchPhase::Move,
        winit::event::TouchPhase::Ended => TouchPhase::Up,
        winit::event::TouchPhase::Cancelled => TouchPhase::Cancel,
    }
}

/// Converts scrolling to whole units of 1/120 of a detent, keeping what is
/// left below a unit for the next.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Scroller {
    /// Horizontal and vertical scrolling not yet sent, below one unit.
    remainder: (f64, f64),
}

impl Scroller {
    /// Returns the input scrolling by `delta` sends, if it reaches a whole
    /// unit, in a window `scale_factor` times its logical size. The window
    /// system's deltas already follow the user's scrolling direction
    /// preference.
    pub fn scroll(&mut self, delta: MouseScrollDelta, scale_factor: f64) -> Option<UserInput> {
        let (horizontal, vertical) = match delta {
            // Positive deltas move the content right and down: scrolling left
            // and up, away from the user.
            MouseScrollDelta::LineDelta(x, y) => (
                -f64::from(x) * UNITS_PER_LINE,
                f64::from(y) * UNITS_PER_LINE,
            ),
            MouseScrollDelta::PixelDelta(position) => (
                -position.x / scale_factor * UNITS_PER_PIXEL,
                position.y / scale_factor * UNITS_PER_PIXEL,
            ),
        };
        let horizontal = self.remainder.0 + horizontal;
        let vertical = self.remainder.1 + vertical;
        let (whole_horizontal, whole_vertical) = (horizontal.trunc(), vertical.trunc());
        self.remainder = (horizontal - whole_horizontal, vertical - whole_vertical);
        (whole_horizontal != 0.0 || whole_vertical != 0.0).then_some(UserInput::Scroll {
            vertical: whole_vertical as i32,
            horizontal: whole_horizontal as i32,
        })
    }
}

/// Returns the display the session is shown on: the window's content area,
/// or the `resolution` the user chose instead, at the refresh rate of the
/// monitor it is on (Section 6.2).
pub fn display_size(window: &Window, resolution: Option<(u32, u32)>) -> DisplaySize {
    let monitor = window
        .current_monitor()
        .or_else(|| window.primary_monitor());
    let size = window.inner_size();
    let window_scale = (window.scale_factor() * f64::from(DisplaySize::UNSCALED)).round() as u32;
    let (width, height, scale_percent) = match resolution {
        Some((width, height)) => (
            width,
            height,
            stretched_scale(window_scale, (size.width, size.height), (width, height)),
        ),
        None => (size.width, size.height, window_scale),
    };
    DisplaySize {
        width,
        height,
        refresh_millihertz: monitor
            .and_then(|monitor| monitor.refresh_rate_millihertz())
            .unwrap_or(0),
        scale_percent,
    }
}

/// Returns the interface scale to ask the host for when the picture of
/// `resolution` is stretched to fill a `window`-sized window at
/// `window_scale` percent: smaller by as much as the picture is stretched,
/// so that the host's text shows at the size the window's would, rounded to
/// the scales desktops offer (Section 6.2).
fn stretched_scale(window_scale: u32, window: (u32, u32), resolution: (u32, u32)) -> u32 {
    let stretch = (f64::from(window.0) / f64::from(resolution.0.max(1)))
        .min(f64::from(window.1) / f64::from(resolution.1.max(1)));
    if stretch <= 0.0 {
        return window_scale;
    }
    let scale = f64::from(window_scale) / stretch;
    ((scale / SCALE_STEP).round() * SCALE_STEP).clamp(SCALE_RANGE.0, SCALE_RANGE.1) as u32
}

#[cfg(test)]
mod tests {
    use winit::dpi::PhysicalPosition;

    use super::*;

    /// A chosen resolution stretched over the window asks for a scale as
    /// much smaller, in steps of 25 %, and never below 100 %.
    #[test]
    fn scales_chosen_resolutions() {
        // 2560 wide in a 3024-pixel window at 200 %: about 169 %.
        assert_eq!(stretched_scale(200, (3024, 1964), (2560, 1080)), 175);
        // The window's own size keeps its scale.
        assert_eq!(stretched_scale(200, (2560, 1080), (2560, 1080)), 200);
        // Stretched a lot, the scale stays at 100 %.
        assert_eq!(stretched_scale(100, (3840, 2160), (1280, 720)), 100);
        // Shrunk, larger.
        assert_eq!(stretched_scale(100, (1920, 1080), (3840, 2160)), 200);
        // An empty window keeps the window's scale.
        assert_eq!(stretched_scale(200, (0, 0), (2560, 1080)), 200);
    }

    /// Lines scroll by detents; precise scrolling adds up to whole units,
    /// keeping the rest for the next.
    #[test]
    fn scrolls_in_whole_units() {
        let mut scroller = Scroller::default();
        assert_eq!(
            scroller.scroll(MouseScrollDelta::LineDelta(0.0, 1.0), 1.0),
            Some(UserInput::Scroll {
                vertical: 120,
                horizontal: 0
            })
        );
        assert_eq!(
            scroller.scroll(MouseScrollDelta::LineDelta(1.0, 0.0), 1.0),
            Some(UserInput::Scroll {
                vertical: 0,
                horizontal: -120
            })
        );
        // A fifth of a logical pixel at 200 %: 0.3 units, kept.
        let fifth = MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.2));
        assert_eq!(scroller.scroll(fifth, 2.0), None);
        assert_eq!(scroller.scroll(fifth, 2.0), None);
        assert_eq!(scroller.scroll(fifth, 2.0), None);
        // 1.2 units in all.
        assert_eq!(
            scroller.scroll(fifth, 2.0),
            Some(UserInput::Scroll {
                vertical: 1,
                horizontal: 0
            })
        );
    }

    /// The buttons QDP has are sent; others are not.
    #[test]
    fn maps_buttons() {
        assert_eq!(
            button_input(MouseButton::Back, ElementState::Pressed),
            Some(UserInput::Button {
                button: PointerButton::Back,
                pressed: true
            })
        );
        assert_eq!(
            button_input(MouseButton::Other(9), ElementState::Pressed),
            None
        );
    }
}
