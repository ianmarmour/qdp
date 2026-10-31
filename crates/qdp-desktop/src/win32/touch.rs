//! Touch injection on Windows with `InjectTouchInput`, which Windows 8 and
//! later provide for performing touches as a touchscreen would.
//!
//! Each injection lists every contact touching, so the host's touch frames,
//! which list them all, pass straight through.

use qdp::message::MAX_CONTACTS;
use qdp_runtime::{TouchPhase, TouchPoint};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::Pointer::{
    InitializeTouchInjection, InjectTouchInput, POINTER_FLAG_CANCELED, POINTER_FLAG_DOWN,
    POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_UP, POINTER_FLAG_UPDATE,
    POINTER_FLAGS, POINTER_INFO, POINTER_TOUCH_INFO, TOUCH_FEEDBACK_DEFAULT,
};
use windows::Win32::UI::WindowsAndMessaging::{PT_TOUCH, TOUCH_MASK_PRESSURE};

/// QDP positions are in 1/256 pixel (Section 1.5.6).
const SUBUNITS: i32 = 256;

/// The strongest pressure `InjectTouchInput` takes.
const MAX_PRESSURE: u32 = 1024;

/// Returns the pointer flags of a contact doing `phase`.
fn flags(phase: TouchPhase) -> POINTER_FLAGS {
    match phase {
        TouchPhase::Down => POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
        TouchPhase::Move => POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
        TouchPhase::Up => POINTER_FLAG_UP,
        TouchPhase::Cancel => POINTER_FLAG_UP | POINTER_FLAG_CANCELED,
    }
}

/// Returns a frame's point as a touch contact on the primary monitor, whose
/// top-left corner is the virtual screen's origin.
fn contact(point: &TouchPoint) -> POINTER_TOUCH_INFO {
    let (touch_mask, pressure) = if point.pressure == 0 {
        (0, 0)
    } else {
        (
            TOUCH_MASK_PRESSURE,
            u32::from(point.pressure) * MAX_PRESSURE / u32::from(u16::MAX),
        )
    };
    POINTER_TOUCH_INFO {
        pointerInfo: POINTER_INFO {
            pointerType: PT_TOUCH,
            pointerId: u32::from(point.id),
            pointerFlags: flags(point.phase),
            ptPixelLocation: POINT {
                x: point.x / SUBUNITS,
                y: point.y / SUBUNITS,
            },
            ..POINTER_INFO::default()
        },
        touchMask: touch_mask,
        pressure,
        ..POINTER_TOUCH_INFO::default()
    }
}

/// Performs touch frames, setting up injection on the first.
#[derive(Debug, Default)]
pub(super) struct TouchInjector {
    /// Whether touch injection has been set up.
    initialized: bool,
}

impl TouchInjector {
    /// Performs a touch frame.
    pub(super) fn touch(&mut self, points: &[TouchPoint]) -> Result<(), String> {
        if !self.initialized {
            // SAFETY: the call takes plain values.
            unsafe { InitializeTouchInjection(MAX_CONTACTS as u32, TOUCH_FEEDBACK_DEFAULT) }
                .map_err(|error| format!("cannot inject touches: {error}"))?;
            self.initialized = true;
        }
        let contacts: Vec<POINTER_TOUCH_INFO> = points.iter().map(contact).collect();
        // SAFETY: each contact is a fully initialised `POINTER_TOUCH_INFO`.
        unsafe { InjectTouchInput(&contacts) }
            .map_err(|error| format!("Windows refused the touch: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Points become contacts in pixels, with the flags of their phase and
    /// pressure scaled to Windows's range.
    #[test]
    fn converts_points() {
        let point = TouchPoint {
            id: 3,
            phase: TouchPhase::Down,
            x: 100 * SUBUNITS + 128,
            y: 20 * SUBUNITS,
            pressure: u16::MAX,
        };
        let down = contact(&point);
        assert_eq!(down.pointerInfo.pointerId, 3);
        assert_eq!(down.pointerInfo.ptPixelLocation, POINT { x: 100, y: 20 });
        assert_eq!(
            down.pointerInfo.pointerFlags,
            POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT
        );
        assert_eq!((down.touchMask, down.pressure), (TOUCH_MASK_PRESSURE, 1024));
        let cancelled = contact(&TouchPoint {
            phase: TouchPhase::Cancel,
            pressure: 0,
            ..point
        });
        assert_eq!(
            cancelled.pointerInfo.pointerFlags,
            POINTER_FLAG_UP | POINTER_FLAG_CANCELED
        );
        assert_eq!(cancelled.touchMask, 0);
    }
}
