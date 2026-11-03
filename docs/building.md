# Building QDP from source

Most people don't need to build QDP: [Initial Setup](setup/README.md) covers
the prebuilt releases. To build it yourself, you need Rust 1.87 or later from
[rustup](https://rustup.rs), a C compiler, and a clone of the repository:

```bash
git clone https://github.com/ianmarmour/qdp.git && cd qdp
```

Then set up your system as below, and build the `qdp` command and the gateway
into `target/release`:

```bash
cargo build --release
```

## Linux

Install the compiler, NASM and the development files QDP uses. On Fedora:

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

A host you built needs access to the input devices it creates: see
[Installing on Linux](setup/linux.md#input-without-the-packages).

## macOS

Install Xcode 26 from the App Store and open it once to accept its license.
Intel Macs also need NASM (`brew install nasm`).

## Windows

Install the Visual Studio C++ build tools, then Rust, NASM, Git and the
.NET 8 SDK:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
foreach ($id in 'Rustlang.Rustup', 'NASM.NASM', 'Git.Git', 'Microsoft.DotNet.SDK.8') { winget install --exact --id $id }
```

NASM's installer doesn't add itself to the `PATH`, so add it, then open a new
terminal:

```powershell
[Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';C:\Program Files\NASM', 'User')
```

## The apps

Each app builds with its platform's tools:

| App | Builds on | Guide |
|---|---|---|
| Android | Linux, macOS | [`android`](../android/README.md) |
| Mac, iPhone and iPad | macOS | [`apple`](../apple/README.md) |
| Windows | Windows | [`windows`](../windows/README.md) |
| Linux | Linux | [`crates/qdp-gtk`](../crates/qdp-gtk/README.md) |

The packages and installers build from these programs: see
[Packaging](../packaging/README.md).

## Checking your changes

CI runs these checks, which you can run before opening a pull request:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

If tests fail for timing on a machine with few processors, run them one at a
time with `-- --test-threads=1`.
