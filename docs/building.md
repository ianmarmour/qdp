# Building QDP from source

This page sets up a machine to build and test QDP: the `qdp` command, the
gateway and the apps. Most people don't need to build anything: the
[installation guides](setup/README.md) cover the prebuilt releases.

- [Requirements](#requirements)
- [Linux](#linux)
- [macOS](#macos)
- [Windows](#windows)
- [The apps](#the-apps)
- [Checking your changes](#checking-your-changes)

## Requirements

Every build needs:

- **Rust 1.87 or later.** [rustup](https://rustup.rs) installs it. CI builds
  with Rust 1.97.
- **A C compiler.** The codecs use it for their hand-written assembly.
- **A clone of the repository:**

  ```bash
  git clone https://github.com/ianmarmour/qdp.git && cd qdp
  ```

`cargo build --release` builds the `qdp` command (both the host and the
client) and the gateway, `qdp-gateway`, into `target/release`. The apps build
with their platforms' own tools, as their READMEs describe.

## Linux

Install the compiler, NASM (on x86-64), and the development files QDP uses:

- PipeWire and libclang, for screen capture and audio.
- libudev, for gamepads.
- GTK 4 and libadwaita, for the Linux app.

On Fedora:

```bash
sudo dnf install gcc nasm pkgconf-pkg-config clang-devel pipewire-devel systemd-devel gtk4-devel libadwaita-devel
```

On Debian and Ubuntu:

```bash
sudo apt install build-essential nasm pkg-config clang libclang-dev libpipewire-0.3-dev libudev-dev libgtk-4-dev libadwaita-1-dev
```

On Arch:

```bash
sudo pacman -S base-devel nasm clang pipewire systemd-libs gtk4 libadwaita
```

Build `qdp` and `qdp-gateway`:

```bash
cargo build --release
```

Build the Linux app, which runs the `qdp` built beside it:

```bash
cargo build --release -p qdp-gtk
```

To build `qdp` without screen capture, audio and gamepads, and so without
PipeWire and libudev, pass `--no-default-features --features window`. For a
machine that only serves, `--no-default-features` also leaves out the window,
so `qdp connect` runs only with `--headless`. The
[`qdp-cli` README](../crates/qdp-cli/README.md#features) lists every feature.

The RPM and Debian packages build from these programs:
[`packaging`](../packaging/README.md) says how.

To run the host you built, see [Sharing a computer](setup/hosting.md). On Linux it needs
access to `/dev/uinput` for input, which the packages set up; a build run from
the repository needs the [manual setup](setup/linux.md#input-without-the-packages).

## macOS

Install Xcode 26 from the App Store, which brings the compiler, and open it
once to accept its license. Then install XcodeGen, which writes the apps'
Xcode project, with [Homebrew](https://brew.sh):

```bash
brew install xcodegen
```

Macs with Apple silicon need nothing else. Intel Macs also need NASM
(`brew install nasm`) to build `qdp`, and the apps run only on Apple silicon.

Build `qdp` and `qdp-gateway`:

```bash
cargo build --release
```

The Mac, iPhone and iPad apps build with Xcode:
[`apple`](../apple/README.md) says how. The Android app builds on a Mac as
it does on Linux: see [`android`](../android/README.md).

## Windows

Install, from a terminal, the Visual Studio C++ build tools, which Rust uses
to compile and link:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then Rust, NASM, Git and, for the Windows app, the .NET 8 SDK:

```powershell
foreach ($id in 'Rustlang.Rustup', 'NASM.NASM', 'Git.Git', 'Microsoft.DotNet.SDK.8') { winget install --exact --id $id }
```

NASM's installer doesn't add itself to the `PATH`. Add
`C:\Program Files\NASM` to your user's `PATH`, then open a new terminal so
that it has this and the other installers' changes:

```powershell
[Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';C:\Program Files\NASM', 'User')
```

Build `qdp.exe` and `qdp-gateway.exe`:

```powershell
cargo build --release
```

The Windows app builds with the .NET SDK: [`windows`](../windows/README.md)
says how. Its installers, and the gateway's, build with the WiX Toolset:
[`packaging`](../packaging/README.md#windows-installers) says how.

## The apps

| App | Where | Builds on |
|---|---|---|
| Android | [`android`](../android/README.md) | Linux, macOS |
| Mac, iPhone and iPad | [`apple`](../apple/README.md) | macOS |
| Windows | [`windows`](../windows/README.md) | Windows |
| Linux | [`crates/qdp-gtk`](../crates/qdp-gtk/README.md) | Linux |

Each app is a native app around a Rust core in `crates/`. The
[architecture overview](architecture.md) explains how they fit together.

## Checking your changes

CI checks every change with these commands, which you can run before opening
a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The streaming tests encode, send and decode video and audio in real time, a
host and a client each. On a machine with few processors they can fall behind
when they run together, so CI runs them one at a time on macOS and Windows:
add `-- --test-threads=1` if they fail there for timing.

The apps have checks of their own:

- **Android:** `./gradlew assembleRelease lintRelease` in `android`, and
  `cargo clippy --target aarch64-linux-android -p qdp-android --all-targets`.
- **Windows:** `dotnet test windows/QDP.Tests/QDP.Tests.csproj -c Release`.

[Releasing](releasing.md) describes how CI runs these checks and publishes
releases.
