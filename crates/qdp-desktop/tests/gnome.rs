//! Shares a GNOME desktop through Mutter's remote desktop interface: its
//! frames, and the pointer the injector moves. It needs a GNOME session on
//! the D-Bus session bus, such as `mutter --headless`, so it is ignored by
//! default; run it with `--ignored` inside one.

#![cfg(all(target_os = "linux", feature = "screen-capture"))]

use std::time::{Duration, Instant};

use qdp_desktop::{ScreenOptions, open_gnome};
use qdp_runtime::{InputAction, PointerPosition};

/// How long the pointer may take to arrive where it was moved.
const MOVE_TIMEOUT: Duration = Duration::from_secs(5);

/// QDP's units in a pixel.
const UNITS_PER_PIXEL: i32 = 256;

/// Frames arrive, and the pointer goes where the injector moves it.
#[test]
#[ignore = "needs a GNOME session"]
fn shares_a_gnome_desktop() {
    let (mut source, mut injector) = open_gnome(ScreenOptions::default()).unwrap();
    let (width, height) = source.size();
    assert!(width > 0 && height > 0);
    assert_eq!(source.capture().unwrap().size(), (width, height));

    let target = (width as i32 / 3, height as i32 / 4);
    injector
        .inject(&InputAction::PointerMoveTo {
            x: target.0 * UNITS_PER_PIXEL,
            y: target.1 * UNITS_PER_PIXEL,
        })
        .unwrap();
    let started = Instant::now();
    loop {
        let _ = source.capture();
        let position = source.cursor().map(|cursor| cursor.position);
        if position == Some(PointerPosition::At(target.0, target.1)) {
            break;
        }
        assert!(
            started.elapsed() < MOVE_TIMEOUT,
            "the pointer is at {position:?}, not {target:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
