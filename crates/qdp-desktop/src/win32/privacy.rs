//! Privacy for whoever is at a host while a session streams its screen
//! (Section 11.4): every display covered in black, saying that the computer
//! is in use from another device, and the computer's own keyboard and mouse
//! held back, so that only the device streaming sees and controls it.
//!
//! The covers are overlays: windows over every other, which clicks pass
//! through and which screen captures leave out, so that the stream still
//! shows the desktop beneath them. Windows before 10 version 2004 cannot
//! leave a window out of captures, so privacy does not start there, as its
//! covers would hide the desktop from the stream too. Low-level keyboard
//! and mouse hooks hold back every event but those injected, as a
//! session's are. Ctrl+Alt+Delete, which no program can hold back, still
//! reaches Windows, which shows its secure screen over the covers, from
//! which the computer can be locked.
//!
//! The covers and hooks live on a thread of their own, which pumps their
//! messages; the covers follow the displays as they change.

use std::cell::Cell;
use std::sync::{Once, mpsc};
use std::thread::{self, JoinHandle};

use tracing::warn;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DRAW_TEXT_FORMAT, DT_CALCRECT, DT_CENTER, DT_NOPREFIX,
    DT_WORDBREAK, DeleteObject, DrawTextW, EndPaint, EnumDisplayMonitors, FW_LIGHT, FW_NORMAL,
    FillRect, HDC, HFONT, HGDIOBJ, HMONITOR, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetThreadDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
    GetWindowDisplayAffinity, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED,
    LWA_ALPHA, MSLLHOOKSTRUCT, PostThreadMessageW, RegisterClassW, SW_SHOWNOACTIVATE,
    SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowsHookExW, ShowWindow,
    UnhookWindowsHookEx, WDA_EXCLUDEFROMCAPTURE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP,
    WM_DISPLAYCHANGE, WM_PAINT, WM_QUIT, WNDCLASSW, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, w};

use super::overlay::{self, OVERLAY_STYLES, scaled};

/// The covers' window class.
const CLASS: PCWSTR = w!("QdpPrivacyCover");

/// What the covers say, in large type.
const TITLE: &str = "This computer is in use from another device";

/// What the covers say beneath.
const DETAIL: &str = "Its keyboard and mouse are off until the stream ends. \
                      Ctrl+Alt+Delete still works.";

/// The title's size, in points.
const TITLE_POINTS: i32 = 24;

/// The detail's size, in points.
const DETAIL_POINTS: i32 = 12;

/// The space between the title and the detail, at 100 % scale.
const LINE_GAP: i32 = 12;

/// The widest the text runs, at 100 % scale.
const TEXT_WIDTH: i32 = 720;

/// The least space beside the text, at 100 % scale.
const TEXT_MARGIN: i32 = 32;

/// The covers' colour: black. Colours are 0x00BBGGRR.
const BACKGROUND: COLORREF = COLORREF(0);

/// The title's colour: white.
const TITLE_COLOR: COLORREF = COLORREF(0x00ff_ffff);

/// The detail's colour: a grey.
const DETAIL_COLOR: COLORREF = COLORREF(0x00a0_a0a0);

/// The covers' opacity: opaque.
const OPAQUE: u8 = 255;

/// The message privacy's thread posts itself to cover the displays again
/// once they changed.
const RECOVER: u32 = WM_APP;

/// The text drawing the covers' text, centred and wrapped.
const TEXT_FORMAT: DRAW_TEXT_FORMAT =
    DRAW_TEXT_FORMAT(DT_CENTER.0 | DT_WORDBREAK.0 | DT_NOPREFIX.0);

/// Where an input event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// The computer's own keyboard or mouse.
    Local,
    /// A program, as a session injects its input.
    Injected,
}

/// Returns whether an input event from `origin` is held back from Windows:
/// those of the computer's own keyboard and mouse are.
fn holds_back(origin: Origin) -> bool {
    origin == Origin::Local
}

thread_local! {
    /// Whether this thread is about to cover the displays again.
    static RECOVERING: Cell<bool> = const { Cell::new(false) };
}

/// The host's displays covered, and its own keyboard and mouse held back,
/// until dropped.
#[derive(Debug)]
pub struct Privacy {
    /// The thread of the covers and hooks.
    thread_id: u32,
    /// That thread, joined as privacy ends.
    thread: Option<JoinHandle<()>>,
}

impl Privacy {
    /// Covers the host's displays and holds back its keyboard and mouse.
    ///
    /// # Errors
    ///
    /// Fails if Windows cannot leave the covers out of screen captures, as
    /// before Windows 10 version 2004, or if the covers or the hooks cannot
    /// be made.
    pub fn start() -> Result<Self, String> {
        let (started, start) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("qdp-privacy".to_owned())
            .spawn(move || run(&started))
            .map_err(|error| format!("cannot start privacy's thread: {error}"))?;
        let started = start
            .recv()
            .unwrap_or_else(|_| Err("privacy's thread ended".to_owned()));
        match started {
            Ok(thread_id) => Ok(Self {
                thread_id,
                thread: Some(thread),
            }),
            Err(error) => {
                let _ = thread.join();
                Err(error)
            }
        }
    }
}

impl Drop for Privacy {
    fn drop(&mut self) {
        // SAFETY: the thread made its message queue with its covers, before
        // it said it started; posting has no other contract.
        let _ = unsafe { PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Covers the displays and holds back the keyboard and mouse until this
/// thread is told to quit, saying through `started` whether it could, with
/// its thread's identifier.
fn run(started: &mpsc::SyncSender<Result<u32, String>>) {
    // SAFETY: the call has no contract; it returns the previous context.
    let _previous =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let mut covers = match Covers::open() {
        Ok(covers) => covers,
        Err(error) => {
            let _ = started.send(Err(error));
            return;
        }
    };
    let hooks = match Hooks::install() {
        Ok(hooks) => hooks,
        Err(error) => {
            let _ = started.send(Err(error));
            return;
        }
    };
    // SAFETY: the call has no contract.
    let _ = started.send(Ok(unsafe { GetCurrentThreadId() }));
    overlay::pump(|message| {
        if message.message == RECOVER {
            RECOVERING.set(false);
            if let Err(error) = covers.cover() {
                warn!(%error, "cannot cover the displays again");
            }
        }
    });
    drop(hooks);
    drop(covers);
}

/// Registers the covers' window class once, returning this program's
/// instance.
fn register() -> Result<HINSTANCE, String> {
    static REGISTER: Once = Once::new();
    let instance = overlay::instance()?;
    REGISTER.call_once(|| {
        let class = WNDCLASSW {
            lpfnWndProc: Some(procedure),
            hInstance: instance,
            lpszClassName: CLASS,
            ..WNDCLASSW::default()
        };
        // SAFETY: the class is valid; a failure shows as the windows failing.
        unsafe { RegisterClassW(&class) };
    });
    Ok(instance)
}

/// Returns the rectangles of the displays, on the virtual screen.
fn displays() -> Vec<RECT> {
    /// Adds each display's rectangle to the vector `data` points to.
    unsafe extern "system" fn collect(
        _monitor: HMONITOR,
        _dc: HDC,
        rect: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        // SAFETY: `data` is the vector below, borrowed for the call, and
        // `rect` the display's rectangle.
        unsafe { (*(data.0 as *mut Vec<RECT>)).push(*rect) };
        BOOL::from(true)
    }
    let mut displays: Vec<RECT> = Vec::new();
    // SAFETY: the callback runs only during the call, while the vector
    // lives.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(std::ptr::from_mut(&mut displays) as isize),
        )
    };
    displays
}

/// A cover over each display.
struct Covers {
    /// This program's instance, which the covers' class belongs to.
    instance: HINSTANCE,
    /// The covers.
    windows: Vec<HWND>,
}

impl Covers {
    /// Covers each display.
    fn open() -> Result<Self, String> {
        let mut covers = Self {
            instance: register()?,
            windows: Vec::new(),
        };
        covers.cover()?;
        Ok(covers)
    }

    /// Covers each display, in place of any covers before, showing each
    /// only once Windows leaves it out of captures.
    fn cover(&mut self) -> Result<(), String> {
        self.uncover();
        for display in displays() {
            // SAFETY: the class is registered, and the arguments are valid.
            let window = unsafe {
                CreateWindowExW(
                    OVERLAY_STYLES,
                    CLASS,
                    w!("QDP"),
                    WS_POPUP,
                    display.left,
                    display.top,
                    display.right - display.left,
                    display.bottom - display.top,
                    None,
                    None,
                    Some(self.instance),
                    None,
                )
            }
            .map_err(|error| format!("cannot cover a display: {error}"))?;
            self.windows.push(window);
            let mut affinity = 0;
            // SAFETY: the window was made on this thread; the affinity is
            // written before the call returns.
            unsafe {
                SetLayeredWindowAttributes(window, COLORREF(0), OPAQUE, LWA_ALPHA)
                    .map_err(|error| format!("cannot make a cover opaque: {error}"))?;
                let excluded = SetWindowDisplayAffinity(window, WDA_EXCLUDEFROMCAPTURE)
                    .and_then(|()| GetWindowDisplayAffinity(window, &mut affinity));
                if excluded.is_err() || affinity != WDA_EXCLUDEFROMCAPTURE.0 {
                    return Err(
                        "this version of Windows cannot leave the covers out of the \
                                stream: privacy needs Windows 10 version 2004 or later"
                            .to_owned(),
                    );
                }
                let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            }
        }
        Ok(())
    }

    /// Takes the covers away.
    fn uncover(&mut self) {
        for window in self.windows.drain(..) {
            // SAFETY: the window was made on this thread.
            let _ = unsafe { DestroyWindow(window) };
        }
    }
}

impl Drop for Covers {
    fn drop(&mut self) {
        self.uncover();
    }
}

/// The hooks holding back the computer's own keyboard and mouse.
struct Hooks {
    /// The keyboard's.
    keyboard: HHOOK,
    /// The mouse's.
    mouse: HHOOK,
}

impl Hooks {
    /// Installs the hooks, on this thread, which must pump messages.
    fn install() -> Result<Self, String> {
        // SAFETY: the procedures live as long as the program; low-level
        // hooks need no module, and run on this thread.
        let keyboard = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0) }
            .map_err(|error| format!("cannot hold back the keyboard: {error}"))?;
        // SAFETY: as above.
        let mouse = match unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) } {
            Ok(mouse) => mouse,
            Err(error) => {
                // SAFETY: the hook was installed above.
                let _ = unsafe { UnhookWindowsHookEx(keyboard) };
                return Err(format!("cannot hold back the mouse: {error}"));
            }
        };
        Ok(Self { keyboard, mouse })
    }
}

impl Drop for Hooks {
    fn drop(&mut self) {
        // SAFETY: the hooks were installed on this thread.
        unsafe {
            let _ = UnhookWindowsHookEx(self.keyboard);
            let _ = UnhookWindowsHookEx(self.mouse);
        }
    }
}

/// The keyboard's hook: holds back the computer's own keys.
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to the event.
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let origin = if event.flags.contains(LLKHF_INJECTED) {
            Origin::Injected
        } else {
            Origin::Local
        };
        if holds_back(origin) {
            return LRESULT(1);
        }
    }
    // SAFETY: the arguments are the hook's own.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// The mouse's hook: holds back the computer's own mouse.
unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to the event.
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let origin = if event.flags & LLMHF_INJECTED != 0 {
            Origin::Injected
        } else {
            Origin::Local
        };
        if holds_back(origin) {
            return LRESULT(1);
        }
    }
    // SAFETY: the arguments are the hook's own.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Returns the height of `text` in `font`, centred and wrapped to `width`,
/// measured on `dc`.
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

/// Paints a cover: black, with what it says in the middle.
fn paint(window: HWND) {
    let mut paint = PAINTSTRUCT::default();
    // SAFETY: the window is this thread's, painting in response to
    // WM_PAINT; the brush and fonts are deleted after use.
    unsafe {
        let dc = BeginPaint(window, &mut paint);
        let mut client = RECT::default();
        let _ = GetClientRect(window, &mut client);
        let background = CreateSolidBrush(BACKGROUND);
        FillRect(dc, &client, background);
        let _ = DeleteObject(HGDIOBJ(background.0));

        let dpi = GetDpiForWindow(window) as i32;
        let title_font = overlay::font(TITLE_POINTS, FW_LIGHT, dpi);
        let detail_font = overlay::font(DETAIL_POINTS, FW_NORMAL, dpi);
        let width = scaled(TEXT_WIDTH, dpi).min(client.right - 2 * scaled(TEXT_MARGIN, dpi));
        let mut title: Vec<u16> = TITLE.encode_utf16().collect();
        let mut detail: Vec<u16> = DETAIL.encode_utf16().collect();
        let title_height = text_height(dc, title_font, &mut title, width);
        let detail_height = text_height(dc, detail_font, &mut detail, width);
        let gap = scaled(LINE_GAP, dpi);
        let left = (client.right - width) / 2;
        let top = (client.bottom - title_height - gap - detail_height) / 2;

        SetBkMode(dc, TRANSPARENT);
        let previous = SelectObject(dc, HGDIOBJ(title_font.0));
        SetTextColor(dc, TITLE_COLOR);
        let mut rect = RECT {
            left,
            top,
            right: left + width,
            bottom: top + title_height,
        };
        DrawTextW(dc, &mut title, &mut rect, TEXT_FORMAT);
        SelectObject(dc, HGDIOBJ(detail_font.0));
        SetTextColor(dc, DETAIL_COLOR);
        let detail_top = top + title_height + gap;
        let mut rect = RECT {
            left,
            top: detail_top,
            right: left + width,
            bottom: detail_top + detail_height,
        };
        DrawTextW(dc, &mut detail, &mut rect, TEXT_FORMAT);
        SelectObject(dc, previous);
        let _ = DeleteObject(HGDIOBJ(title_font.0));
        let _ = DeleteObject(HGDIOBJ(detail_font.0));
        let _ = EndPaint(window, &paint);
    }
}

/// The covers' window procedure.
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
        WM_DISPLAYCHANGE => {
            // Every cover hears of the change; the displays are covered
            // again once.
            if !RECOVERING.replace(true) {
                // SAFETY: the calls have no contract.
                let _ = unsafe {
                    PostThreadMessageW(GetCurrentThreadId(), RECOVER, WPARAM(0), LPARAM(0))
                };
            }
            LRESULT(0)
        }
        // SAFETY: the arguments are the procedure's own.
        _ => unsafe { DefWindowProcW(window, message, wparam, lparam) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
        KEYEVENTF_KEYUP, SendInput, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetWindowRect, IsWindowVisible};

    /// How long a test waits for Windows to catch up.
    const PATIENCE: Duration = Duration::from_secs(5);

    /// The bit `GetAsyncKeyState` sets while a key is down.
    const KEY_DOWN: i16 = i16::MIN;

    /// Waits until `done` holds, failing after a while.
    fn wait_until(what: &str, done: impl Fn() -> bool) {
        let started = Instant::now();
        while !done() {
            assert!(started.elapsed() < PATIENCE, "{what}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Injects Shift, pressed or released, as a session would.
    fn inject_shift(pressed: bool) {
        let flags = if pressed {
            KEYBD_EVENT_FLAGS(0)
        } else {
            KEYEVENTF_KEYUP
        };
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_SHIFT,
                    dwFlags: flags,
                    ..KEYBDINPUT::default()
                },
            },
        };
        // SAFETY: the input is valid for the call.
        unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    }

    /// Returns whether Shift is down.
    fn shift_down() -> bool {
        // SAFETY: the call takes any virtual key.
        let state = unsafe { GetAsyncKeyState(i32::from(VK_SHIFT.0)) };
        state & KEY_DOWN != 0
    }

    #[test]
    fn holds_back_only_the_computers_own_input() {
        assert!(holds_back(Origin::Local));
        assert!(!holds_back(Origin::Injected));
    }

    #[test]
    fn covers_the_displays_out_of_captures_while_on() {
        let privacy = Privacy::start().unwrap();
        // SAFETY: the calls have no contract beyond valid arguments; the
        // rectangle and affinity are written before they return.
        unsafe {
            let cover = FindWindowW(CLASS, None).unwrap();
            assert!(IsWindowVisible(cover).as_bool());
            let mut affinity = 0;
            GetWindowDisplayAffinity(cover, &mut affinity).unwrap();
            assert_eq!(affinity, WDA_EXCLUDEFROMCAPTURE.0);
            let mut placed = RECT::default();
            GetWindowRect(cover, &mut placed).unwrap();
            assert!(displays().contains(&placed), "{placed:?} covers no display");
        }

        // What a session injects still reaches Windows.
        inject_shift(true);
        wait_until("an injected key was held back", shift_down);
        inject_shift(false);
        wait_until("an injected release was held back", || !shift_down());

        drop(privacy);
        // SAFETY: as above.
        assert!(
            unsafe { FindWindowW(CLASS, None) }.is_err(),
            "a cover stayed"
        );
    }
}
