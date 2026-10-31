//! Pen injection on Windows through a synthetic pointer device, which
//! Windows 10 version 1809 and later provide for performing pen input as a
//! digitizer would, with pressure, tilt, rotation, eraser and barrel button.

use qdp::message::{PenFlags, PenReport};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Controls::{
    CreateSyntheticPointerDevice, DestroySyntheticPointerDevice, HSYNTHETICPOINTERDEVICE,
    POINTER_FEEDBACK_DEFAULT, POINTER_TYPE_INFO, POINTER_TYPE_INFO_0,
};
use windows::Win32::UI::Input::Pointer::{
    InjectSyntheticPointerInput, POINTER_FLAG_DOWN, POINTER_FLAG_FIRSTBUTTON,
    POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_SECONDBUTTON, POINTER_FLAG_UP,
    POINTER_FLAG_UPDATE, POINTER_FLAGS, POINTER_INFO, POINTER_PEN_INFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PEN_FLAG_BARREL, PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_MASK_PRESSURE, PEN_MASK_ROTATION,
    PEN_MASK_TILT_X, PEN_MASK_TILT_Y, PT_PEN,
};

/// QDP positions are in 1/256 pixel (Section 1.5.6).
const SUBUNITS: i32 = 256;

/// QDP's tilt and twist are in 1/100 degree (Section 8.4.5).
const CENTIDEGREES: i32 = 100;

/// The strongest pressure Windows takes.
const MAX_PRESSURE: u32 = 1024;

/// The pointer ID of the one pen.
const PEN_POINTER_ID: u32 = 1;

/// Returns whether a report's tip or eraser touches.
fn touching(report: &PenReport) -> bool {
    report.flags.contains(PenFlags::TIP_SWITCH) || report.flags.contains(PenFlags::ERASER)
}

/// Returns the pointer flags taking the pen from `previous` to `report`.
fn pointer_flags(previous: &PenReport, report: &PenReport) -> POINTER_FLAGS {
    if !report.flags.contains(PenFlags::IN_RANGE) {
        // Updating without being in range takes the pen out of range.
        return POINTER_FLAG_UPDATE;
    }
    let mut flags = match (touching(previous), touching(report)) {
        (false, true) => POINTER_FLAG_DOWN | POINTER_FLAG_INCONTACT | POINTER_FLAG_FIRSTBUTTON,
        (true, false) => POINTER_FLAG_UP,
        (true, true) => POINTER_FLAG_UPDATE | POINTER_FLAG_INCONTACT | POINTER_FLAG_FIRSTBUTTON,
        (false, false) => POINTER_FLAG_UPDATE,
    } | POINTER_FLAG_INRANGE;
    if report.flags.contains(PenFlags::BARREL_SWITCH) {
        flags |= POINTER_FLAG_SECONDBUTTON;
    }
    flags
}

/// Returns the pen input taking the pen from `previous` to `report`, at a
/// position on the primary monitor, whose top-left corner is the virtual
/// screen's origin.
fn pen_info(previous: &PenReport, report: &PenReport) -> POINTER_PEN_INFO {
    let pen_flags = [
        (PenFlags::BARREL_SWITCH, PEN_FLAG_BARREL),
        (PenFlags::INVERT, PEN_FLAG_INVERTED),
        (PenFlags::ERASER, PEN_FLAG_ERASER),
    ]
    .into_iter()
    .filter(|(flag, _)| report.flags.contains(*flag))
    .fold(0, |flags, (_, windows_flag)| flags | windows_flag);
    POINTER_PEN_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_PEN,
            pointerId: PEN_POINTER_ID,
            pointerFlags: pointer_flags(previous, report),
            ptPixelLocation: POINT {
                x: report.x / SUBUNITS,
                y: report.y / SUBUNITS,
            },
            ..POINTER_INFO::default()
        },
        penFlags: pen_flags,
        penMask: PEN_MASK_PRESSURE | PEN_MASK_ROTATION | PEN_MASK_TILT_X | PEN_MASK_TILT_Y,
        pressure: u32::from(report.pressure) * MAX_PRESSURE / u32::from(u16::MAX),
        rotation: u32::from(report.twist) / CENTIDEGREES as u32,
        tiltX: i32::from(report.tilt_x) / CENTIDEGREES,
        tiltY: i32::from(report.tilt_y) / CENTIDEGREES,
    }
}

/// A synthetic pen device, destroyed when dropped.
#[derive(Debug)]
struct SyntheticPen(HSYNTHETICPOINTERDEVICE);

// SAFETY: the device handle names a system object that any thread may use.
unsafe impl Send for SyntheticPen {}

impl Drop for SyntheticPen {
    fn drop(&mut self) {
        // SAFETY: the handle came from `CreateSyntheticPointerDevice` and is
        // destroyed once.
        unsafe { DestroySyntheticPointerDevice(self.0) };
    }
}

/// Performs the pen's input, creating the synthetic pen on its first report.
#[derive(Debug, Default)]
pub(super) struct PenInjector {
    /// The synthetic pen, once created.
    device: Option<SyntheticPen>,
    /// The state last injected.
    previous: PenReport,
}

impl PenInjector {
    /// Sets the pen to `report`.
    pub(super) fn set(&mut self, report: &PenReport) -> Result<(), String> {
        let out_of_range = |report: &PenReport| !report.flags.contains(PenFlags::IN_RANGE);
        if out_of_range(&self.previous) && out_of_range(report) {
            self.previous = *report;
            return Ok(());
        }
        let device = match &mut self.device {
            Some(device) => device,
            empty => {
                // SAFETY: the call takes plain values.
                let handle =
                    unsafe { CreateSyntheticPointerDevice(PT_PEN, 1, POINTER_FEEDBACK_DEFAULT) }
                        .map_err(|error| format!("cannot create a pen: {error}"))?;
                empty.insert(SyntheticPen(handle))
            }
        };
        let input = POINTER_TYPE_INFO {
            r#type: PT_PEN,
            Anonymous: POINTER_TYPE_INFO_0 {
                penInfo: pen_info(&self.previous, report),
            },
        };
        self.previous = *report;
        // SAFETY: the device is open and the input is a fully initialised
        // pen `POINTER_TYPE_INFO`.
        unsafe { InjectSyntheticPointerInput(device.0, &[input]) }
            .map_err(|error| format!("Windows refused the pen input: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns a report with `flags`.
    fn report(flags: &[PenFlags]) -> PenReport {
        PenReport {
            flags: PenFlags(flags.iter().fold(0, |all, flag| all | flag.0)),
            x: 30 * SUBUNITS,
            y: 40 * SUBUNITS,
            pressure: u16::MAX,
            tilt_x: -4500,
            tilt_y: 1000,
            twist: 9000,
        }
    }

    /// The tip touching goes down, stays in contact, and lifts; the pen
    /// leaves range by updating without being in range.
    #[test]
    fn follows_the_tip() {
        let hovering = report(&[PenFlags::IN_RANGE]);
        let touching = report(&[PenFlags::IN_RANGE, PenFlags::TIP_SWITCH]);
        let out = report(&[]);
        assert_eq!(
            pointer_flags(&hovering, &touching),
            POINTER_FLAG_DOWN
                | POINTER_FLAG_INCONTACT
                | POINTER_FLAG_FIRSTBUTTON
                | POINTER_FLAG_INRANGE
        );
        assert_eq!(
            pointer_flags(&touching, &hovering),
            POINTER_FLAG_UP | POINTER_FLAG_INRANGE
        );
        assert_eq!(
            pointer_flags(&out, &hovering),
            POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE
        );
        assert_eq!(pointer_flags(&hovering, &out), POINTER_FLAG_UPDATE);
    }

    /// Pressure, tilt and rotation take Windows's units, and the eraser and
    /// barrel their flags.
    #[test]
    fn converts_the_pen() {
        let erasing = report(&[
            PenFlags::IN_RANGE,
            PenFlags::ERASER,
            PenFlags::INVERT,
            PenFlags::BARREL_SWITCH,
        ]);
        let info = pen_info(&report(&[PenFlags::IN_RANGE]), &erasing);
        assert_eq!(info.pointerInfo.ptPixelLocation, POINT { x: 30, y: 40 });
        assert_eq!(
            info.penFlags,
            PEN_FLAG_BARREL | PEN_FLAG_INVERTED | PEN_FLAG_ERASER
        );
        assert_eq!(
            (info.pressure, info.tiltX, info.tiltY, info.rotation),
            (1024, -45, 10, 90)
        );
        assert!(
            info.pointerInfo
                .pointerFlags
                .contains(POINTER_FLAG_SECONDBUTTON)
        );
    }
}
