<img src="branding/qdp-orbit-q.svg" width="96" height="96" alt="The QDP logo: an ivory Q on deep teal">

# QDP

**Quick Desktop Protocol: a fast, modern alternative to RDP and VNC.**

QDP streams a computer's desktop to your other devices with very low latency.
It's suitable for doing remote work and ultra low latency applications like
gaming. QDP supports native apps for Linux, Windows, macOS, Android, iPhone and
iPad. It leverages cutting edge codecs and techniques to provide the lowest
latency smoothest streaming possible.

QDP is in active development. Its protocol is draft 00 of an
[open specification](docs/spec/README.md), and may change before version 1.

## Highlights

- **Low latency.** Frames are encoded as the desktop draws them and sent as
  QUIC datagrams. On Linux they stay in GPU memory from capture to encoder,
  keeping up with 4K at 120 frames a second. Each device draws the pointer
  itself, so it never lags.
- **Simple pairing.** Type the four-digit code the computer shows, once. No
  accounts, no passwords.
- **Made for games too.** Controllers with rumble, raw mouse input, an
  on-screen controller on phones and tablets, and, between Linux computers,
  controllers passed through as they are.
- **Everything you expect.** Sound both ways, a shared clipboard, touch, pens,
  and the login screen on Linux.
- **Secure by default.** TLS 1.3 with pinned keys, a notice on the computer
  while it's shared, and a locked desktop if a session drops.
- **Ready for organizations.** A [gateway](docs/gateway.md) signs people in
  with the organization's directory and an authenticator app, and reaches the
  computers inside its network.

## Platforms

| | Share this computer | Connect to a computer |
|---|---|---|
| **Linux** | `qdp serve`, the login screen included | The QDP app, `qdp connect` |
| **Windows** | `qdp serve` | The QDP app, `qdp connect` |
| **macOS** | `qdp serve` | The QDP app, `qdp connect` |
| **Android** | | The QDP app |
| **iPhone and iPad** | | The Quick Desktop app |

## Installing

Every download is on the
[latest release](https://github.com/ianmarmour/qdp/releases/latest).

| Platform | Download | Guide |
|---|---|---|
| Linux | `qdp-linux-x86_64.rpm` or `.deb`, and the app's `qdp-gtk-linux-x86_64.rpm` or `.tar.gz` | [Installing on Linux](docs/install/linux.md) |
| Windows | `qdp-windows-x64.msi` | [Installing on Windows](docs/install/windows.md) |
| Mac (Apple silicon) | `qdp-macos-app.zip` and `qdp-macos-arm64.tar.gz` | [Installing on a Mac](docs/install/macos.md) |
| Android | `qdp-android-arm64.apk` | [Installing on Android](docs/install/android.md) |
| iPhone and iPad | Build with Xcode, or sideload `qdp-ios-unsigned.ipa` | [Installing on iPhone and iPad](docs/install/iphone-ipad.md) |

## Getting started

1. On the computer you want to reach, start the host:

   ```bash
   qdp serve
   ```

2. On your device, open the QDP app and choose the computer. Linux computers
   show up on their own; add Windows and Mac computers by their address. From
   a terminal, `qdp connect ADDRESS --pair` does the same.
3. Type the four-digit code the computer shows.

From then on, the two remember each other, and connecting takes one tap.

## Documentation

- [Sharing a computer](docs/hosting.md): pairing, platform setup, video,
  sound, input, security, and running as a service.
- [Connecting](docs/connecting.md): each app, and the `qdp connect` command.
- [The gateway](docs/gateway.md): reaching computers inside an organization.
- [Troubleshooting](docs/troubleshooting.md): logs, and what to check when
  something doesn't work.
- [The specification](docs/spec/README.md) and its
  [design rationale](docs/design-rationale.md).

[The documentation's index](docs/README.md) lists everything.

## Building from source

You need Rust 1.87 or later, a C compiler, and your system's development
files, which [Building from source](docs/building.md) lists for each system.

```bash
git clone https://github.com/ianmarmour/qdp.git && cd qdp
cargo build --release
cargo test --workspace
```

This builds the `qdp` command and the gateway into `target/release`. Each app
builds with its platform's tools: see [`android`](android/README.md),
[`apple`](apple/README.md), [`windows`](windows/README.md) and
[`crates/qdp-gtk`](crates/qdp-gtk/README.md).

## Repository layout

| Path | What it is |
|---|---|
| [`crates/qdp`](crates/qdp/README.md) | The protocol, with no I/O |
| [`crates/qdp-quic`](crates/qdp-quic/README.md) | QUIC transport |
| [`crates/qdp-codec`](crates/qdp-codec/README.md) | AV1 and Opus, in software and hardware |
| [`crates/qdp-runtime`](crates/qdp-runtime/README.md) | Host and client media pipelines |
| [`crates/qdp-desktop`](crates/qdp-desktop/README.md) | Screen capture, input, audio and more for each operating system |
| [`crates/qdp-app`](crates/qdp-app/README.md) | What the client apps share |
| [`crates/qdp-cli`](crates/qdp-cli/README.md) | The `qdp` command |
| [`crates/qdp-gateway`](crates/qdp-gateway/README.md) | The gateway |
| [`crates/qdp-gtk`](crates/qdp-gtk/README.md) | The Linux app |
| [`crates/qdp-android`](crates/qdp-android/README.md), [`android`](android/README.md) | The Android app and its core |
| [`crates/qdp-apple`](crates/qdp-apple/README.md), [`apple`](apple/README.md) | The Mac, iPhone and iPad apps and their core |
| [`crates/qdp-windows`](crates/qdp-windows/README.md), [`windows`](windows/README.md) | The Windows app and its core |
| [`packaging`](packaging/README.md) | RPMs, Debian packages and Windows installers |
| [`docs`](docs/README.md) | User guides, developer guides and the specification |
| [`branding`](branding/README.md) | The logo and app icons |

The [architecture overview](docs/architecture.md) explains how they fit
together.

## Contributing

Bug reports and pull requests are welcome: see
[Contributing](CONTRIBUTING.md).

## License

QDP is licensed under the [Apache License 2.0](LICENSE).
