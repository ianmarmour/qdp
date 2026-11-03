# qdp-desktop

Desktop integration for the Quick Desktop Protocol (QDP): each operating
system's screen capture, input, audio, clipboard, discovery and
notifications, for hosts and for desktop clients.

## What's inside

| | Linux | macOS | Windows |
|---|---|---|---|
| Screen capture | `ScreenCapture`, through the ScreenCast portal and PipeWire, which can also change the monitor's mode and scale; `ConsoleCapture`, through DRM/KMS, for a host running as root | `MacScreenCapture`, with ScreenCaptureKit | `DesktopDuplication`, with DXGI Desktop Duplication |
| Input | `UinputInjector`, virtual devices through uinput | `QuartzInjector`, Quartz events | `SendInputInjector`, `SendInput` |
| Audio | PipeWire | ScreenCaptureKit and cpal | cpal, with WASAPI loopback |
| Advertising | Avahi | | |
| Notifications | the freedesktop notification service | Notification Center | an on-screen banner |

It also holds:

- `SystemClipboard`, which shares the system clipboard's text on each system;
- `GamepadCapture`, which watches a client's gamepads;
- `PassthroughCapture`, which passes the HID devices the user chose through;
- `TouchpadCapture`, which sends a Linux touchpad's raw contacts;
- `lock_desktop`, which locks the desktop when a session ends unexpectedly;
- `host_privacy`, which covers a Windows host's displays and holds back its
  own input during a session;
- `MetalVideo`, which draws decoded video with Metal on macOS and iOS.

## Features

| Feature | What it adds | Builds with (Linux) |
|---|---|---|
| `audio` | Playing and capturing sound | PipeWire's development files, libclang |
| `discovery` | Advertising the host on the local network | |
| `gamepad` | Capturing the client's gamepads | libudev's development files |
| `notifications` | Desktop notifications, such as pairing codes | |
| `screen-capture` | Capturing the screen | PipeWire's development files, libclang |

All are on by default.

## Tests

The tests in `tests/` capture the screen and use the clipboard and desktop
of the machine they run on, where it has them.

## Documentation

```bash
cargo doc -p qdp-desktop --open
```

## License

Apache-2.0, like the rest of [QDP](../../README.md).
