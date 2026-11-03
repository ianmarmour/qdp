<img src="branding/qdp-orbit-q.svg" width="96" height="96" alt="The QDP logo: an ivory Q on deep teal">

# QDP

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
  transmission low latency and optimized.
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

| Platform | Host | Client | Download |
|---|:---:|:---:|---|
| Linux | ✅ | ✅ | [RPM](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.rpm), [DEB](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.deb), [TARBALL](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.tar.gz) |
| Windows | ✅ | ✅ | [MSI](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-windows-x64.msi) |
| macOS (Apple silicon) | ✅ | ✅ | [APP](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-macos-app.zip) / [BIN](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-macos-arm64.tar.gz) |
| Android | | ✅ | [APK](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-android-arm64.apk) |
| iPhone and iPad | | ✅ | [IPA](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-ios-unsigned.ipa) |

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
