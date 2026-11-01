//! Notices on the host's screen, such as that a device streams it or the
//! code a device pairs with: a banner at the top of the main display, over
//! every window, which clicks pass through and which screen captures leave
//! out, so that whoever is at the computer sees it while the device
//! streaming does not. Windows before 10 version 2004 cannot leave a window
//! out of captures, and stream the notice too.
//!
//! Each notice is a window of a thread of its own, which pumps its messages
//! until the notice is closed or its lifetime ends. Notices shown together
//! stack downwards, and close up as they go.

use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::{Mutex, Once, PoisonError, mpsc};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateCompatibleDC, CreateFontW,
    CreateSolidBrush, DEFAULT_CHARSET, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_NOPREFIX, DT_WORDBREAK,
    DeleteDC, DeleteObject, DrawTextW, EndPaint, FF_SWISS, FONT_WEIGHT, FW_NORMAL, FW_SEMIBOLD,
    FillRect, GetMonitorInfoW, HDC, HFONT, HGDIOBJ, MONITOR_DEFAULTTOPRIMARY, MONITORINFO,
    MonitorFromPoint, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI,
    SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    LWA_ALPHA, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SW_SHOWNOACTIVATE,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetLayeredWindowAttributes,
    SetTimer, SetWindowDisplayAffinity, SetWindowPos, ShowWindow, TranslateMessage,
    WDA_EXCLUDEFROMCAPTURE, WM_CLOSE, WM_DESTROY, WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{PCWSTR, w};

use crate::notification::Urgency;

/// A notice's width, in pixels at 100 % scale.
const WIDTH: i32 = 400;

/// The space around a notice's text, and between a notice and the
/// display's top or the notice above, in pixels at 100 % scale.
const PADDING: i32 = 16;

/// The space between a notice's summary and its body, at 100 % scale.
const LINE_GAP: i32 = 4;

/// The width of the coloured bar down a notice's left side, at 100 % scale.
const BAR_WIDTH: i32 = 4;

/// The summary's size, in points.
const SUMMARY_POINTS: i32 = 11;

/// The body's size, in points.
const BODY_POINTS: i32 = 10;

/// The points in an inch, which fonts are sized in.
const POINTS_PER_INCH: i32 = 72;

/// The dots per inch of 100 % scale.
const BASE_DPI: i32 = 96;

/// A notice's opacity, of 255.
const OPACITY: u8 = 240;

/// A notice's background: a dark grey. Colours are 0x00BBGGRR.
const BACKGROUND: COLORREF = COLORREF(0x0020_2020);

/// The summary's colour: white.
const SUMMARY_COLOR: COLORREF = COLORREF(0x00ff_ffff);

/// The body's colour: a light grey.
const BODY_COLOR: COLORREF = COLORREF(0x00d0_d0d0);

/// The bar's colour on a notice that needs the user now: Windows' red for
/// critical states.
const CRITICAL_BAR: COLORREF = COLORREF(0x001c_2bc4);

/// The bar's colour on other notices: Windows' default accent blue.
const NORMAL_BAR: COLORREF = COLORREF(0x00d4_7800);

/// The timer that ends a notice's lifetime.
const LIFETIME_TIMER: usize = 1;

/// The notices' window class.
const CLASS: PCWSTR = w!("QdpNotice");

/// A notice shown on the screen, until it is closed or its lifetime ends.
#[derive(Debug)]
pub(crate) struct Notice {
    /// Its window, as an integer, as windows' handles may cross threads.
    window: isize,
}

/// A notice in the stack: its window, width and height.
struct Stacked {
    /// Its window, as an integer.
    window: isize,
    /// Its width, in pixels.
    width: i32,
    /// Its height, in pixels.
    height: i32,
}

/// The notices shown, top first.
static STACK: Mutex<Vec<Stacked>> = Mutex::new(Vec::new());

/// What a notice shows, measured for its display, for painting it.
struct Content {
    /// Its summary, in UTF-16.
    summary: Vec<u16>,
    /// Its body, in UTF-16.
    body: Vec<u16>,
    /// The summary's font.
    summary_font: HFONT,
    /// The body's font.
    body_font: HFONT,
    /// The bar's colour.
    bar: COLORREF,
    /// The bar's width, in pixels.
    bar_width: i32,
    /// Where the summary goes.
    summary_rect: RECT,
    /// Where the body goes.
    body_rect: RECT,
}

impl Drop for Content {
    fn drop(&mut self) {
        // SAFETY: the fonts were made for this content and are selected into
        // no device context any more.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.summary_font.0));
            let _ = DeleteObject(HGDIOBJ(self.body_font.0));
        }
    }
}

thread_local! {
    /// What this thread's notice shows, while it shows.
    static CONTENT: RefCell<Option<Content>> = const { RefCell::new(None) };
}

impl Notice {
    /// Shows a notice titled `summary` saying `body`, for `lifetime` at
    /// most, with a red bar if `urgency` is critical and a blue one
    /// otherwise.
    ///
    /// # Errors
    ///
    /// Fails if the notice's thread or window cannot be made, as without a
    /// desktop to show it on.
    pub(crate) fn show(
        summary: &str,
        body: &str,
        lifetime: Duration,
        urgency: Urgency,
    ) -> Result<Self, String> {
        let summary: Vec<u16> = summary.encode_utf16().collect();
        let body: Vec<u16> = body.encode_utf16().collect();
        let (created, creation) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("qdp-notice".to_owned())
            .spawn(move || {
                let window = match open(summary, body, lifetime, urgency) {
                    Ok(window) => window,
                    Err(error) => {
                        let _ = created.send(Err(error));
                        return;
                    }
                };
                let _ = created.send(Ok(window.0 as isize));
                pump();
                CONTENT.with_borrow_mut(Option::take);
            })
            .map_err(|error| format!("cannot start the notice's thread: {error}"))?;
        let window = creation
            .recv()
            .map_err(|_| "the notice's thread ended".to_owned())??;
        Ok(Self { window })
    }

    /// Takes the notice away, if it still shows.
    pub(crate) fn close(self) {
        // Held while posting, so that the window cannot be destroyed, and
        // its handle reused, meanwhile.
        let stack = STACK.lock().unwrap_or_else(PoisonError::into_inner);
        if stack.iter().any(|stacked| stacked.window == self.window) {
            // SAFETY: the window still shows; posting has no other contract.
            let _ = unsafe {
                PostMessageW(
                    Some(HWND(self.window as *mut c_void)),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                )
            };
        }
    }
}

/// Registers the notices' window class once, returning this module's
/// instance.
fn register() -> Result<HINSTANCE, String> {
    static REGISTER: Once = Once::new();
    // SAFETY: the call has no contract.
    let module = unsafe { GetModuleHandleW(None) }
        .map_err(|error| format!("cannot find this program's module: {error}"))?;
    let instance = HINSTANCE(module.0);
    REGISTER.call_once(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance,
            lpszClassName: CLASS,
            ..WNDCLASSW::default()
        };
        // SAFETY: the class is valid; a failure shows as the window failing.
        unsafe { RegisterClassW(&class) };
    });
    Ok(instance)
}

/// Returns the main display's work area, out of the taskbar's way, and its
/// dots per inch.
fn primary_work_area() -> (RECT, i32) {
    // SAFETY: the calls have no contract; the information is written before
    // they return.
    unsafe {
        let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
        let mut information = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..MONITORINFO::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut information);
        let (mut dpi, mut vertical_dpi) = (BASE_DPI as u32, BASE_DPI as u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut vertical_dpi);
        (information.rcWork, dpi as i32)
    }
}

/// Returns `pixels` at 100 % scale at `dpi`.
fn scaled(pixels: i32, dpi: i32) -> i32 {
    pixels * dpi / BASE_DPI
}

/// Makes Segoe UI of `points` and `weight` at `dpi`.
fn font(points: i32, weight: FONT_WEIGHT, dpi: i32) -> HFONT {
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

/// The text drawing a notice's text wraps.
const TEXT_FORMAT: DRAW_TEXT_FORMAT = DRAW_TEXT_FORMAT(DT_WORDBREAK.0 | DT_NOPREFIX.0);

/// Returns the height of `text` in `font`, wrapped to `width`, measured on
/// `dc`.
fn text_height(dc: HDC, font: HFONT, text: &mut [u16], width: i32) -> i32 {
    let mut rect = RECT {
        right: width,
        ..RECT::default()
    };
    // SAFETY: the device context and font are valid; the rectangle is
    // written before the call returns.
    unsafe {
        let previous = SelectObject(dc, HGDIOBJ(font.0));
        DrawTextW(dc, text, &mut rect, TEXT_FORMAT | DT_CALCRECT);
        SelectObject(dc, previous);
    }
    rect.bottom
}

/// Returns where the notice below those in `stack` goes, from `top`.
fn next_top(stack: &[Stacked], top: i32, dpi: i32) -> i32 {
    stack
        .iter()
        .fold(top + scaled(PADDING, dpi), |below, stacked| {
            below + stacked.height + scaled(PADDING, dpi)
        })
}

/// Opens a notice's window on this thread, which must then pump its
/// messages.
fn open(
    mut summary: Vec<u16>,
    mut body: Vec<u16>,
    lifetime: Duration,
    urgency: Urgency,
) -> Result<HWND, String> {
    // SAFETY: the call has no contract; it returns the previous context.
    let _previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let instance = register()?;
    let (work, dpi) = primary_work_area();
    let width = scaled(WIDTH, dpi);
    let text_left = scaled(BAR_WIDTH + PADDING, dpi);
    let text_width = width - text_left - scaled(PADDING, dpi);
    let summary_font = font(SUMMARY_POINTS, FW_SEMIBOLD, dpi);
    let body_font = font(BODY_POINTS, FW_NORMAL, dpi);
    // SAFETY: a memory device context for measuring, deleted below.
    let dc = unsafe { CreateCompatibleDC(None) };
    let summary_height = text_height(dc, summary_font, &mut summary, text_width);
    let body_height = text_height(dc, body_font, &mut body, text_width);
    // SAFETY: the context was made above.
    let _ = unsafe { DeleteDC(dc) };
    let summary_top = scaled(PADDING, dpi);
    let body_top = summary_top + summary_height + scaled(LINE_GAP, dpi);
    let height = body_top + body_height + scaled(PADDING, dpi);
    let content = Content {
        summary,
        body,
        summary_font,
        body_font,
        bar: match urgency {
            Urgency::Critical => CRITICAL_BAR,
            Urgency::Normal => NORMAL_BAR,
        },
        bar_width: scaled(BAR_WIDTH, dpi),
        summary_rect: RECT {
            left: text_left,
            top: summary_top,
            right: text_left + text_width,
            bottom: summary_top + summary_height,
        },
        body_rect: RECT {
            left: text_left,
            top: body_top,
            right: text_left + text_width,
            bottom: body_top + body_height,
        },
    };
    CONTENT.set(Some(content));

    let mut stack = STACK.lock().unwrap_or_else(PoisonError::into_inner);
    let left = work.left + (work.right - work.left - width) / 2;
    let top = next_top(&stack, work.top, dpi);
    // SAFETY: the class is registered, and the arguments are valid.
    let window = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
            CLASS,
            w!("QDP"),
            WS_POPUP,
            left,
            top,
            width,
            height,
            None,
            None,
            Some(instance),
            None,
        )
    }
    .map_err(|error| format!("cannot open the notice's window: {error}"))?;
    let preference: DWM_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND;
    let lifetime_millis = u32::try_from(lifetime.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: the window was made on this thread; the preference outlives
    // the call. Each call failing leaves the notice as it was: opaque,
    // square, captured or without a lifetime, which the caller's closing
    // still ends.
    unsafe {
        let _ = SetLayeredWindowAttributes(window, COLORREF(0), OPACITY, LWA_ALPHA);
        let _ = SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE);
        let _ = DwmSetWindowAttribute(
            window,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            std::ptr::from_ref(&preference).cast(),
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        );
        SetTimer(Some(window), LIFETIME_TIMER, lifetime_millis, None);
    }
    stack.push(Stacked {
        window: window.0 as isize,
        width,
        height,
    });
    drop(stack);
    // SAFETY: the window was made on this thread.
    let _ = unsafe { ShowWindow(window, SW_SHOWNOACTIVATE) };
    Ok(window)
}

/// Pumps this thread's messages until its notice is destroyed.
fn pump() {
    let mut message = MSG::default();
    // SAFETY: the message is written before each call returns; GetMessageW
    // returns 0 on WM_QUIT and -1 on failure, either of which ends the loop.
    while unsafe { GetMessageW(&mut message, None, 0, 0) }.0 > 0 {
        // SAFETY: the message came from GetMessageW.
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

/// Takes `window` out of the stack, and moves those below it up.
fn unstack(window: HWND) {
    let mut stack = STACK.lock().unwrap_or_else(PoisonError::into_inner);
    stack.retain(|stacked| stacked.window != window.0 as isize);
    let (work, dpi) = primary_work_area();
    for index in 0..stack.len() {
        let top = next_top(&stack[..index], work.top, dpi);
        let stacked = &stack[index];
        let left = work.left + (work.right - work.left - stacked.width) / 2;
        // SAFETY: the window still shows, on another thread, which is not
        // waited for.
        let _ = unsafe {
            SetWindowPos(
                HWND(stacked.window as *mut c_void),
                None,
                left,
                top,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
            )
        };
    }
}

/// Paints this thread's notice into `window`.
fn paint(window: HWND) {
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: the window is this thread's, painting in response to
    // WM_PAINT; the brushes are deleted after use.
    unsafe {
        let dc = BeginPaint(window, &mut paint);
        CONTENT.with_borrow_mut(|content| {
            let Some(content) = content else {
                return;
            };
            let mut client = RECT::default();
            let _ = GetClientRect(window, &mut client);
            let background = CreateSolidBrush(BACKGROUND);
            FillRect(dc, &client, background);
            let _ = DeleteObject(HGDIOBJ(background.0));
            let bar = CreateSolidBrush(content.bar);
            let bar_rect = RECT {
                right: content.bar_width,
                ..client
            };
            FillRect(dc, &bar_rect, bar);
            let _ = DeleteObject(HGDIOBJ(bar.0));
            SetBkMode(dc, TRANSPARENT);
            let previous = SelectObject(dc, HGDIOBJ(content.summary_font.0));
            SetTextColor(dc, SUMMARY_COLOR);
            let mut rect = content.summary_rect;
            DrawTextW(dc, &mut content.summary, &mut rect, TEXT_FORMAT);
            SelectObject(dc, HGDIOBJ(content.body_font.0));
            SetTextColor(dc, BODY_COLOR);
            let mut rect = content.body_rect;
            DrawTextW(dc, &mut content.body, &mut rect, TEXT_FORMAT);
            SelectObject(dc, previous);
        });
        let _ = EndPaint(window, &paint);
    }
}

/// The notices' window procedure.
extern "system" fn procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_PAINT => {
            paint(window);
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == LIFETIME_TIMER => {
            // SAFETY: the window is this thread's.
            let _ = unsafe { DestroyWindow(window) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unstack(window);
            // SAFETY: the call has no contract.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: the arguments are the procedure's own.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use windows::Win32::UI::WindowsAndMessaging::{
        GWL_EXSTYLE, GetWindowDisplayAffinity, GetWindowLongW, GetWindowRect, IsWindow,
        IsWindowVisible,
    };

    /// The tests share the stack, so run one at a time.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// How long a test waits for a window to change.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// Returns `notice`'s window.
    fn window(notice: &Notice) -> HWND {
        HWND(notice.window as *mut c_void)
    }

    /// Returns `window`'s rectangle on the screen.
    fn rect(window: HWND) -> RECT {
        let mut rect = RECT::default();
        // SAFETY: the rectangle is written before the call returns.
        unsafe { GetWindowRect(window, &mut rect) }.unwrap();
        rect
    }

    /// Waits until `done` holds, failing after a while.
    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < PATIENCE, "{what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn shows_over_everything_out_of_captures() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let notice = Notice::show(
            "This computer is being streamed",
            "The device at 192.0.2.7 sees this screen and controls it.",
            Duration::from_secs(60),
            Urgency::Critical,
        )
        .unwrap();
        let shown = window(&notice);
        // SAFETY: the window is valid; the calls have no other contract.
        unsafe {
            wait_until("the notice never showed", || {
                IsWindowVisible(shown).as_bool()
            });
            let styles = GetWindowLongW(shown, GWL_EXSTYLE) as u32;
            for style in [WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_EX_NOACTIVATE] {
                assert_ne!(styles & style.0, 0, "missing {style:?}");
            }
            let mut affinity = 0;
            GetWindowDisplayAffinity(shown, &mut affinity).unwrap();
            assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE.0);
        }
        let (work, _) = primary_work_area();
        let placed = rect(shown);
        assert!(
            placed.top > work.top && placed.bottom < work.bottom,
            "{placed:?}"
        );
        let centre = (placed.left + placed.right) / 2;
        assert!(
            (centre - (work.left + work.right) / 2).abs() <= 1,
            "{placed:?}"
        );
        notice.close();
        // SAFETY: as above.
        wait_until("the notice never closed", || unsafe {
            !IsWindow(Some(shown)).as_bool()
        });
    }

    #[test]
    fn stacks_and_closes_up() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let first = Notice::show(
            "Pair Phone?",
            "Type 1234 on Phone.",
            Duration::from_secs(60),
            Urgency::Critical,
        )
        .unwrap();
        let second = Notice::show(
            "Paired with Tablet",
            "It connects without a code.",
            Duration::from_secs(60),
            Urgency::Normal,
        )
        .unwrap();
        let (top, below) = (window(&first), window(&second));
        let first_top = rect(top).top;
        assert!(
            rect(below).top > rect(top).bottom,
            "the second notice is not below the first"
        );
        first.close();
        wait_until("the second notice did not move up", || {
            rect(below).top == first_top
        });
        second.close();
    }

    #[test]
    fn ends_with_its_lifetime() {
        let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        let notice = Notice::show(
            "Paired with Phone",
            "It connects without a code.",
            Duration::from_millis(100),
            Urgency::Normal,
        )
        .unwrap();
        let shown = window(&notice);
        // SAFETY: the call has no contract.
        wait_until("the notice outlived its lifetime", || unsafe {
            !IsWindow(Some(shown)).as_bool()
        });
        // Closing one already gone does nothing.
        notice.close();
    }
}
