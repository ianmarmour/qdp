//! The on-screen gamepad the phone and tablet apps can show over the
//! stream, for games on a device without a controller, laid out within
//! reach of the thumbs: a stick in each bottom corner, A, B, X and Y on an
//! arc around the right one, each side's trigger and shoulder button as a
//! pill in its top corner, and Select and Start between them. The left
//! stick stands in for the directional pad, which the default layout
//! leaves out, as it does the sticks' buttons.
//!
//! The apps draw the controls [`VirtualGamepad::controls`] returns and pass
//! every touch on them to [`VirtualGamepad::touch`], which works out the
//! gamepad's state, for the app to send to the host as a gamepad of its own,
//! in [`SLOT`]. Each touch takes the control it lands on until it lifts:
//! several thumbs hold several controls at once, and a touch that lands on
//! none does nothing.
//!
//! Lengths are in the units of the app's touches, Android's pixels or
//! UIKit's points, and [`Screen::density`] says how many make a
//! density-independent pixel, in which the layout is designed. On a screen
//! smaller than the layout is designed for, it scales down to fit.

use qdp_runtime::TouchPhase;

use crate::gamepad::GamepadState;

/// The slot the on-screen gamepad connects in, far above those of the
/// device's own controllers, which the apps number from 0.
pub const SLOT: u32 = 1000;

/// The gamepad's name, as the host is told it.
pub const NAME: &str = "QDP on-screen controller";

/// [`TouchOutcome::flags`]' bit saying the controls look different.
pub const REDRAW: u32 = 1 << 0;

/// [`TouchOutcome::flags`]' bit saying a button or direction was pressed.
pub const PRESSED: u32 = 1 << 1;

/// [`ControlView::directions`]'s bit for up.
pub const UP: u32 = 1 << 0;

/// [`ControlView::directions`]'s bit for down.
pub const DOWN: u32 = 1 << 1;

/// [`ControlView::directions`]'s bit for left.
pub const LEFT: u32 = 1 << 2;

/// [`ControlView::directions`]'s bit for right.
pub const RIGHT: u32 = 1 << 3;

/// The space between the sticks and the safe area's sides and bottom, and
/// between the shoulder pills and its sides, in dp.
const MARGIN: f32 = 20.0;

/// The space between the top row, the shoulder pills, Select and Start, and
/// the top of the safe area, in dp: what the app's own menu button between
/// Select and Start keeps.
const TOP_MARGIN: f32 = 12.0;

/// The radius of a stick's base, in dp.
const STICK_RADIUS: f32 = 56.0;

/// The radius of A, B, X and Y, in dp: 52 dp across, more than the 48 a
/// touch target should be.
const FACE_BUTTON_RADIUS: f32 = 26.0;

/// The least space between neighbouring controls, in dp.
const CLEARANCE: f32 = 12.0;

/// How far A, B, X and Y's centres are from the right stick's, on an arc
/// around it, in dp.
const FACE_RING: f32 = STICK_RADIUS + CLEARANCE + FACE_BUTTON_RADIUS;

/// Where A, X, Y and B are on their arc around the right stick, in degrees
/// counter-clockwise from its right: round its top left, from below its
/// left to straight above it, in the order a controller's diamond has them
/// going round. 38° apart, neighbours keep clear of each other.
const FACE_ANGLES: [(Control, f32); 4] = [
    (Control::A, 204.0),
    (Control::X, 166.0),
    (Control::Y, 128.0),
    (Control::B, 90.0),
];

/// The width of each half of a shoulder pill, a trigger and its shoulder
/// button side by side, in dp.
const SHOULDER_WIDTH: f32 = 64.0;

/// A shoulder pill's height, in dp.
const SHOULDER_HEIGHT: f32 = 44.0;

/// The radius of Select and Start, in dp: 44 across, the least a touch
/// target should be.
const SMALL_BUTTON_RADIUS: f32 = 22.0;

/// The space between Select and Start, in dp, which leaves room between
/// them for the app's own button that opens the stream's menu.
const MIDDLE_GAP: f32 = 80.0;

/// From the safe area's side and bottom edges to each stick's centre, in
/// its bottom corner, in dp.
const STICK_INSET: f32 = MARGIN + STICK_RADIUS;

/// How far beyond a button or the directional pad a touch still takes it,
/// in dp.
const HIT_SLOP: f32 = 8.0;

/// How far from a stick's centre a touch still takes it, as a share of its
/// radius: wider than the stick, as thumbs land on it without looking.
const STICK_REACH: f32 = 1.4;

/// How far from the directional pad's centre a touch presses a direction,
/// as a share of half its size.
const DPAD_DEAD_ZONE: f32 = 0.2;

/// tan 22.5°: a touch whose other axis is at least this share of the main
/// one presses a diagonal, so that the eight directions each take 45°.
const DIAGONAL_RATIO: f32 = 0.414_213_57;

/// A control of the on-screen gamepad. A button's value is its Standard
/// Gamepad index, and the directional pad's that of its first button, up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum Control {
    /// The bottom face button.
    A = 0,
    /// The right face button.
    B = 1,
    /// The left face button.
    X = 2,
    /// The top face button.
    Y = 3,
    /// The left shoulder button.
    LeftShoulder = 4,
    /// The right shoulder button.
    RightShoulder = 5,
    /// The left trigger, pulled all the way while held.
    LeftTrigger = 6,
    /// The right trigger, likewise.
    RightTrigger = 7,
    /// Select, also known as View or Back.
    Select = 8,
    /// Start, also known as Menu.
    Start = 9,
    /// The left stick's button.
    LeftStickButton = 10,
    /// The right stick's button.
    RightStickButton = 11,
    /// The directional pad, Standard Gamepad buttons 12 to 15.
    DPad = 12,
    /// The left stick.
    LeftStick = 17,
    /// The right stick.
    RightStick = 18,
}

impl Control {
    /// Returns the number the apps know the control by: its value.
    pub fn code(self) -> u32 {
        self as u32
    }

    /// Returns whether the control is a stick.
    fn is_stick(self) -> bool {
        matches!(self, Self::LeftStick | Self::RightStick)
    }
}

/// A point, in the units of the app's touches.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    /// Its distance from the screen's left edge.
    pub x: f32,
    /// Its distance from the screen's top edge.
    pub y: f32,
}

/// A rectangle, in the units of the app's touches.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    /// Its left edge.
    pub x: f32,
    /// Its top edge.
    pub y: f32,
    /// Its width.
    pub width: f32,
    /// Its height.
    pub height: f32,
}

impl Rect {
    /// Returns the square of side `2 * radius` around `center`.
    fn around(center: Point, radius: f32) -> Self {
        Self {
            x: center.x - radius,
            y: center.y - radius,
            width: 2.0 * radius,
            height: 2.0 * radius,
        }
    }

    /// Returns its centre.
    fn center(&self) -> Point {
        Point {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }

    /// Returns whether `point` is inside it or within `slop` of it.
    fn reaches(&self, point: Point, slop: f32) -> bool {
        point.x >= self.x - slop
            && point.x <= self.x + self.width + slop
            && point.y >= self.y - slop
            && point.y <= self.y + self.height + slop
    }

    /// Returns whether it and `other` overlap.
    #[cfg(test)]
    fn overlaps(&self, other: &Self) -> bool {
        self.x < other.x + other.width
            && other.x < self.x + self.width
            && self.y < other.y + other.height
            && other.y < self.y + self.height
    }
}

/// The insets of a screen's safe area, which the controls stay inside: its
/// display cutout, rounded corners and the system's edge gestures.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Insets {
    /// From the left edge.
    pub left: f32,
    /// From the top edge.
    pub top: f32,
    /// From the right edge.
    pub right: f32,
    /// From the bottom edge.
    pub bottom: f32,
}

/// The screen the gamepad is laid out on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Screen {
    /// Its width.
    pub width: f32,
    /// Its height.
    pub height: f32,
    /// How many units make a density-independent pixel: Android's density,
    /// or 1 for UIKit's points.
    pub density: f32,
    /// Its safe area's insets.
    pub insets: Insets,
}

/// A control as the app draws it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlView {
    /// Which control it is.
    pub control: Control,
    /// Where it is: a circle's bounding square for the sticks and the round
    /// buttons, the rectangle of a shoulder button or trigger, and the
    /// directional pad's bounding square.
    pub frame: Rect,
    /// Whether a touch holds it.
    pub held: bool,
    /// A stick's tilt, each axis from -1 to 1, negative left and up; none
    /// for the other controls.
    pub tilt: (f32, f32),
    /// The directional pad's directions held, of [`UP`], [`DOWN`], [`LEFT`]
    /// and [`RIGHT`]; none for the other controls.
    pub directions: u32,
}

/// What a touch changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TouchOutcome {
    /// Whether the gamepad's state changed, which the app sends the host.
    pub state_changed: bool,
    /// Whether the controls look different, which the app draws again.
    pub redraw: bool,
    /// Whether a button or direction was pressed, for the app to tap the
    /// device's haptics as a real button would click.
    pub pressed: bool,
}

impl TouchOutcome {
    /// Returns the outcome as the apps' bindings return it: [`REDRAW`] and
    /// [`PRESSED`] bits.
    pub fn flags(self) -> u32 {
        let bit = |set: bool, bit: u32| if set { bit } else { 0 };
        bit(self.redraw, REDRAW) | bit(self.pressed, PRESSED)
    }
}

/// A control placed on the screen.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placed {
    /// Which control it is.
    control: Control,
    /// Where it is, as [`ControlView::frame`].
    frame: Rect,
}

/// A touch holding a control.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Finger {
    /// The app's number for the touch.
    id: u32,
    /// The control it holds.
    control: Control,
    /// Where it touched down: a stick tilts from there.
    origin: Point,
    /// Where it is now.
    at: Point,
}

/// The on-screen gamepad: its controls' places, the touches holding them,
/// and the state they make.
#[derive(Debug, Clone, Default)]
pub struct VirtualGamepad {
    /// The controls, once laid out.
    controls: Vec<Placed>,
    /// The touches holding controls.
    fingers: Vec<Finger>,
    /// Units per dp at the layout's scale.
    unit: f32,
}

impl VirtualGamepad {
    /// Returns a gamepad not laid out yet, with no controls.
    pub const fn new() -> Self {
        Self {
            controls: Vec::new(),
            fingers: Vec::new(),
            unit: 0.0,
        }
    }

    /// Lays the controls out on `screen`, releasing every control held, as
    /// they move. Returns whether that changed the gamepad's state.
    pub fn lay_out(&mut self, screen: &Screen) -> bool {
        let before = self.state();
        self.fingers.clear();
        self.controls.clear();
        let left = screen.insets.left;
        let top = screen.insets.top;
        let right = screen.width - screen.insets.right;
        let bottom = screen.height - screen.insets.bottom;
        let density = screen.density;
        if right <= left || bottom <= top || density <= 0.0 {
            self.unit = 0.0;
            return before != self.state();
        }
        let (design_width, design_height) = design_size();
        let scale = ((right - left) / density / design_width)
            .min((bottom - top) / density / design_height)
            .min(1.0);
        let unit = scale * density;
        self.unit = unit;
        // Points measured in dp from the safe area's left or right edge, and
        // from its top or bottom edge.
        let from_left = |x: f32| left + x * unit;
        let from_right = |x: f32| right - x * unit;
        let from_top = |y: f32| top + y * unit;
        let from_bottom = |y: f32| bottom - y * unit;
        let middle = (left + right) / 2.0;
        let circle = |control, x, y, radius: f32| Placed {
            control,
            frame: Rect::around(Point { x, y }, radius * unit),
        };
        let shoulder = |control, x: f32| Placed {
            control,
            frame: Rect {
                x,
                y: from_top(TOP_MARGIN),
                width: SHOULDER_WIDTH * unit,
                height: SHOULDER_HEIGHT * unit,
            },
        };
        let middle_offset = (MIDDLE_GAP / 2.0 + SMALL_BUTTON_RADIUS) * unit;
        let top_row = from_top(TOP_MARGIN + SMALL_BUTTON_RADIUS);
        let left_stick = Point {
            x: from_left(STICK_INSET),
            y: from_bottom(STICK_INSET),
        };
        let right_stick = Point {
            x: from_right(STICK_INSET),
            y: from_bottom(STICK_INSET),
        };
        let mut controls = vec![
            // Each side's trigger outermost and its shoulder button beside
            // it, one pill in each top corner.
            shoulder(Control::LeftTrigger, from_left(MARGIN)),
            shoulder(Control::LeftShoulder, from_left(MARGIN + SHOULDER_WIDTH)),
            shoulder(
                Control::RightShoulder,
                from_right(MARGIN + 2.0 * SHOULDER_WIDTH),
            ),
            shoulder(Control::RightTrigger, from_right(MARGIN + SHOULDER_WIDTH)),
            circle(
                Control::Select,
                middle - middle_offset,
                top_row,
                SMALL_BUTTON_RADIUS,
            ),
            circle(
                Control::Start,
                middle + middle_offset,
                top_row,
                SMALL_BUTTON_RADIUS,
            ),
            circle(Control::LeftStick, left_stick.x, left_stick.y, STICK_RADIUS),
            circle(
                Control::RightStick,
                right_stick.x,
                right_stick.y,
                STICK_RADIUS,
            ),
        ];
        controls.extend(FACE_ANGLES.iter().map(|&(control, angle)| {
            let (sin, cos) = angle.to_radians().sin_cos();
            circle(
                control,
                right_stick.x + cos * FACE_RING * unit,
                right_stick.y - sin * FACE_RING * unit,
                FACE_BUTTON_RADIUS,
            )
        }));
        self.controls = controls;
        before != self.state()
    }

    /// Takes touch `id`'s `phase` at `at`.
    pub fn touch(&mut self, id: u32, phase: TouchPhase, at: Point) -> TouchOutcome {
        let before = self.state();
        let held_before = self.held_controls();
        match phase {
            TouchPhase::Down => {
                self.fingers.retain(|finger| finger.id != id);
                // A stick follows one touch at a time.
                if let Some(control) = self.control_at(at).filter(|control| {
                    !control.is_stick()
                        || !self.fingers.iter().any(|finger| finger.control == *control)
                }) {
                    self.fingers.push(Finger {
                        id,
                        control,
                        origin: at,
                        at,
                    });
                }
            }
            TouchPhase::Move => {
                if let Some(finger) = self.fingers.iter_mut().find(|finger| finger.id == id) {
                    finger.at = at;
                }
            }
            TouchPhase::Up | TouchPhase::Cancel => self.fingers.retain(|finger| finger.id != id),
        }
        let after = self.state();
        let state_changed = after != before;
        TouchOutcome {
            state_changed,
            redraw: state_changed || self.held_controls() != held_before,
            pressed: after.buttons & !before.buttons != 0,
        }
    }

    /// Releases every control held, as when the stream's menu opens.
    /// Returns whether that changed the gamepad's state.
    pub fn release_all(&mut self) -> bool {
        let before = self.state();
        self.fingers.clear();
        before != self.state()
    }

    /// Returns the gamepad's state, as the controls held make it.
    pub fn state(&self) -> GamepadState {
        let mut state = GamepadState {
            buttons: 0,
            left_stick: (0.0, 0.0),
            right_stick: (0.0, 0.0),
            left_trigger: 0.0,
            right_trigger: 0.0,
        };
        for finger in &self.fingers {
            match finger.control {
                Control::LeftStick => state.left_stick = self.tilt(finger),
                Control::RightStick => state.right_stick = self.tilt(finger),
                Control::DPad => {
                    state.buttons |= self.directions(finger) << Control::DPad.code();
                }
                button => {
                    state.buttons |= 1 << button.code();
                    match button {
                        Control::LeftTrigger => state.left_trigger = 1.0,
                        Control::RightTrigger => state.right_trigger = 1.0,
                        _ => {}
                    }
                }
            }
        }
        state
    }

    /// Returns the controls as the app draws them.
    pub fn controls(&self) -> Vec<ControlView> {
        self.controls
            .iter()
            .map(|placed| {
                let finger = self
                    .fingers
                    .iter()
                    .find(|finger| finger.control == placed.control);
                ControlView {
                    control: placed.control,
                    frame: placed.frame,
                    held: finger.is_some(),
                    tilt: finger
                        .filter(|finger| finger.control.is_stick())
                        .map_or((0.0, 0.0), |finger| self.tilt(finger)),
                    directions: finger
                        .filter(|finger| finger.control == Control::DPad)
                        .map_or(0, |finger| self.directions(finger)),
                }
            })
            .collect()
    }

    /// Returns the control a touch at `at` takes, if any: the buttons, the
    /// smallest targets, before the directional pad, and that before the
    /// sticks, whose reach is widest.
    fn control_at(&self, at: Point) -> Option<Control> {
        let slop = HIT_SLOP * self.unit;
        let reaches = |placed: &&Placed| {
            let frame = placed.frame;
            match placed.control {
                Control::LeftStick | Control::RightStick => {
                    distance(frame.center(), at) <= frame.width / 2.0 * STICK_REACH
                }
                Control::DPad
                | Control::LeftShoulder
                | Control::RightShoulder
                | Control::LeftTrigger
                | Control::RightTrigger => frame.reaches(at, slop),
                _ => distance(frame.center(), at) <= frame.width / 2.0 + slop,
            }
        };
        let rank = |control: Control| match control {
            Control::LeftStick | Control::RightStick => 2,
            Control::DPad => 1,
            _ => 0,
        };
        // Among controls alike, as a trigger and the shoulder button beside
        // it, the nearest.
        let nearness = |placed: &Placed| distance(placed.frame.center(), at);
        self.controls
            .iter()
            .filter(reaches)
            .min_by(|a, b| {
                rank(a.control)
                    .cmp(&rank(b.control))
                    .then_with(|| nearness(a).total_cmp(&nearness(b)))
            })
            .map(|placed| placed.control)
    }

    /// Returns the tilt of the stick `finger` holds: how far it moved from
    /// where it touched down, as a share of the stick's radius, at most 1.
    fn tilt(&self, finger: &Finger) -> (f32, f32) {
        let radius = STICK_RADIUS * self.unit;
        if radius <= 0.0 {
            return (0.0, 0.0);
        }
        let x = (finger.at.x - finger.origin.x) / radius;
        let y = (finger.at.y - finger.origin.y) / radius;
        let length = x.hypot(y);
        if length > 1.0 {
            (x / length, y / length)
        } else {
            (x, y)
        }
    }

    /// Returns the directions `finger` presses on the directional pad, by
    /// where it is from the pad's centre: one of eight, 45° each, or none
    /// near the centre.
    fn directions(&self, finger: &Finger) -> u32 {
        let Some(pad) = self
            .controls
            .iter()
            .find(|placed| placed.control == Control::DPad)
        else {
            return 0;
        };
        let center = pad.frame.center();
        let dx = finger.at.x - center.x;
        let dy = finger.at.y - center.y;
        if dx.hypot(dy) < pad.frame.width / 2.0 * DPAD_DEAD_ZONE {
            return 0;
        }
        let horizontal = dx.abs() >= dy.abs() * DIAGONAL_RATIO;
        let vertical = dy.abs() >= dx.abs() * DIAGONAL_RATIO;
        let mut directions = 0;
        if vertical {
            directions |= if dy < 0.0 { UP } else { DOWN };
        }
        if horizontal {
            directions |= if dx < 0.0 { LEFT } else { RIGHT };
        }
        directions
    }

    /// Returns the controls held, in their order.
    fn held_controls(&self) -> Vec<Control> {
        self.controls
            .iter()
            .map(|placed| placed.control)
            .filter(|control| self.fingers.iter().any(|finger| finger.control == *control))
            .collect()
    }
}

/// Returns the width and height, in dp, the layout is designed for: the
/// widest of its top row and its sticks' row, and the sticks' row and the
/// top row stacked. Narrower or shorter safe areas scale it down.
fn design_size() -> (f32, f32) {
    // How far A, B, X and Y reach left of the right stick's centre, and
    // above it.
    let (reach_left, reach_up) =
        FACE_ANGLES
            .iter()
            .fold((0.0_f32, 0.0_f32), |(left, up), &(_, angle)| {
                let (sin, cos) = angle.to_radians().sin_cos();
                (
                    left.max(-cos * FACE_RING + FACE_BUTTON_RADIUS),
                    up.max(sin * FACE_RING + FACE_BUTTON_RADIUS),
                )
            });
    let sticks_row = 2.0 * STICK_INSET + STICK_RADIUS + CLEARANCE + reach_left;
    let top_row = 2.0
        * (MARGIN
            + 2.0 * SHOULDER_WIDTH
            + CLEARANCE
            + MIDDLE_GAP / 2.0
            + 2.0 * SMALL_BUTTON_RADIUS);
    let height =
        STICK_INSET + reach_up.max(STICK_RADIUS) + CLEARANCE + TOP_MARGIN + SHOULDER_HEIGHT;
    (sticks_row.max(top_row), height)
}

/// Returns the distance between `a` and `b`.
fn distance(a: Point, b: Point) -> f32 {
    (a.x - b.x).hypot(a.y - b.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A phone in landscape: 2400 × 1080 pixels at 3 pixels a dp, with its
    /// camera cutout on the left and edge gestures on both sides.
    const PHONE: Screen = Screen {
        width: 2400.0,
        height: 1080.0,
        density: 3.0,
        insets: Insets {
            left: 120.0,
            top: 0.0,
            right: 72.0,
            bottom: 72.0,
        },
    };

    /// The screens the layout must fit: the phone, a 16:9 phone, an iPad in
    /// landscape and an iPhone in portrait.
    const SCREENS: [Screen; 4] = [
        PHONE,
        Screen {
            width: 1920.0,
            height: 1080.0,
            density: 3.0,
            insets: Insets {
                left: 0.0,
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
            },
        },
        Screen {
            width: 1180.0,
            height: 820.0,
            density: 1.0,
            insets: Insets {
                left: 0.0,
                top: 24.0,
                right: 0.0,
                bottom: 20.0,
            },
        },
        Screen {
            width: 393.0,
            height: 852.0,
            density: 1.0,
            insets: Insets {
                left: 0.0,
                top: 59.0,
                right: 0.0,
                bottom: 34.0,
            },
        },
    ];

    /// Returns a gamepad laid out on the phone.
    fn phone() -> VirtualGamepad {
        let mut gamepad = VirtualGamepad::new();
        gamepad.lay_out(&PHONE);
        gamepad
    }

    /// Returns the centre of `control` on `gamepad`.
    fn center(gamepad: &VirtualGamepad, control: Control) -> Point {
        gamepad
            .controls()
            .iter()
            .find(|view| view.control == control)
            .map(|view| view.frame.center())
            .unwrap()
    }

    /// Returns the radius of `control`'s frame on `gamepad`.
    fn radius(gamepad: &VirtualGamepad, control: Control) -> f32 {
        gamepad
            .controls()
            .iter()
            .find(|view| view.control == control)
            .map(|view| view.frame.width / 2.0)
            .unwrap()
    }

    /// Returns `point` moved by `dx`, `dy`.
    fn moved(point: Point, dx: f32, dy: f32) -> Point {
        Point {
            x: point.x + dx,
            y: point.y + dy,
        }
    }

    /// Every control fits in the safe area without overlapping another, on
    /// phones and tablets in either orientation, and keeps its designed
    /// size where there is room.
    #[test]
    fn lays_out_every_control_inside_the_safe_area() {
        for screen in SCREENS {
            let mut gamepad = VirtualGamepad::new();
            gamepad.lay_out(&screen);
            let views = gamepad.controls();
            assert_eq!(views.len(), 12);
            for view in &views {
                let frame = view.frame;
                assert!(frame.width > 0.0, "{screen:?}: {view:?}");
                assert!(frame.x >= screen.insets.left, "{screen:?}: {view:?}");
                assert!(frame.y >= screen.insets.top, "{screen:?}: {view:?}");
                assert!(
                    frame.x + frame.width <= screen.width - screen.insets.right,
                    "{screen:?}: {view:?}"
                );
                assert!(
                    frame.y + frame.height <= screen.height - screen.insets.bottom,
                    "{screen:?}: {view:?}"
                );
            }
            // Round controls keep clear of each other as circles; the
            // directional pad, shoulder buttons and triggers as rectangles.
            let round = |view: &ControlView| {
                !matches!(
                    view.control,
                    Control::DPad
                        | Control::LeftShoulder
                        | Control::RightShoulder
                        | Control::LeftTrigger
                        | Control::RightTrigger
                )
            };
            for (index, a) in views.iter().enumerate() {
                for b in &views[index + 1..] {
                    let clear = if round(a) && round(b) {
                        distance(a.frame.center(), b.frame.center())
                            >= (a.frame.width + b.frame.width) / 2.0
                    } else {
                        !a.frame.overlaps(&b.frame)
                    };
                    assert!(clear, "{screen:?}: {a:?} and {b:?}");
                }
            }
        }
        let mut tablet = VirtualGamepad::new();
        tablet.lay_out(&SCREENS[2]);
        let stick = tablet
            .controls()
            .into_iter()
            .find(|view| view.control == Control::LeftStick)
            .unwrap();
        assert_eq!(stick.frame.width, 2.0 * STICK_RADIUS);
    }

    /// The sticks are in the bottom corners, A, B, X and Y around the right
    /// one, each side's trigger and shoulder button side by side in its top
    /// corner, the trigger outermost, and a touch on either half of a pill
    /// takes that half.
    #[test]
    fn lays_out_the_default_layout() {
        let gamepad = phone();
        let left = center(&gamepad, Control::LeftStick);
        let right = center(&gamepad, Control::RightStick);
        assert!(left.x < PHONE.width / 3.0 && left.y > PHONE.height / 2.0);
        assert!(right.x > PHONE.width * 2.0 / 3.0 && right.y > PHONE.height / 2.0);
        for face in [Control::A, Control::B, Control::X, Control::Y] {
            let ring = distance(center(&gamepad, face), right);
            assert!((ring - FACE_RING * gamepad.unit).abs() < 0.01, "{face:?}");
        }
        let views = gamepad.controls();
        let frame = |control| {
            views
                .iter()
                .find(|view| view.control == control)
                .unwrap()
                .frame
        };
        assert!(frame(Control::LeftTrigger).x < frame(Control::LeftShoulder).x);
        assert!(frame(Control::RightShoulder).x < frame(Control::RightTrigger).x);
        assert_eq!(
            frame(Control::LeftTrigger).x + frame(Control::LeftTrigger).width,
            frame(Control::LeftShoulder).x
        );
        for absent in [
            Control::DPad,
            Control::LeftStickButton,
            Control::RightStickButton,
        ] {
            assert!(
                views.iter().all(|view| view.control != absent),
                "{absent:?}"
            );
        }
        let mut gamepad = gamepad;
        let bumper = frame(Control::LeftShoulder);
        let near_seam = Point {
            x: bumper.x + 2.0,
            y: bumper.y + bumper.height / 2.0,
        };
        gamepad.touch(1, TouchPhase::Down, near_seam);
        assert_eq!(gamepad.state().buttons, 1 << Control::LeftShoulder.code());
    }

    /// A button holds while touched, and says it was pressed.
    #[test]
    fn presses_buttons_while_touched() {
        let mut gamepad = phone();
        let outcome = gamepad.touch(1, TouchPhase::Down, center(&gamepad, Control::A));
        assert_eq!(
            outcome,
            TouchOutcome {
                state_changed: true,
                redraw: true,
                pressed: true
            }
        );
        assert_eq!(gamepad.state().buttons, 1 << Control::A.code());
        let outcome = gamepad.touch(1, TouchPhase::Up, center(&gamepad, Control::A));
        assert!(outcome.state_changed && !outcome.pressed);
        assert_eq!(gamepad.state().buttons, 0);
    }

    /// Several thumbs hold several controls at once.
    #[test]
    fn holds_several_controls_at_once() {
        let mut gamepad = phone();
        let stick = center(&gamepad, Control::LeftStick);
        gamepad.touch(1, TouchPhase::Down, stick);
        gamepad.touch(2, TouchPhase::Down, center(&gamepad, Control::RightTrigger));
        gamepad.touch(3, TouchPhase::Down, center(&gamepad, Control::B));
        let half = radius(&gamepad, Control::LeftStick) / 2.0;
        gamepad.touch(1, TouchPhase::Move, moved(stick, half, 0.0));
        let state = gamepad.state();
        assert_eq!(
            state.buttons,
            1 << Control::RightTrigger.code() | 1 << Control::B.code()
        );
        assert_eq!(state.right_trigger, 1.0);
        assert_eq!(state.left_stick, (0.5, 0.0));
        let held: Vec<_> = gamepad
            .controls()
            .into_iter()
            .filter(|view| view.held)
            .map(|view| view.control)
            .collect();
        assert_eq!(
            held,
            [Control::RightTrigger, Control::LeftStick, Control::B]
        );
    }

    /// A stick tilts from where the thumb touched down, at most all the
    /// way, and one touch at a time moves it.
    #[test]
    fn tilts_sticks_from_where_they_were_touched() {
        let mut gamepad = phone();
        // Off the stick's centre, the touch is still at rest.
        let down = moved(center(&gamepad, Control::RightStick), 30.0, 0.0);
        let outcome = gamepad.touch(7, TouchPhase::Down, down);
        assert!(!outcome.state_changed && outcome.redraw);
        assert_eq!(gamepad.state().right_stick, (0.0, 0.0));
        let half = radius(&gamepad, Control::RightStick) / 2.0;
        gamepad.touch(7, TouchPhase::Move, moved(down, 0.0, -half));
        assert_eq!(gamepad.state().right_stick, (0.0, -0.5));
        gamepad.touch(7, TouchPhase::Move, moved(down, 1000.0, 0.0));
        assert_eq!(gamepad.state().right_stick, (1.0, 0.0));
        // A second thumb on the stick does not take it.
        let outcome = gamepad.touch(8, TouchPhase::Down, down);
        assert!(!outcome.redraw);
        gamepad.touch(8, TouchPhase::Move, moved(down, -1000.0, 0.0));
        assert_eq!(gamepad.state().right_stick, (1.0, 0.0));
        gamepad.touch(7, TouchPhase::Cancel, down);
        assert_eq!(gamepad.state().right_stick, (0.0, 0.0));
    }

    /// The directional pad presses the direction a thumb is in from its
    /// centre, diagonals included, and nothing at its centre, following the
    /// thumb as it rolls.
    #[test]
    fn presses_directions_as_the_thumb_rolls() {
        // The default layout leaves the pad out; a layout of it alone.
        let pad = Point { x: 300.0, y: 300.0 };
        let mut gamepad = VirtualGamepad {
            controls: vec![Placed {
                control: Control::DPad,
                frame: Rect::around(pad, 168.0),
            }],
            fingers: Vec::new(),
            unit: 3.0,
        };
        let bit = |direction: u32| direction << Control::DPad.code();
        assert!(gamepad.touch(1, TouchPhase::Down, pad).redraw);
        assert_eq!(gamepad.state().buttons, 0);
        gamepad.touch(1, TouchPhase::Move, moved(pad, 0.0, -100.0));
        assert_eq!(gamepad.state().buttons, bit(UP));
        let outcome = gamepad.touch(1, TouchPhase::Move, moved(pad, 100.0, -100.0));
        assert!(outcome.pressed);
        assert_eq!(gamepad.state().buttons, bit(UP | RIGHT));
        gamepad.touch(1, TouchPhase::Move, moved(pad, 100.0, 20.0));
        assert_eq!(gamepad.state().buttons, bit(RIGHT));
        let view = gamepad
            .controls()
            .into_iter()
            .find(|view| view.control == Control::DPad)
            .unwrap();
        assert_eq!(view.directions, RIGHT);
    }

    /// A touch between the controls does nothing.
    #[test]
    fn ignores_touches_between_the_controls() {
        let mut gamepad = phone();
        let outcome = gamepad.touch(
            1,
            TouchPhase::Down,
            Point {
                x: PHONE.width / 2.0,
                y: PHONE.height / 3.0,
            },
        );
        assert_eq!(outcome, TouchOutcome::default());
        assert!(gamepad.controls().iter().all(|view| !view.held));
    }

    /// Opening the menu, or laying the controls out again, releases them.
    #[test]
    fn releases_everything_held() {
        let mut gamepad = phone();
        gamepad.touch(1, TouchPhase::Down, center(&gamepad, Control::Start));
        assert!(gamepad.release_all());
        assert_eq!(gamepad.state().buttons, 0);
        assert!(!gamepad.release_all());
        gamepad.touch(1, TouchPhase::Down, center(&gamepad, Control::Y));
        assert!(gamepad.lay_out(&SCREENS[1]));
        assert_eq!(gamepad.state().buttons, 0);
    }

    /// The bindings' flags say what the app redraws and when it taps the
    /// haptics.
    #[test]
    fn reports_outcomes_as_flags() {
        let outcome = |redraw, pressed| TouchOutcome {
            state_changed: true,
            redraw,
            pressed,
        };
        assert_eq!(outcome(false, false).flags(), 0);
        assert_eq!(outcome(true, false).flags(), REDRAW);
        assert_eq!(outcome(true, true).flags(), REDRAW | PRESSED);
    }

    /// Each control is known by its Standard Gamepad index, the sticks by
    /// numbers past them.
    #[test]
    fn numbers_controls_by_their_standard_gamepad_index() {
        assert_eq!(Control::A.code(), 0);
        assert_eq!(Control::Start.code(), 9);
        assert_eq!(Control::DPad.code(), 12);
        assert_eq!(Control::RightStick.code(), 18);
    }

    /// A screen with no safe area has no controls.
    #[test]
    fn lays_out_nothing_without_room() {
        let mut gamepad = VirtualGamepad::new();
        gamepad.lay_out(&Screen {
            width: 0.0,
            height: 0.0,
            density: 3.0,
            insets: Insets::default(),
        });
        assert!(gamepad.controls().is_empty());
        let outcome = gamepad.touch(1, TouchPhase::Down, Point::default());
        assert_eq!(outcome, TouchOutcome::default());
    }
}
