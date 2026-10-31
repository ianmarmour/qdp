//! Pen input on macOS as Quartz tablet events: proximity events when the pen
//! comes into or leaves range, and mouse events of the tablet point subtype
//! carrying pressure, tilt and rotation, which drawing applications read.

use objc2_core_foundation::CGPoint;
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventMouseSubtype, CGEventTapLocation, CGEventType, CGMouseButton,
};
use qdp::message::{PenFlags, PenReport};

/// The tablet device ID the pen's proximity and point events share.
const DEVICE_ID: i64 = 1;

/// `NX_TABLET_POINTER_PEN`: the tip end is in range.
const POINTER_TYPE_PEN: i64 = 1;

/// `NX_TABLET_POINTER_ERASER`: the eraser end is in range.
const POINTER_TYPE_ERASER: i64 = 3;

/// The tablet buttons bit of the tip.
const TIP_BUTTON: i64 = 0x01;

/// The tablet buttons bit of the barrel button.
const BARREL_BUTTON: i64 = 0x02;

/// QDP's tilt and twist are in 1/100 degree (Section 8.4.5).
const CENTIDEGREES: f64 = 100.0;

/// The tilt Quartz reports as fully tilted, in degrees.
const FULL_TILT_DEGREES: f64 = 90.0;

/// Returns the pointer type of the end a report has in range, if any.
fn pointer_type(report: &PenReport) -> Option<i64> {
    if !report.flags.contains(PenFlags::IN_RANGE) {
        None
    } else if report.flags.contains(PenFlags::INVERT) || report.flags.contains(PenFlags::ERASER) {
        Some(POINTER_TYPE_ERASER)
    } else {
        Some(POINTER_TYPE_PEN)
    }
}

/// Returns whether a report's tip or eraser touches.
fn touching(report: &PenReport) -> bool {
    report.flags.contains(PenFlags::TIP_SWITCH) || report.flags.contains(PenFlags::ERASER)
}

/// Returns the mouse event type taking the pen from `previous` to `report`.
fn point_event_type(previous: &PenReport, report: &PenReport) -> CGEventType {
    match (touching(previous), touching(report)) {
        (false, true) => CGEventType::LeftMouseDown,
        (true, true) => CGEventType::LeftMouseDragged,
        (true, false) => CGEventType::LeftMouseUp,
        (false, false) => CGEventType::MouseMoved,
    }
}

/// Posts a proximity event: the pen's end of `pointer_type` entering or
/// leaving range.
fn post_proximity(pointer_type: i64, entering: bool) -> Result<(), String> {
    let event = CGEvent::new(None).ok_or("cannot create a tablet event")?;
    CGEvent::set_type(Some(&event), CGEventType::TabletProximity);
    let fields = [
        (CGEventField::TabletProximityEventDeviceID, DEVICE_ID),
        (CGEventField::TabletProximityEventPointerType, pointer_type),
        (
            CGEventField::TabletProximityEventEnterProximity,
            i64::from(entering),
        ),
    ];
    for (field, value) in fields {
        CGEvent::set_integer_value_field(Some(&event), field, value);
    }
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    Ok(())
}

/// Posts a tablet point at `position`, in global points.
fn post_point(previous: &PenReport, report: &PenReport, position: CGPoint) -> Result<(), String> {
    let event = CGEvent::new_mouse_event(
        None,
        point_event_type(previous, report),
        position,
        CGMouseButton::Left,
    )
    .ok_or("cannot create a tablet event")?;
    let buttons = [
        (touching(report), TIP_BUTTON),
        (
            report.flags.contains(PenFlags::BARREL_SWITCH),
            BARREL_BUTTON,
        ),
    ]
    .into_iter()
    .filter(|(held, _)| *held)
    .fold(0, |buttons, (_, bit)| buttons | bit);
    let integers = [
        (
            CGEventField::MouseEventSubtype,
            i64::from(CGEventMouseSubtype::TabletPoint.0),
        ),
        (CGEventField::TabletEventDeviceID, DEVICE_ID),
        (CGEventField::TabletEventPointButtons, buttons),
    ];
    for (field, value) in integers {
        CGEvent::set_integer_value_field(Some(&event), field, value);
    }
    let tilt = |centidegrees: i16| f64::from(centidegrees) / CENTIDEGREES / FULL_TILT_DEGREES;
    let doubles = [
        (
            CGEventField::TabletEventPointPressure,
            f64::from(report.pressure) / f64::from(u16::MAX),
        ),
        (CGEventField::TabletEventTiltX, tilt(report.tilt_x)),
        // Quartz's tilt is positive away from the user.
        (CGEventField::TabletEventTiltY, -tilt(report.tilt_y)),
        (
            CGEventField::TabletEventRotation,
            f64::from(report.twist) / CENTIDEGREES,
        ),
    ];
    for (field, value) in doubles {
        CGEvent::set_double_value_field(Some(&event), field, value);
    }
    CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    Ok(())
}

/// Posts the pen's input.
#[derive(Debug, Default)]
pub(super) struct TabletPen {
    /// The state last posted.
    previous: PenReport,
}

impl TabletPen {
    /// Sets the pen to `report`, at `position` in global points.
    pub(super) fn set(&mut self, report: &PenReport, position: CGPoint) -> Result<(), String> {
        let previous = std::mem::replace(&mut self.previous, *report);
        let (was_in_range, in_range) = (pointer_type(&previous), pointer_type(report));
        if was_in_range != in_range {
            if let Some(leaving) = was_in_range {
                if touching(&previous) {
                    let lifted = PenReport {
                        flags: PenFlags::IN_RANGE,
                        ..previous
                    };
                    post_point(&previous, &lifted, position)?;
                }
                post_proximity(leaving, false)?;
            }
            let Some(entering) = in_range else {
                return Ok(());
            };
            post_proximity(entering, true)?;
            let hovering = PenReport::default();
            return post_point(&hovering, report, position);
        }
        if in_range.is_none() {
            return Ok(());
        }
        post_point(&previous, report, position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tip touching presses, drags and releases the left button, and the
    /// eraser end has its own pointer type.
    #[test]
    fn follows_the_tip() {
        let hovering = PenReport {
            flags: PenFlags::IN_RANGE,
            ..PenReport::default()
        };
        let touching = PenReport {
            flags: PenFlags(PenFlags::IN_RANGE.0 | PenFlags::TIP_SWITCH.0),
            ..hovering
        };
        assert_eq!(
            point_event_type(&hovering, &touching),
            CGEventType::LeftMouseDown
        );
        assert_eq!(
            point_event_type(&touching, &touching),
            CGEventType::LeftMouseDragged
        );
        assert_eq!(
            point_event_type(&touching, &hovering),
            CGEventType::LeftMouseUp
        );
        let erasing = PenReport {
            flags: PenFlags(PenFlags::IN_RANGE.0 | PenFlags::INVERT.0),
            ..hovering
        };
        assert_eq!(pointer_type(&erasing), Some(POINTER_TYPE_ERASER));
        assert_eq!(pointer_type(&PenReport::default()), None);
    }
}
