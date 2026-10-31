//! A virtual pen tablet covering the desktop, through uinput, for the
//! client's pen.
//!
//! It reports like a screen tablet: the tool in range (`BTN_TOOL_PEN`, or
//! `BTN_TOOL_RUBBER` for the eraser end), whether it touches (`BTN_TOUCH`),
//! the barrel button (`BTN_STYLUS`), position, pressure and tilt. A tool
//! leaves range before another enters, as libinput requires.

use std::io;

use qdp::message::{PenFlags, PenReport};

use super::uinput::{
    ABS_X, ABS_Y, AxisRange, EV_ABS, EV_KEY, Event, INPUT_PROP_DIRECT, Identity, UI_SET_EVBIT,
    UI_SET_KEYBIT, UI_SET_PROPBIT, VirtualDevice,
};

/// `BTN_TOOL_PEN`: the pen's tip end is in range.
const BTN_TOOL_PEN: u16 = 0x140;

/// `BTN_TOOL_RUBBER`: the pen's eraser end is in range.
const BTN_TOOL_RUBBER: u16 = 0x141;

/// `BTN_TOUCH`: the tool touches the surface.
const BTN_TOUCH: u16 = 0x14a;

/// `BTN_STYLUS`: the barrel button.
const BTN_STYLUS: u16 = 0x14b;

/// `ABS_PRESSURE`.
const ABS_PRESSURE: u16 = 0x18;

/// `ABS_TILT_X`: tilt towards the right.
const ABS_TILT_X: u16 = 0x1a;

/// `ABS_TILT_Y`: tilt towards the user.
const ABS_TILT_Y: u16 = 0x1b;

/// The largest tilt, in the reports' 1/100 degree.
const MAX_TILT: i32 = 9000;

/// The tilt axes' resolution, in their units per radian: 1/100 degree.
const TILT_RESOLUTION: i32 = 5730;

/// How the pen presents itself.
const IDENTITY: Identity<'static> = Identity::virtual_device("QDP pen");

/// The end of the pen in range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    /// The writing tip.
    Pen,
    /// The eraser.
    Rubber,
}

impl Tool {
    /// Returns the tool a report has in range, if any.
    fn of(report: &PenReport) -> Option<Self> {
        if !report.flags.contains(PenFlags::IN_RANGE) {
            None
        } else if report.flags.contains(PenFlags::INVERT) || report.flags.contains(PenFlags::ERASER)
        {
            Some(Self::Rubber)
        } else {
            Some(Self::Pen)
        }
    }

    /// Returns the tool's key code.
    const fn code(self) -> u16 {
        match self {
            Self::Pen => BTN_TOOL_PEN,
            Self::Rubber => BTN_TOOL_RUBBER,
        }
    }
}

/// Returns the groups of events, each ended by its own `SYN_REPORT`, that
/// take the pen from the tool `previous` had in range to `report`.
fn frames(previous: Option<Tool>, report: &PenReport) -> Vec<Vec<Event>> {
    let tool = Tool::of(report);
    let mut frames = Vec::new();
    if let Some(leaving) = previous.filter(|&leaving| Some(leaving) != tool) {
        frames.push(vec![
            Event::new(EV_KEY, BTN_TOUCH, 0),
            Event::new(EV_KEY, BTN_STYLUS, 0),
            Event::new(EV_ABS, ABS_PRESSURE, 0),
            Event::new(EV_KEY, leaving.code(), 0),
        ]);
    }
    if let Some(tool) = tool {
        let touching =
            report.flags.contains(PenFlags::TIP_SWITCH) || report.flags.contains(PenFlags::ERASER);
        frames.push(vec![
            Event::new(EV_KEY, tool.code(), 1),
            Event::new(EV_ABS, ABS_X, report.x.max(0)),
            Event::new(EV_ABS, ABS_Y, report.y.max(0)),
            Event::new(EV_ABS, ABS_PRESSURE, i32::from(report.pressure)),
            Event::new(EV_ABS, ABS_TILT_X, i32::from(report.tilt_x)),
            Event::new(EV_ABS, ABS_TILT_Y, i32::from(report.tilt_y)),
            Event::new(EV_KEY, BTN_TOUCH, i32::from(touching)),
            Event::new(
                EV_KEY,
                BTN_STYLUS,
                i32::from(report.flags.contains(PenFlags::BARREL_SWITCH)),
            ),
        ]);
    }
    frames
}

/// A virtual pen tablet.
#[derive(Debug)]
pub(super) struct VirtualPen {
    /// The uinput device.
    device: VirtualDevice,
    /// The tool in range.
    tool: Option<Tool>,
}

impl VirtualPen {
    /// Creates a pen tablet covering a `width × height` pixel desktop, in
    /// 1/256 pixel.
    pub(super) fn new(width: u32, height: u32) -> io::Result<Self> {
        let file = VirtualDevice::open()?;
        VirtualDevice::set_bit(&file, UI_SET_PROPBIT, INPUT_PROP_DIRECT)?;
        VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_KEY)?;
        for code in [BTN_TOOL_PEN, BTN_TOOL_RUBBER, BTN_TOUCH, BTN_STYLUS] {
            VirtualDevice::set_bit(&file, UI_SET_KEYBIT, code)?;
        }
        VirtualDevice::set_bit(&file, UI_SET_EVBIT, EV_ABS)?;
        VirtualDevice::set_axis(&file, ABS_X, AxisRange::desktop(width))?;
        VirtualDevice::set_axis(&file, ABS_Y, AxisRange::desktop(height))?;
        let pressure = AxisRange::new(0, i32::from(u16::MAX));
        VirtualDevice::set_axis(&file, ABS_PRESSURE, pressure)?;
        let tilt = AxisRange {
            resolution: TILT_RESOLUTION,
            ..AxisRange::new(-MAX_TILT, MAX_TILT)
        };
        VirtualDevice::set_axis(&file, ABS_TILT_X, tilt)?;
        VirtualDevice::set_axis(&file, ABS_TILT_Y, tilt)?;
        Ok(Self {
            device: VirtualDevice::create(file, IDENTITY)?,
            tool: None,
        })
    }

    /// Sets the pen to `report`.
    pub(super) fn set(&mut self, report: &PenReport) -> io::Result<()> {
        for frame in frames(self.tool, report) {
            self.device.emit(&frame)?;
        }
        self.tool = Tool::of(report);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a report with `flags` at (10, 20).
    fn report(flags: &[PenFlags]) -> PenReport {
        PenReport {
            flags: PenFlags(flags.iter().fold(0, |all, flag| all | flag.0)),
            x: 10,
            y: 20,
            pressure: 500,
            tilt_x: -4500,
            tilt_y: 100,
            twist: 0,
        }
    }

    /// Returns the value a frame gives `code`, if it sets it.
    fn value(frame: &[Event], kind: u16, code: u16) -> Option<i32> {
        frame
            .iter()
            .find(|event| event.kind == kind && event.code == code)
            .map(|event| event.value)
    }

    /// A pen in range reports its tool, position, pressure, tilt, tip and
    /// barrel; out of range it reports its tool leaving.
    #[test]
    fn reports_the_pen() {
        let touching = report(&[
            PenFlags::IN_RANGE,
            PenFlags::TIP_SWITCH,
            PenFlags::BARREL_SWITCH,
        ]);
        let entering = frames(None, &touching);
        assert_eq!(entering.len(), 1);
        let frame = &entering[0];
        assert_eq!(value(frame, EV_KEY, BTN_TOOL_PEN), Some(1));
        assert_eq!(value(frame, EV_KEY, BTN_TOUCH), Some(1));
        assert_eq!(value(frame, EV_KEY, BTN_STYLUS), Some(1));
        assert_eq!(value(frame, EV_ABS, ABS_TILT_X), Some(-4500));
        assert_eq!(value(frame, EV_ABS, ABS_PRESSURE), Some(500));
        let leaving = frames(Some(Tool::Pen), &report(&[]));
        assert_eq!(leaving.len(), 1);
        assert_eq!(value(&leaving[0], EV_KEY, BTN_TOOL_PEN), Some(0));
        assert!(frames(None, &report(&[])).is_empty());
    }

    /// Turning the pen over takes the tip out of range before the eraser
    /// comes in, and the eraser touching is a touch.
    #[test]
    fn switches_tools() {
        let erasing = report(&[PenFlags::IN_RANGE, PenFlags::INVERT, PenFlags::ERASER]);
        let switched = frames(Some(Tool::Pen), &erasing);
        assert_eq!(switched.len(), 2);
        assert_eq!(value(&switched[0], EV_KEY, BTN_TOOL_PEN), Some(0));
        assert_eq!(value(&switched[1], EV_KEY, BTN_TOOL_RUBBER), Some(1));
        assert_eq!(value(&switched[1], EV_KEY, BTN_TOUCH), Some(1));
        assert_eq!(frames(Some(Tool::Rubber), &erasing).len(), 1);
    }
}
