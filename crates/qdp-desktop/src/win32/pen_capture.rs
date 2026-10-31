//! The client's pen on Windows: the window's pen pointer messages, read
//! with `GetPointerPenInfo` for pressure, tilt, rotation, eraser and barrel.
//!
//! The window is subclassed so that pen messages are taken before the
//! windowing library sees them, which would otherwise report the pen as a
//! touch without its tilt, hover or eraser. Other pointers pass through.

use qdp::message::PenFlags;
use qdp_runtime::PenInput;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::Pointer::{
    GetPointerPenInfo, GetPointerType, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE,
    POINTER_PEN_INFO,
};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    PEN_FLAG_BARREL, PEN_FLAG_ERASER, PEN_FLAG_INVERTED, PEN_MASK_PRESSURE, PEN_MASK_ROTATION,
    PEN_MASK_TILT_X, PEN_MASK_TILT_Y, POINTER_INPUT_TYPE, PT_PEN, WM_POINTERCAPTURECHANGED,
    WM_POINTERDOWN, WM_POINTERENTER, WM_POINTERLEAVE, WM_POINTERUP, WM_POINTERUPDATE,
};

/// The identifier of this subclass of the window.
const SUBCLASS_ID: usize = 0x5144_5050; // "QDPP"

/// The strongest pressure Windows reports.
const MAX_PRESSURE: f64 = 1024.0;

/// The pointer messages a pen sends.
const POINTER_MESSAGES: [u32; 6] = [
    WM_POINTERDOWN,
    WM_POINTERUPDATE,
    WM_POINTERUP,
    WM_POINTERENTER,
    WM_POINTERLEAVE,
    WM_POINTERCAPTURECHANGED,
];

/// Returns the pointer ID a pointer message's `wParam` carries.
fn pointer_id(wparam: WPARAM) -> u32 {
    (wparam.0 & 0xffff) as u32
}

/// Returns a pen's state from its pointer information, at `position` in the
/// window's client area, in physical pixels.
fn pen_input(info: &POINTER_PEN_INFO, position: POINT) -> PenInput {
    let pointer_flags = info.pointerInfo.pointerFlags;
    let in_range = pointer_flags.contains(POINTER_FLAG_INRANGE);
    let in_contact = pointer_flags.contains(POINTER_FLAG_INCONTACT);
    let inverted = info.penFlags & PEN_FLAG_INVERTED != 0;
    let erasing = in_contact && (inverted || info.penFlags & PEN_FLAG_ERASER != 0);
    let flags = [
        (in_range, PenFlags::IN_RANGE),
        (in_contact && !erasing, PenFlags::TIP_SWITCH),
        (
            info.penFlags & PEN_FLAG_BARREL != 0,
            PenFlags::BARREL_SWITCH,
        ),
        (erasing, PenFlags::ERASER),
        (inverted, PenFlags::INVERT),
    ]
    .into_iter()
    .filter(|(set, _)| *set)
    .fold(0, |flags, (_, flag)| flags | flag.0);
    let measured = |mask: u32, value: f64| {
        if info.penMask & mask != 0 { value } else { 0.0 }
    };
    PenInput {
        x: f64::from(position.x),
        y: f64::from(position.y),
        flags: PenFlags(flags),
        pressure: measured(PEN_MASK_PRESSURE, f64::from(info.pressure) / MAX_PRESSURE),
        tilt_x: measured(PEN_MASK_TILT_X, f64::from(info.tiltX)),
        tilt_y: measured(PEN_MASK_TILT_Y, f64::from(info.tiltY)),
        twist: measured(PEN_MASK_ROTATION, f64::from(info.rotation)),
    }
}

/// Where a window's pen goes: called with each change, in the client area's
/// physical pixels.
type PenSink = Box<dyn Fn(PenInput)>;

/// Reads a pen pointer message, returning the pen's state, or `None` if the
/// pointer is not a pen.
///
/// # Safety
///
/// `hwnd` must be a window of this thread.
unsafe fn read_pen(hwnd: HWND, wparam: WPARAM) -> Option<PenInput> {
    let id = pointer_id(wparam);
    let mut kind = POINTER_INPUT_TYPE::default();
    // SAFETY: `kind` is a valid place for the type.
    unsafe { GetPointerType(id, &mut kind) }.ok()?;
    if kind != PT_PEN {
        return None;
    }
    let mut info = POINTER_PEN_INFO::default();
    // SAFETY: `info` is a valid place for the pen's information.
    unsafe { GetPointerPenInfo(id, &mut info) }.ok()?;
    let mut position = info.pointerInfo.ptPixelLocation;
    // SAFETY: `hwnd` is a window of this thread and `position` a valid point.
    if !unsafe { ScreenToClient(hwnd, &mut position) }.as_bool() {
        return None;
    }
    Some(pen_input(&info, position))
}

/// Takes the window's pen messages, passing everything else on.
unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    reference: usize,
) -> LRESULT {
    if POINTER_MESSAGES.contains(&message) {
        // SAFETY: Windows calls the procedure on the window's own thread.
        if let Some(pen) = unsafe { read_pen(hwnd, wparam) } {
            // SAFETY: `reference` is the `PenSink` the `WindowPen` owns, which
            // outlives the subclass.
            let sink = unsafe { &*(reference as *const PenSink) };
            sink(pen);
            return LRESULT(0);
        }
    }
    // SAFETY: the arguments are the message the window received.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// A window's pen, read until dropped.
pub struct WindowPen {
    /// The window.
    hwnd: HWND,
    /// Where the pen goes, kept here because the subclass holds its address.
    _sink: Box<PenSink>,
}

impl std::fmt::Debug for WindowPen {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WindowPen")
            .field("hwnd", &self.hwnd)
            .finish_non_exhaustive()
    }
}

impl WindowPen {
    /// Starts reading the pen over the window `hwnd`, which must belong to
    /// this thread, calling `sink` with each change in the client area's
    /// physical pixels.
    pub fn attach(hwnd: isize, sink: impl Fn(PenInput) + 'static) -> Result<Self, String> {
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        let sink: Box<PenSink> = Box::new(Box::new(sink));
        let reference = &*sink as *const PenSink as usize;
        // SAFETY: `hwnd` is a window of this thread, and `reference` stays
        // valid until `drop` removes the subclass.
        let attached =
            unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, reference) };
        if !attached.as_bool() {
            return Err("cannot read the pen over the window".to_owned());
        }
        Ok(Self { hwnd, _sink: sink })
    }
}

impl Drop for WindowPen {
    fn drop(&mut self) {
        // SAFETY: the subclass was set on this window by `attach`. The sink is
        // dropped after it, once nothing can call it.
        let _ = unsafe { RemoveWindowSubclass(self.hwnd, Some(subclass_proc), SUBCLASS_ID) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::Input::Pointer::{POINTER_FLAG_UPDATE, POINTER_INFO};

    /// Returns pen information with `pointer_flags` and `pen_flags`.
    fn info(
        pointer_flags: windows::Win32::UI::Input::Pointer::POINTER_FLAGS,
        pen_flags: u32,
    ) -> POINTER_PEN_INFO {
        POINTER_PEN_INFO {
            pointerInfo: POINTER_INFO {
                pointerType: PT_PEN,
                pointerFlags: pointer_flags,
                ..POINTER_INFO::default()
            },
            penFlags: pen_flags,
            penMask: PEN_MASK_PRESSURE | PEN_MASK_TILT_X | PEN_MASK_ROTATION,
            pressure: 512,
            rotation: 30,
            tiltX: -20,
            tiltY: 40,
        }
    }

    /// A pen touching reports its tip, pressure, tilt and rotation where
    /// Windows measured them; inverted and touching, it erases.
    #[test]
    fn reads_the_pen() {
        let touching = info(
            POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
            PEN_FLAG_BARREL,
        );
        let pen = pen_input(&touching, POINT { x: 5, y: 6 });
        assert_eq!(
            pen.flags,
            PenFlags(PenFlags::IN_RANGE.0 | PenFlags::TIP_SWITCH.0 | PenFlags::BARREL_SWITCH.0)
        );
        assert_eq!((pen.x, pen.y, pen.pressure), (5.0, 6.0, 0.5));
        // Tilt Y was not measured.
        assert_eq!((pen.tilt_x, pen.tilt_y, pen.twist), (-20.0, 0.0, 30.0));
        let erasing = info(
            POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
            PEN_FLAG_INVERTED,
        );
        assert_eq!(
            pen_input(&erasing, POINT::default()).flags,
            PenFlags(PenFlags::IN_RANGE.0 | PenFlags::ERASER.0 | PenFlags::INVERT.0)
        );
        let left = info(POINTER_FLAG_UPDATE, 0);
        assert_eq!(
            pen_input(&left, POINT::default()).flags,
            PenFlags::default()
        );
    }
}
