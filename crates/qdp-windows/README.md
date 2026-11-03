# qdp-windows

> **Internal crate.** It exists for QDP's own programs, has no stable API,
> and isn't published to crates.io.

The Windows app's core, which the C# app in
[`windows`](../../windows/README.md) calls through the C interface in
[`include/qdp_windows.h`](include/qdp_windows.h), exported by
`qdp_windows.dll`.

A session is [`qdp-app`](../qdp-app/README.md)'s, as in the other apps, shown
in a stream window of the core's own rather than in the app's: a top-level
window drawn with a flip-model swap chain, which the system can hand to the
display without composing it, the least delay a window can have.

Its video is decoded by D3D11 video, or by rav1d where the GPU has no AV1
decoder, and drawn by a render thread of its own: pictures go from the
session's thread straight to the render thread, never through the window's.
The window reads the keyboard, mouse, touch, pen and gamepads as the desktop
client's does, and closes when the session ends.

## Building

`windows/build.ps1` builds the core, then the app around it:
[`windows`](../../windows/README.md) says how. The C functions exist only on
Windows; `events`, the queue of what the app is told, builds everywhere, so
that the workspace's tests cover it.

## License

Apache-2.0, like the rest of [QDP](../../README.md).
