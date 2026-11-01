//! What the host's overlays share, the notices and privacy's covers:
//! windows over every other, which clicks pass through, which never take
//! the focus and which screen captures leave out, sized for their display's
//! scale, each on a thread of its own that pumps its messages.

use windows::Win32::Foundation::HINSTANCE;
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, DEFAULT_CHARSET, FF_SWISS, FONT_WEIGHT,
    HFONT, OUT_DEFAULT_PRECIS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, MSG, TranslateMessage, WINDOW_EX_STYLE, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
};
use windows::core::w;

/// The dots per inch of 100 % scale.
pub(super) const BASE_DPI: i32 = 96;

/// The points in an inch, which fonts are sized in.
const POINTS_PER_INCH: i32 = 72;

/// An overlay's extended styles: over every window, with no taskbar
/// button, never activated, and layered and transparent, so that clicks
/// pass through it.
pub(super) const OVERLAY_STYLES: WINDOW_EX_STYLE = WINDOW_EX_STYLE(
    WS_EX_TOPMOST.0
        | WS_EX_TOOLWINDOW.0
        | WS_EX_NOACTIVATE.0
        | WS_EX_LAYERED.0
        | WS_EX_TRANSPARENT.0,
);

/// Returns `pixels` at 100 % scale at `dpi`.
pub(super) fn scaled(pixels: i32, dpi: i32) -> i32 {
    pixels * dpi / BASE_DPI
}

/// Makes Segoe UI, Windows' own typeface, of `points` and `weight` at
/// `dpi`.
pub(super) fn font(points: i32, weight: FONT_WEIGHT, dpi: i32) -> HFONT {
    // SAFETY: the arguments are valid; the call has no other contract.
    unsafe {
        CreateFontW(
            -(points * dpi / POINTS_PER_INCH),
            0,
            0,
            0,
            weight.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            u32::from(FF_SWISS.0),
            w!("Segoe UI"),
        )
    }
}

/// Returns this program's module, which overlays' window classes belong
/// to.
pub(super) fn instance() -> Result<HINSTANCE, String> {
    // SAFETY: the call has no contract.
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|error| format!("cannot find this program's module: {error}"))?;
    Ok(HINSTANCE(module.0))
}

/// Pumps this thread's messages until WM_QUIT, handing those posted to the
/// thread itself, rather than to a window, to `thread_message`.
pub(super) fn pump(mut thread_message: impl FnMut(&MSG)) {
    let mut message = MSG::default();
    // SAFETY: the message is written before each call returns; GetMessageW
    // returns 0 on WM_QUIT and -1 on failure, either of which ends the loop.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        if message.hwnd.is_invalid() {
            thread_message(&message);
            continue;
        }
        // SAFETY: the message came from GetMessageW.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}
