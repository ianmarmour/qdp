<p align="center">
  <img src="branding/qdp-banner.svg" width="760" alt="QDP: Quick Desktop Protocol">
</p>

<p align="center">
  <a href="https://github.com/ianmarmour/qdp/actions/workflows/ci.yml"><img src="https://github.com/ianmarmour/qdp/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-025559" alt="License: Apache-2.0"></a>
  <img src="https://img.shields.io/badge/rust-1.87%2B-025559?logo=rust" alt="Rust 1.87+">
  <a href="#platforms"><img src="https://img.shields.io/badge/platforms-Linux%20%7C%20Windows%20%7C%20macOS%20%7C%20Android%20%7C%20iOS-025559" alt="Platforms: Linux, Windows, macOS, Android, iOS"></a>
</p>

**Quick Desktop Protocol: a fast, modern alternative to RDP and VNC.**

QDP streams a computer's desktop to your other devices with very low latency.
It's suitable for doing remote work and ultra low latency applications like
gaming. It leverages cutting edge codecs and techniques to provide the lowest
latency smoothest streaming possible. QDP offers native apps for Linux,
Windows, macOS, Android, iPhone and iPad.

## Highlights

- **Low Latency** - QDP uses hardware acceleration for encoding and decoding
  with the latest codecs, wherever the device supports it. It uses
  multi-threading, SIMD and advanced memory sharing techniques to keep
  everything as fast as possible, and leverages QUIC datagrams to keep network
  transmission low latency and optimized. The
  [benchmarks](docs/benchmarks.md) compare it with RDP and VNC.
- **Multi Platform** - From day one, QDP was designed with multi-platform
  usage in mind. We offer servers for Windows, Linux and macOS, and clients
  for all modern platforms, including mobile. We aim to bring every new
  feature to every platform, and test it on each.
- **Simple Setup** - QDP installs from each platform's native packages and
  apps. We never expect our end users to have to compile code from source or
  go through complex installations.
- **Enterprise Support** - We believe that enterprise customers should have a
  delightful experience too. With this in mind, we build and maintain an
  [enterprise gateway](docs/gateway.md) that gives companies a safe and
  secure remote desktop experience with QDP.

## Platforms

Install QDP as a **host** on the computer you want to reach, and as a
**client** on each device you'll connect from.

| Platform | Host | Client |
|---|---|---|
| Linux | [RPM](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.rpm), [DEB](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.deb), [TARBALL](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.tar.gz) | The host's download, and the app: [RPM](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gtk-linux-x86_64.rpm), [TARBALL](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gtk-linux-x86_64.tar.gz) |
| Windows | [MSI](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-windows-x64.msi) | The same [MSI](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-windows-x64.msi) |
| macOS (Apple silicon) | [BIN](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-macos-arm64.tar.gz) | [APP](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-macos-app.zip) |
| Android | | [APK](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-android-arm64.apk) |
| iPhone and iPad | | [IPA](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-ios-unsigned.ipa) |

Every download is on the
[latest release](https://github.com/ianmarmour/qdp/releases/latest), and our
[installation guides](docs/setup/README.md) walk through the installation
on each platform step by step.

## Documentation

- **[Initial Setup](docs/setup/README.md)**: install QDP, share a computer, and
  connect your devices to it.
- **[Troubleshooting](docs/troubleshooting.md)**: fixes for common problems,
  and how to collect logs.
- **[Gateway Setup](docs/gateway.md)**: install QDP Gateway, expose it to the
  internet, and grant access to endpoints.
- **[Specification](docs/spec/README.md)**: the Quick Desktop Protocol, and
  the reasoning behind its design.

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
| [`crates/qdp-bench`](crates/qdp-bench/README.md), [`bench`](bench/README.md) | Latency benchmarks against RDP and VNC |
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
[Contributing](CONTRIBUTING.md). To build QDP yourself, see
[Building from source](docs/building.md).

## License

QDP is licensed under the [Apache License 2.0](LICENSE).
