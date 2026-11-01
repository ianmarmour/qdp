//! Windows' own shortcuts, sent to the host instead while a stream window is
//! in front: the Windows key, and Alt+Tab, Alt+Esc, Ctrl+Esc, Alt+F4 and
//! Alt+Space, act on the host's desktop, as they would from a keyboard
//! plugged into it (Section 8.4.1).
//!
//! Windows acts on them before any window sees them, so a low-level keyboard
//! hook takes them first: on the thread that installed it, which must pump
//! messages, as a window's thread does. The hook holds back only these
//! keys; the modifiers held with them reach the window as usual.

use std::cell::RefCell;
use std::collections::HashSet;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_F4, VK_LWIN, VK_RWIN, VK_SPACE, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, LLKHF_ALTDOWN,
    LLKHF_INJECTED, LLKHF_UP, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL,
};

/// The bit `GetAsyncKeyState` sets while a key is down.
const KEY_DOWN: i16 = i16::MIN;

/// The HID usages, on the Keyboard/Keypad page, of the keys the hook holds
/// back.
const USAGES: [(VIRTUAL_KEY, u8); 6] = [
    (VK_LWIN, 0xe3),
    (VK_RWIN, 0xe7),
    (VK_TAB, 0x2b),
    (VK_ESCAPE, 0x29),
    (VK_F4, 0x3d),
    (VK_SPACE, 0x2c),
];

/// Where the shortcuts go, on the thread that installed the hook.
struct Target {
    /// The stream window, which must be in front.
    hwnd: HWND,
    /// Whether the shortcuts go to the host now.
    active: bool,
    /// Takes each key held back: its HID usage and whether it was pressed.
    sink: Box<dyn Fn(u8, bool)>,
    /// The keys held back while down, whose releases are held back too.
    held: HashSet<u32>,
}

thread_local! {
    /// The hook's target on this thread, while one is installed.
    static TARGET: RefCell<Option<Target>> = const { RefCell::new(None) };
}

impl Target {
    /// Returns whether the key event `key` is held back from Windows,
    /// telling the sink of it.
    fn key(&mut self, key: &KBDLLHOOKSTRUCT) -> bool {
        if key.flags.contains(LLKHF_INJECTED) {
            return false;
        }
        let pressed = !key.flags.contains(LLKHF_UP);
        let usage = USAGES
            .iter()
            .find(|(virtual_key, _)| u32::from(virtual_key.0) == key.vkCode)
            .map(|&(_, usage)| usage);
        let Some(usage) = usage else {
            return false;
        };
        if !pressed {
            if self.held.remove(&key.vkCode) {
                (self.sink)(usage, false);
                return true;
            }
            return false;
        }
        if self.held.contains(&key.vkCode) {
            // A repeat, which the host makes itself.
            return true;
        }
        // SAFETY: the call has no contract.
        let in_front = unsafe { GetForegroundWindow() } == self.hwnd;
        if !self.active || !in_front || !is_shortcut(key) {
            return false;
        }
        self.held.insert(key.vkCode);
        (self.sink)(usage, true);
        true
    }
}

/// Returns whether pressing `key` is one of Windows' shortcuts, given the
/// modifiers held.
fn is_shortcut(key: &KBDLLHOOKSTRUCT) -> bool {
    let alt = key.flags.contains(LLKHF_ALTDOWN);
    // SAFETY: the call has no contract.
    let control = unsafe { GetAsyncKeyState(i32::from(VK_CONTROL.0)) } & KEY_DOWN != 0;
    let virtual_key = key.vkCode;
    let is = |candidate: VIRTUAL_KEY| u32::from(candidate.0) == virtual_key;
    is(VK_LWIN)
        || is(VK_RWIN)
        || (alt && (is(VK_TAB) || is(VK_ESCAPE) || is(VK_F4) || is(VK_SPACE)))
        || (control && is(VK_ESCAPE))
}

/// The low-level keyboard hook.
unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, `lparam` points to the event's
        // KBDLLHOOKSTRUCT for the duration of the call.
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let held_back = TARGET.with(|target| {
            target
                .try_borrow_mut()
                .ok()
                .and_then(|mut target| target.as_mut().map(|target| target.key(key)))
                .unwrap_or(false)
        });
        if held_back {
            return LRESULT(1);
        }
    }
    // SAFETY: passing the event on, as every hook must.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Windows' shortcuts, taken for a stream window while installed: one at a
/// time on a thread, on the window's thread.
pub struct SystemKeys {
    /// The hook.
    hook: HHOOK,
}

impl std::fmt::Debug for SystemKeys {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("SystemKeys").finish_non_exhaustive()
    }
}

impl SystemKeys {
    /// Starts taking Windows' shortcuts while the window `hwnd` is in front
    /// and [`set_active`](Self::set_active) allows, telling `sink` of each
    /// key held back: its HID usage and whether it was pressed.
    pub fn capture(hwnd: isize, sink: impl Fn(u8, bool) + 'static) -> Result<Self, String> {
        let installed = TARGET.with(|target| target.borrow().is_some());
        if installed {
            return Err("this thread takes Windows' shortcuts already".to_owned());
        }
        // SAFETY: the procedure is a valid hook for the whole time it is
        // installed; a low-level hook needs no module or thread.
        let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0) }
            .map_err(|error| format!("cannot take Windows' shortcuts: {error}"))?;
        TARGET.with(|target| {
            *target.borrow_mut() = Some(Target {
                hwnd: HWND(hwnd as *mut _),
                active: false,
                sink: Box::new(sink),
                held: HashSet::new(),
            });
        });
        Ok(Self { hook })
    }

    /// Sends the shortcuts to the host, or leaves them to Windows.
    pub fn set_active(&self, active: bool) {
        TARGET.with(|target| {
            if let Some(target) = target.borrow_mut().as_mut() {
                target.active = active;
            }
        });
    }
}

impl Drop for SystemKeys {
    fn drop(&mut self) {
        TARGET.with(|target| target.borrow_mut().take());
        // SAFETY: the hook was installed by this value, on this thread.
        if let Err(error) = unsafe { UnhookWindowsHookEx(self.hook) } {
            tracing::warn!(%error, "cannot remove the keyboard hook");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT_FLAGS;

    use super::*;

    /// Returns a key event for `virtual_key` with `flags`.
    fn event(virtual_key: VIRTUAL_KEY, flags: KBDLLHOOKSTRUCT_FLAGS) -> KBDLLHOOKSTRUCT {
        KBDLLHOOKSTRUCT {
            vkCode: u32::from(virtual_key.0),
            flags,
            ..KBDLLHOOKSTRUCT::default()
        }
    }

    /// The Windows key and Alt's shortcuts are shortcuts; Tab alone is not.
    #[test]
    fn knows_the_shortcuts() {
        let none = KBDLLHOOKSTRUCT_FLAGS(0);
        assert!(is_shortcut(&event(VK_LWIN, none)));
        assert!(is_shortcut(&event(VK_TAB, LLKHF_ALTDOWN)));
        assert!(is_shortcut(&event(VK_F4, LLKHF_ALTDOWN)));
        assert!(!is_shortcut(&event(VK_TAB, none)));
        assert!(!is_shortcut(&event(VK_F4, none)));
    }

    /// A key held back while down is held back when released, even once
    /// the window is no longer in front; injected keys pass.
    #[test]
    fn holds_back_releases_of_keys_held_back() {
        let released = Rc::new(Cell::new(0));
        let mut target = Target {
            hwnd: HWND(std::ptr::null_mut()),
            active: true,
            sink: Box::new({
                let released = Rc::clone(&released);
                move |usage, pressed| {
                    assert_eq!(usage, 0xe3);
                    if !pressed {
                        released.set(released.get() + 1);
                    }
                }
            }),
            held: HashSet::from([u32::from(VK_LWIN.0)]),
        };
        assert!(!target.key(&event(VK_LWIN, LLKHF_INJECTED | LLKHF_UP)));
        assert!(target.key(&event(VK_LWIN, LLKHF_UP)));
        assert_eq!(released.get(), 1);
        assert!(!target.key(&event(VK_LWIN, LLKHF_UP)));
        assert!(target.held.is_empty());
    }
}
