//! A virtual gamepad, through uinput, for each gamepad the client attaches.
//!
//! Each one presents itself as a wired Xbox 360 controller, with the codes,
//! ranges and USB IDs the kernel's `xpad` driver gives one, so that games and
//! SDL apply their mapping for that controller. It has no motion sensors, so
//! the reports' gyro and accelerometer readings are not passed on.
//!
//! Applications rumble it through the kernel's force feedback interface: they
//! upload `FF_RUMBLE` effects, which uinput hands to this process to accept,
//! then play and stop them. The gamepad works out the rumble the playing
//! effects add up to, which the host sends to the client.

use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use qdp::message::{GamepadButtons, GamepadOutput, GamepadReport};
use tracing::debug;

use super::uinput::{
    ABS_X, ABS_Y, AxisRange, EV_ABS, EV_KEY, Event, IOC_READ, IOC_WRITE, Identity, UI_SET_EVBIT,
    UI_SET_KEYBIT, VirtualDevice, uinput_request,
};

/// `UI_SET_FFBIT`.
const UI_SET_FFBIT: libc::Ioctl = uinput_request(IOC_WRITE, 107, size_of::<libc::c_int>());

/// `UI_BEGIN_FF_UPLOAD`.
const UI_BEGIN_FF_UPLOAD: libc::Ioctl = uinput_request(
    IOC_READ | IOC_WRITE,
    200,
    size_of::<libc::uinput_ff_upload>(),
);

/// `UI_END_FF_UPLOAD`.
const UI_END_FF_UPLOAD: libc::Ioctl =
    uinput_request(IOC_WRITE, 201, size_of::<libc::uinput_ff_upload>());

/// `UI_BEGIN_FF_ERASE`.
const UI_BEGIN_FF_ERASE: libc::Ioctl = uinput_request(
    IOC_READ | IOC_WRITE,
    202,
    size_of::<libc::uinput_ff_erase>(),
);

/// `UI_END_FF_ERASE`.
const UI_END_FF_ERASE: libc::Ioctl =
    uinput_request(IOC_WRITE, 203, size_of::<libc::uinput_ff_erase>());

/// `EV_FF`: force feedback, which applications write to play effects.
const EV_FF: u16 = 0x15;

/// `EV_UINPUT`: uinput's requests to the process behind a device.
const EV_UINPUT: u16 = 0x0101;

/// `UI_FF_UPLOAD`: an application is uploading an effect.
const UI_FF_UPLOAD: u16 = 1;

/// `UI_FF_ERASE`: an application is erasing an effect.
const UI_FF_ERASE: u16 = 2;

/// `FF_RUMBLE`: an effect turning the strong and weak motors.
const FF_RUMBLE: u16 = 0x50;

/// `FF_GAIN`: scales every effect's strength.
const FF_GAIN: u16 = 0x60;

/// The strongest gain, which leaves effects unchanged.
const FULL_GAIN: u16 = u16::MAX;

/// The effects a gamepad holds at once.
const MAX_EFFECTS: u32 = 16;

/// `ABS_Z`: the left trigger.
const ABS_Z: u16 = 0x02;

/// `ABS_RX`: the right stick's horizontal axis.
const ABS_RX: u16 = 0x03;

/// `ABS_RY`: the right stick's vertical axis.
const ABS_RY: u16 = 0x04;

/// `ABS_RZ`: the right trigger.
const ABS_RZ: u16 = 0x05;

/// `ABS_HAT0X`: the D-pad's horizontal axis.
const ABS_HAT0X: u16 = 0x10;

/// `ABS_HAT0Y`: the D-pad's vertical axis.
const ABS_HAT0Y: u16 = 0x11;

/// `BUS_USB`.
const BUS_USB: u16 = 0x03;

/// How each gamepad presents itself: a wired Xbox 360 controller.
const IDENTITY: Identity<'static> = Identity {
    name: "QDP gamepad (Xbox 360)",
    bus: BUS_USB,
    vendor: 0x045e,
    product: 0x028e,
    version: 0x0114,
    effects: MAX_EFFECTS,
};

/// The Standard Gamepad buttons that are buttons on the controller, with
/// their codes. Buttons 6 and 7 are the triggers' axes, and 12 to 15 the
/// D-pad's hat. The Xbox controller's left face button, X, has the code
/// `BTN_X` and its top one, Y, `BTN_Y`, as `xpad` gives them.
const BUTTONS: [(u32, u16); 11] = [
    (0, 0x130),  // A: BTN_SOUTH
    (1, 0x131),  // B: BTN_EAST
    (2, 0x133),  // X: BTN_X
    (3, 0x134),  // Y: BTN_Y
    (4, 0x136),  // LB: BTN_TL
    (5, 0x137),  // RB: BTN_TR
    (8, 0x13a),  // Back: BTN_SELECT
    (9, 0x13b),  // Start: BTN_START
    (16, 0x13c), // Guide: BTN_MODE
    (10, 0x13d), // Left stick: BTN_THUMBL
    (11, 0x13e), // Right stick: BTN_THUMBR
];

/// The Standard Gamepad buttons of the D-pad: up, down, left and right.
const DPAD: [u32; 4] = [12, 13, 14, 15];

/// The sticks' axes.
const STICKS: [u16; 4] = [ABS_X, ABS_Y, ABS_RX, ABS_RY];

/// The noise the sticks' values filter out, as `xpad` sets it.
const STICK_FUZZ: i32 = 16;

/// The sticks' dead zone around the centre, as `xpad` sets it.
const STICK_FLAT: i32 = 128;

/// The largest trigger value, as an Xbox 360 controller's triggers report.
const TRIGGER_MAXIMUM: i32 = 255;

/// The bits a 16-bit trigger value drops to fit the controller's range.
const TRIGGER_SHIFT: u32 = 8;

/// Returns the events setting the controller to `report`. The kernel passes on
/// only the values that changed.
fn report_events(report: &GamepadReport) -> Vec<Event> {
    let held = |button: u32| report.buttons.contains(GamepadButtons::button(button));
    let mut events: Vec<Event> = BUTTONS
        .iter()
        .map(|&(button, code)| Event::new(EV_KEY, code, i32::from(held(button))))
        .collect();
    let sticks = [
        report.left_stick_x,
        report.left_stick_y,
        report.right_stick_x,
        report.right_stick_y,
    ];
    events.extend(
        STICKS
            .iter()
            .zip(sticks)
            .map(|(&axis, value)| Event::new(EV_ABS, axis, i32::from(value))),
    );
    for (axis, value) in [(ABS_Z, report.left_trigger), (ABS_RZ, report.right_trigger)] {
        events.push(Event::new(EV_ABS, axis, i32::from(value >> TRIGGER_SHIFT)));
    }
    let [up, down, left, right] = DPAD.map(|button| i32::from(held(button)));
    events.push(Event::new(EV_ABS, ABS_HAT0X, right - left));
    events.push(Event::new(EV_ABS, ABS_HAT0Y, down - up));
    events
}

/// An uploaded rumble effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rumble {
    /// The strong, low-frequency motor's strength.
    strong: u16,
    /// The weak, high-frequency motor's strength.
    weak: u16,
    /// How long one play lasts, or zero for until it is stopped.
    length: Duration,
    /// How long after starting it the motors turn.
    delay: Duration,
}

impl Rumble {
    /// Returns the rumble an `FF_RUMBLE` effect describes, or `None` for any
    /// other kind of effect.
    fn from_effect(effect: &libc::ff_effect) -> Option<Self> {
        if effect.type_ != FF_RUMBLE {
            return None;
        }
        // The union starts with `struct ff_rumble_effect`: the strong, then the
        // weak magnitude.
        let union = effect.u[0].to_ne_bytes();
        Some(Self {
            strong: u16::from_ne_bytes([union[0], union[1]]),
            weak: u16::from_ne_bytes([union[2], union[3]]),
            length: Duration::from_millis(u64::from(effect.replay.length)),
            delay: Duration::from_millis(u64::from(effect.replay.delay)),
        })
    }
}

/// A playing effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Playing {
    /// When it started.
    started: Instant,
    /// When it ends, or `None` if it plays until it is stopped.
    ends: Option<Instant>,
}

/// The effects applications uploaded and play, and the rumble they add up to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RumbleState {
    /// The uploaded effects, by effect ID.
    effects: HashMap<i16, Rumble>,
    /// The playing effects, by effect ID.
    playing: HashMap<i16, Playing>,
    /// The gain applications set, scaling every effect.
    gain: u16,
}

impl Default for RumbleState {
    fn default() -> Self {
        Self {
            effects: HashMap::new(),
            playing: HashMap::new(),
            gain: FULL_GAIN,
        }
    }
}

impl RumbleState {
    /// Stores an uploaded effect, returning whether it is one the gamepad can
    /// play.
    fn upload(&mut self, effect: &libc::ff_effect) -> bool {
        let Some(rumble) = Rumble::from_effect(effect) else {
            return false;
        };
        self.effects.insert(effect.id, rumble);
        true
    }

    /// Forgets an erased effect, stopping it.
    fn erase(&mut self, id: i16) {
        self.effects.remove(&id);
        self.playing.remove(&id);
    }

    /// Plays an effect `count` times, or stops it if `count` is zero.
    fn play(&mut self, id: i16, count: i32, now: Instant) {
        let Some(rumble) = self.effects.get(&id) else {
            return;
        };
        let Ok(count @ 1..) = u32::try_from(count) else {
            self.playing.remove(&id);
            return;
        };
        let ends = (!rumble.length.is_zero()).then(|| now + rumble.delay + rumble.length * count);
        self.playing.insert(id, Playing { started: now, ends });
    }

    /// Returns the rumble the effects playing at `now` add up to, forgetting
    /// those that have ended.
    fn output(&mut self, now: Instant) -> GamepadOutput {
        self.playing
            .retain(|_, playing| playing.ends.is_none_or(|ends| now < ends));
        let mut strong: u32 = 0;
        let mut weak: u32 = 0;
        for (id, playing) in &self.playing {
            let Some(rumble) = self.effects.get(id) else {
                continue;
            };
            if now < playing.started + rumble.delay {
                continue;
            }
            strong += u32::from(rumble.strong);
            weak += u32::from(rumble.weak);
        }
        let scale = |value: u32| {
            let scaled =
                value.min(u32::from(u16::MAX)) * u32::from(self.gain) / u32::from(FULL_GAIN);
            u16::try_from(scaled).unwrap_or(u16::MAX)
        };
        GamepadOutput {
            low_frequency_rumble: scale(strong),
            high_frequency_rumble: scale(weak),
            ..GamepadOutput::default()
        }
    }
}

/// One virtual gamepad.
#[derive(Debug)]
pub(super) struct VirtualGamepad {
    /// The uinput device.
    device: VirtualDevice,
    /// Its force feedback.
    rumble: RumbleState,
    /// The output last reported to the host.
    reported: GamepadOutput,
}

impl VirtualGamepad {
    /// Creates a gamepad at rest.
    pub(super) fn new() -> io::Result<Self> {
        let file = VirtualDevice::open()?;
        VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_KEY)?;
        for (_, code) in BUTTONS {
            VirtualDevice::set_bit(&file, UI_SET_KEYBIT, code)?;
        }
        VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_ABS)?;
        let stick = AxisRange {
            fuzz: STICK_FUZZ,
            flat: STICK_FLAT,
            ..AxisRange::new(i32::from(i16::MIN), i32::from(i16::MAX))
        };
        for axis in STICKS {
            VirtualDevice::set_axis(&file, axis, stick)?;
        }
        for axis in [ABS_Z, ABS_RZ] {
            VirtualDevice::set_axis(&file, axis, AxisRange::new(0, TRIGGER_MAXIMUM))?;
        }
        for axis in [ABS_HAT0X, ABS_HAT0Y] {
            VirtualDevice::set_axis(&file, axis, AxisRange::new(-1, 1))?;
        }
        VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_FF)?;
        VirtualDevice::set_bit(&file, UI_SET_FFBIT, FF_RUMBLE)?;
        VirtualDevice::set_bit(&file, UI_SET_FFBIT, FF_GAIN)?;
        Ok(Self {
            device: VirtualDevice::create(file, IDENTITY)?,
            rumble: RumbleState::default(),
            reported: GamepadOutput::default(),
        })
    }

    /// Sets the controls to `report`.
    pub(super) fn set(&mut self, report: &GamepadReport) -> io::Result<()> {
        self.device.emit(&report_events(report))
    }

    /// Answers the applications' force feedback requests and returns the
    /// rumble, if it changed since the last call.
    pub(super) fn output(&mut self, now: Instant) -> Option<GamepadOutput> {
        for event in self.device.read_events() {
            match (event.kind, event.code) {
                (EV_UINPUT, UI_FF_UPLOAD) => self.answer_upload(event.value),
                (EV_UINPUT, UI_FF_ERASE) => self.answer_erase(event.value),
                (EV_FF, FF_GAIN) => {
                    self.rumble.gain = u16::try_from(event.value).unwrap_or(FULL_GAIN);
                }
                (EV_FF, effect) => {
                    if let Ok(id) = i16::try_from(effect) {
                        self.rumble.play(id, event.value, now);
                    }
                }
                _ => {}
            }
        }
        let output = self.rumble.output(now);
        if output == self.reported {
            return None;
        }
        self.reported = output;
        Some(output)
    }

    /// Accepts or refuses the effect an application is uploading.
    fn answer_upload(&mut self, request_id: i32) {
        // SAFETY: all zero bits is a valid `uinput_ff_upload`.
        let mut upload: libc::uinput_ff_upload = unsafe { std::mem::zeroed() };
        upload.request_id = request_id as u32;
        // SAFETY: `upload` is a valid `uinput_ff_upload`, which the call fills
        // in with the effect.
        if unsafe { libc::ioctl(self.device.raw_fd(), UI_BEGIN_FF_UPLOAD, &mut upload) } < 0 {
            debug!(error = %io::Error::last_os_error(), "force feedback upload lost");
            return;
        }
        upload.retval = if self.rumble.upload(&upload.effect) {
            0
        } else {
            -libc::EINVAL
        };
        // SAFETY: `upload` is the `uinput_ff_upload` the kernel filled in,
        // which the call reads.
        if unsafe { libc::ioctl(self.device.raw_fd(), UI_END_FF_UPLOAD, &upload) } < 0 {
            debug!(error = %io::Error::last_os_error(), "force feedback upload not answered");
        }
    }

    /// Accepts the erasure of an effect.
    fn answer_erase(&mut self, request_id: i32) {
        // SAFETY: all zero bits is a valid `uinput_ff_erase`.
        let mut erase: libc::uinput_ff_erase = unsafe { std::mem::zeroed() };
        erase.request_id = request_id as u32;
        // SAFETY: `erase` is a valid `uinput_ff_erase`, which the call fills in
        // with the effect ID.
        if unsafe { libc::ioctl(self.device.raw_fd(), UI_BEGIN_FF_ERASE, &mut erase) } < 0 {
            debug!(error = %io::Error::last_os_error(), "force feedback erasure lost");
            return;
        }
        if let Ok(id) = i16::try_from(erase.effect_id) {
            self.rumble.erase(id);
        }
        erase.retval = 0;
        // SAFETY: `erase` is the `uinput_ff_erase` the kernel filled in, which
        // the call reads.
        if unsafe { libc::ioctl(self.device.raw_fd(), UI_END_FF_ERASE, &erase) } < 0 {
            debug!(error = %io::Error::last_os_error(), "force feedback erasure not answered");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The force feedback requests have the numbers of
    /// `linux/uinput.h` on 64-bit systems.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn request_numbers() {
        assert_eq!(size_of::<libc::ff_effect>(), 48);
        assert_eq!(UI_SET_FFBIT, 0x4004_556b);
        assert_eq!(UI_BEGIN_FF_UPLOAD, 0xc068_55c8);
        assert_eq!(UI_END_FF_UPLOAD, 0x4068_55c9);
        assert_eq!(UI_BEGIN_FF_ERASE, 0xc00c_55ca);
        assert_eq!(UI_END_FF_ERASE, 0x400c_55cb);
    }

    /// Reports become the controller's buttons, sticks, 8-bit triggers and
    /// D-pad hat.
    #[test]
    fn translates_reports() {
        let report = GamepadReport {
            buttons: GamepadButtons(1 << 2 | 1 << 12 | 1 << 15),
            left_trigger: u16::MAX,
            right_trigger: 0x80ff,
            left_stick_x: -32_768,
            right_stick_y: 1234,
            ..GamepadReport::default()
        };
        let events = report_events(&report);
        let value = |kind, code| {
            events
                .iter()
                .find(|event| event.kind == kind && event.code == code)
                .map(|event| event.value)
        };
        assert_eq!(value(EV_KEY, 0x133), Some(1));
        assert_eq!(value(EV_KEY, 0x130), Some(0));
        assert_eq!(value(EV_ABS, ABS_X), Some(-32_768));
        assert_eq!(value(EV_ABS, ABS_RY), Some(1234));
        assert_eq!(value(EV_ABS, ABS_Z), Some(255));
        assert_eq!(value(EV_ABS, ABS_RZ), Some(0x80));
        assert_eq!(value(EV_ABS, ABS_HAT0X), Some(1));
        assert_eq!(value(EV_ABS, ABS_HAT0Y), Some(-1));
    }

    /// Returns an `FF_RUMBLE` effect.
    fn rumble_effect(id: i16, strong: u16, weak: u16, length: u16) -> libc::ff_effect {
        // SAFETY: all zero bits is a valid `ff_effect`.
        let mut effect: libc::ff_effect = unsafe { std::mem::zeroed() };
        effect.type_ = FF_RUMBLE;
        effect.id = id;
        effect.replay.length = length;
        let rumble = std::ptr::addr_of_mut!(effect.u).cast::<u16>();
        // SAFETY: the union starts with `struct ff_rumble_effect`, two `u16`s,
        // and is aligned for them.
        unsafe {
            rumble.write(strong);
            rumble.add(1).write(weak);
        }
        effect
    }

    /// Playing effects add up, scaled by the gain, and stop when stopped,
    /// erased, or done.
    #[test]
    fn adds_up_rumble() {
        let mut state = RumbleState::default();
        let start = Instant::now();
        assert!(state.upload(&rumble_effect(0, 40_000, 1000, 0)));
        assert!(state.upload(&rumble_effect(1, 40_000, 2000, 100)));
        let mut periodic = rumble_effect(2, 1, 1, 0);
        periodic.type_ = 0x51; // FF_PERIODIC
        assert!(!state.upload(&periodic));
        state.play(0, 1, start);
        state.play(1, 2, start);
        let output = state.output(start);
        assert_eq!(output.low_frequency_rumble, u16::MAX);
        assert_eq!(output.high_frequency_rumble, 3000);
        state.gain = FULL_GAIN / 2;
        let later = start + Duration::from_millis(150);
        assert_eq!(state.output(later).high_frequency_rumble, 1499);
        // Two plays of 100 ms end after 200 ms.
        let done = start + Duration::from_millis(200);
        assert_eq!(state.output(done).high_frequency_rumble, 499);
        state.play(0, 0, done);
        assert_eq!(state.output(done), GamepadOutput::default());
        state.play(0, 1, done);
        state.erase(0);
        assert_eq!(state.output(done), GamepadOutput::default());
    }
}
