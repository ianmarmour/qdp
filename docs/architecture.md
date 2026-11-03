# Architecture

QDP is one Cargo workspace of Rust crates, plus a native app for each client
platform built around a Rust core. This page explains how the pieces fit, for
anyone changing the code. The [specification](spec/README.md) defines the
protocol itself.

## A session, end to end

A **host** is a computer that shares its desktop. A **client** is a device
that shows it. A session between them runs over one QUIC connection:

1. **Connecting.** The client connects over QUIC with TLS 1.3 and checks the
   host's key against the pin it remembered when pairing. The two then
   authenticate on the control stream: with the client's key, a token, a
   password, a four-digit pairing code the first time, or an assertion from
   a [gateway](gateway.md).
2. **Video.** The host captures its screen, encodes each frame as AV1 on the
   GPU where it can, and sends it in fragments as QUIC datagrams. The client
   reassembles each frame, asks for what was lost, decodes it on the GPU
   where it can, and draws it.
3. **Timing.** Both sides keep a shared session clock. The client reports
   when each frame reached the screen, so that the host can pace its encoder
   and send rate to the client and the network.
4. **Everything else.** Audio flows both ways as Opus or PCM. The client draws
   the host's cursor itself, so that moving it has no network delay. Input
   goes to the host as USB HID reports: keyboard, mouse, touch, pen, gamepads
   and passed-through devices. Clipboard text is shared on request.

The protocol logic is sans-I/O: the `qdp` crate holds state machines that
take and return messages, and the other crates connect them to the network,
the codecs and the operating system.

## Crates

The crates form layers, each building on the ones above it:

```text
protocol            qdp
transport, codecs   qdp-quic, qdp-codec
pipelines           qdp-runtime
integration         qdp-desktop (the operating systems), qdp-app (the apps' shared core)
programs            qdp-cli (the qdp command), qdp-android, qdp-apple, qdp-windows (the apps' cores)

standalone          qdp-gateway (needs only qdp and qdp-quic)
                    qdp-gtk (the Linux app, which runs the qdp command)
```

| Crate | What it does | Used by |
|---|---|---|
| [`qdp`](../crates/qdp/README.md) | The protocol: wire format, messages, session state machines, video, input and clock logic, with no I/O | every crate |
| [`qdp-quic`](../crates/qdp-quic/README.md) | QUIC transport with quinn and rustls: endpoints, key pinning, the session driver | runtime, gateway |
| [`qdp-codec`](../crates/qdp-codec/README.md) | AV1 encoding and decoding, in software and in each platform's hardware; Opus audio | runtime, desktop, apps |
| [`qdp-runtime`](../crates/qdp-runtime/README.md) | The host and client media pipelines that tie the protocol, transport and codecs together | desktop, CLI, apps' cores |
| [`qdp-desktop`](../crates/qdp-desktop/README.md) | Each operating system's screen capture, input, audio, clipboard, discovery and notifications | CLI, apps' cores |
| [`qdp-app`](../crates/qdp-app/README.md) | What the client apps share: sessions with reconnection and pairing, events, settings, the on-screen gamepad | CLI, apps' cores |
| [`qdp-cli`](../crates/qdp-cli/README.md) | The `qdp` command: `qdp serve` and `qdp connect` | people, the Linux app |
| [`qdp-gateway`](../crates/qdp-gateway/README.md) | The gateway server for organizations | administrators |
| [`qdp-android`](../crates/qdp-android/README.md) | The Android app's core, called from Kotlin through JNI | the Android app |
| [`qdp-apple`](../crates/qdp-apple/README.md) | The Mac, iPhone and iPad apps' core, called from Swift through C | the Apple apps |
| [`qdp-windows`](../crates/qdp-windows/README.md) | The Windows app's core, called from C# through C | the Windows app |
| [`qdp-gtk`](../crates/qdp-gtk/README.md) | The Linux app, in GTK 4 and libadwaita | people |

## Apps

| App | Code | Core | Video |
|---|---|---|---|
| Android | Kotlin and Jetpack Compose, in [`android`](../android/README.md) | `qdp-android`, through JNI | MediaCodec, straight into a `SurfaceView` |
| Mac, iPhone and iPad | SwiftUI, in [`apple`](../apple/README.md) | `qdp-apple`, through a C interface | VideoToolbox, drawn with Metal |
| Windows | WinUI 3 and C#, in [`windows`](../windows/README.md) | `qdp-windows`, a DLL with a C interface | D3D11 video, in a flip-model swap chain |
| Linux | GTK 4 and libadwaita, in [`crates/qdp-gtk`](../crates/qdp-gtk/README.md) | runs `qdp connect --events` | the `qdp` command's window |
| Command line | [`crates/qdp-cli`](../crates/qdp-cli/README.md) | itself | Metal on macOS, the processor elsewhere |

Each app decodes in software with rav1d where the device has no AV1 decoder.
The cores draw video on a thread of their own, never the app's main thread,
and hand pictures from the decoder to the screen without copying them where
the platform allows.

## Platform code

| | Linux | Windows | macOS |
|---|---|---|---|
| Screen capture | ScreenCast portal and PipeWire; DRM/KMS for the console | DXGI Desktop Duplication | ScreenCaptureKit |
| Hardware encoding | VA-API | NVENC, then Media Foundation (AMD, Intel) | none: software |
| Hardware decoding | none: software | D3D11 video | VideoToolbox |
| Input | uinput and uhid | `SendInput` | Quartz events |
| Audio | PipeWire | WASAPI, through cpal | ScreenCaptureKit and cpal |
| Advertising the host | Avahi | none: clients add the host by address | none: clients add the host by address |

The host-side code lives in `qdp-desktop`, and the codecs in `qdp-codec`,
each behind `#[cfg(target_os = ...)]`. Code that only builds on one platform
keeps its platform-independent logic in modules that build everywhere, so
that the workspace's tests on Linux still cover it.

## Where to look

| To change | Start in |
|---|---|
| A message or its wire format | `crates/qdp/src/message` and the spec chapter it belongs to |
| Session setup, authentication, pairing | `crates/qdp/src/session`, `crates/qdp-runtime/src/auth.rs`, `pairing.rs` |
| Fragmentation, loss recovery | `crates/qdp/src/video` |
| Encoding or decoding | `crates/qdp-codec` |
| How the host streams | `crates/qdp-runtime/src/host.rs` |
| How the client receives | `crates/qdp-runtime/src/client.rs` |
| Screen capture or input on one OS | `crates/qdp-desktop/src/{linux,macos,win32}` |
| The `qdp` command's options | `crates/qdp-cli/src/args.rs` |
| What the apps share | `crates/qdp-app` |
