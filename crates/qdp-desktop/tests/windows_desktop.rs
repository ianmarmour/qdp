//! Captures the screen and performs input on a Windows desktop. The tests move
//! the real pointer and press real keys, so they are ignored by default; run
//! them with `--ignored` in a session nobody is using. Wine on a virtual X
//! display runs the input test, but has no Desktop Duplication.

#![cfg(windows)]
#![allow(unsafe_code)]

use qdp::message::KeyboardLeds;
use qdp_desktop::{DesktopDuplication, SendInputInjector};
use qdp_runtime::{FrameSource, InputAction, InputInjector, PointerButton};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_A, VK_LBUTTON, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

/// The size the injector is told the primary monitor has; the tests stay
/// inside it.
const SIZE: (u32, u32) = (640, 480);

/// The Caps Lock key's usage.
const CAPS_LOCK: u8 = 0x39;

/// Returns the pointer's position.
fn pointer() -> (i32, i32) {
    let mut position = POINT::default();
    // SAFETY: the position is written before the call returns.
    unsafe { GetCursorPos(&mut position) }.unwrap();
    (position.x, position.y)
}

/// Returns whether a virtual key is down.
fn is_down(key: u16) -> bool {
    /// The bit of `GetAsyncKeyState` reporting a key as down.
    const DOWN: u16 = 0x8000;
    // SAFETY: GetAsyncKeyState takes any virtual key code.
    (unsafe { GetAsyncKeyState(i32::from(key)) }) as u16 & DOWN != 0
}

/// Performs `action`, panicking if it fails.
fn inject(injector: &mut SendInputInjector, action: InputAction) {
    injector.inject(&action).unwrap();
}

/// Pointer motion, buttons and keys reach the desktop, and Caps Lock lights
/// its LED.
#[test]
#[ignore = "moves the real pointer and presses real keys"]
fn injects_input() {
    let mut injector = SendInputInjector::new(SIZE.0, SIZE.1);
    let (width, height) = (SIZE.0 as i32, SIZE.1 as i32);
    let (x, y) = (width / 3, height / 2);
    inject(
        &mut injector,
        InputAction::PointerMoveTo {
            x: x * 256,
            y: y * 256,
        },
    );
    // The test's monitor may be larger than `SIZE`; positions scale with it.
    let (moved_x, moved_y) = pointer();
    assert!(moved_x > 0 && moved_y > 0);
    inject(&mut injector, InputAction::PointerMoveTo { x: 0, y: 0 });
    assert_eq!(pointer(), (0, 0));
    inject(
        &mut injector,
        InputAction::MouseMoveBy {
            dx: 3 * 256,
            dy: 2 * 256,
        },
    );
    assert_eq!(pointer(), (3, 2));

    inject(&mut injector, InputAction::ButtonDown(PointerButton::Left));
    assert!(is_down(VK_LBUTTON.0));
    inject(&mut injector, InputAction::ButtonUp(PointerButton::Left));
    assert!(!is_down(VK_LBUTTON.0));
    for (usage, key) in [(0x04, VK_A), (0x52, VK_UP)] {
        inject(&mut injector, InputAction::KeyDown(usage));
        assert!(is_down(key.0), "usage {usage:#04x} pressed");
        inject(&mut injector, InputAction::KeyUp(usage));
        assert!(!is_down(key.0), "usage {usage:#04x} released");
    }

    let before = injector.keyboard_leds();
    for _ in 0..2 {
        inject(&mut injector, InputAction::KeyDown(CAPS_LOCK));
        inject(&mut injector, InputAction::KeyUp(CAPS_LOCK));
        let lit = injector.keyboard_leds();
        assert_ne!(
            lit.contains(KeyboardLeds::CAPS_LOCK),
            before.contains(KeyboardLeds::CAPS_LOCK)
        );
        assert_eq!(injector.keyboard_leds(), lit);
        inject(&mut injector, InputAction::KeyDown(CAPS_LOCK));
        inject(&mut injector, InputAction::KeyUp(CAPS_LOCK));
        assert_eq!(injector.keyboard_leds(), before);
    }
}

/// The primary monitor delivers frames of its size.
#[test]
#[ignore = "needs a desktop session on a graphics adapter"]
fn captures_the_screen() {
    let mut capture = DesktopDuplication::open().unwrap();
    let (width, height) = capture.size();
    assert!(width > 0 && height > 0);
    for _ in 0..3 {
        let frame = capture.capture().unwrap();
        assert_eq!(frame.size(), (width, height));
    }
}
