# qdp-app

> **Internal crate.** It exists for QDP's own programs, has no stable API,
> and isn't published to crates.io.

The core QDP's client apps share: the Android, Apple and Windows apps, each
through its own core, and the `qdp connect` window.

## What's inside

| Module | What it does |
|---|---|
| `session` | Runs a session with a host on a thread of its own, reconnecting and pairing with the code the host shows, and passes the user's input to it |
| `events` | What the app is told, as lines of text |
| `sink` | Passes the session's video to the platform's `Presenter`, and its cursor, pointer mode and statistics to the app |
| `settings` | What the app connects with, and whether a host answers at its address |
| `gamepad` | Converts a gamepad's state to QDP's report |
| `virtual_gamepad` | Lays out the on-screen controller of the phone and tablet apps, and works out its state from the touches on it |
| `cursor` | Scales the host's cursor images to the picture |
| `placement` | Fits the picture in the app's view, keeping its shape |
| `qr` | Encodes the QR code that adds a gateway to an authenticator app |
| `keymap`, `window_input` | With the `winit` feature: winit windows' keys as HID usages and their other input as QDP's, for the desktop clients |

## Where it fits

[`qdp-android`](../qdp-android/README.md), [`qdp-apple`](../qdp-apple/README.md)
and [`qdp-windows`](../qdp-windows/README.md) wrap it for their apps, and
[`qdp-cli`](../qdp-cli/README.md) uses it for its window. Everything here
builds and is tested on every platform with the workspace.

## License

Apache-2.0, like the rest of [QDP](../../README.md).
