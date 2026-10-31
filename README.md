# qdp
Quick Desktop Protocol (QDP): a fast and modern alternative to RDP and VNC

The protocol is specified in [`doc/spec`](doc/spec), with the reasoning behind
its design in [`doc/design-rationale.md`](doc/design-rationale.md).

## Trying it

Building needs a Rust toolchain and a C compiler, which the codecs use for
their hand-written assembly. On x86-64 it also needs NASM:

```bash
sudo dnf install nasm
```

On Debian and Ubuntu that is `sudo apt install nasm`, on Arch
`sudo pacman -S nasm`, and on macOS with Homebrew `brew install nasm` (only
needed on Intel Macs). Apple Silicon Macs need only the Xcode command line
tools. On Windows, install NASM, for example with `winget install NASM.NASM`,
alongside the Visual Studio C++ build tools that Rust uses there.

On Linux, screen capture uses PipeWire, whose development files and libclang
are needed to build it:

```bash
sudo dnf install pipewire-devel clang-devel
```

On Debian and Ubuntu that is `sudo apt install libpipewire-0.3-dev libclang-dev`,
and on Arch `sudo pacman -S pipewire clang`. Audio uses PipeWire on Linux too.
To build without screen capture and audio instead, pass
`--no-default-features --features window`.

Build the `qdp` command, which is both the host and the client:

```bash
cargo build --release
```

On the host, print the key pin that clients use to recognize it, then serve.
The host creates its key in `./qdp-identity` the first time:

```bash
./target/release/qdp pin
```

```bash
./target/release/qdp serve --token my-secret
```

On the client, connect with the host's address or name and its pin. The host
listens on UDP port 4433 unless given `--listen`; add `:PORT` to the address
for another port. The stream opens in a window, and closing the window
disconnects:

```bash
./target/release/qdp connect 192.0.2.7 --pin PIN --token my-secret
```

With `--headless` the client opens no window and only logs statistics, and
`--output session.y4m` also writes the decoded video to a YUV4MPEG2 file, which
ffmpeg and most players open. To build without window support, for a machine
that only serves, use `cargo build --release --no-default-features`.

To use a password instead of a token, pass `--user NAME` on both sides with the
password in the `QDP_PASSWORD` environment variable; it is checked with
SCRAM-SHA-256-PLUS, so it never crosses the network.

On Linux the host encodes on the GPU through VA-API when the GPU can encode
AV1 (AMD RDNA 3 or newer, Intel Arc or newer), and otherwise falls back to
software encoding. It needs the GPU's VA-API driver, which desktop
distributions install with Mesa (`mesa-va-drivers`) or Intel's media driver
(`intel-media-driver`); `vainfo` lists `VAProfileAV1Profile0` with
`VAEntrypointEncSlice` when hardware encoding is available. The host logs which
encoder it uses, and `--encoder software` or `--encoder hardware` forces one.

Clients decode on the GPU where they can: on Macs with hardware AV1 decoding
(Apple M3 and newer) through VideoToolbox, falling back to software decoding
elsewhere or if the hardware cannot handle a stream. `--decoder software` or
`--decoder hardware` forces one.

The client draws the host's pointer as its own, so moving it has no network
latency, and follows the host's pointer when an application moves it. Hosts
report the pointer apart from the video on Windows, on macOS, and on Linux
desktops whose ScreenCast portal offers cursor metadata, such as GNOME and KDE;
on other Linux desktops it is part of the video.

The host streams its sound, and the client plays it; `--audio off` on either
side turns it off. `qdp connect --microphone` sends the client's microphone to
the host, which on Linux presents it to its applications as a source named
"QDP microphone"; the window's title says when the microphone is on. Sound is
coded with Opus, in pure Rust, or sent as PCM. On Linux the host captures what
plays on its default output through PipeWire; on Windows it captures the
default output by loopback; on macOS 13 and later it captures the desktop's
sound with ScreenCaptureKit, which needs Screen Recording to be allowed. Only
Linux hosts can present a client's microphone, since macOS and Windows would
need a virtual audio driver.

Text copied on either side can be pasted on the other: the host and the client
share their clipboards' text. Text a password manager marks as secret is never
shared. `--clipboard send` or `--clipboard receive` limits sharing to one
direction, and `--clipboard off` turns it off, on either side. On Linux this
uses the Wayland compositor's data control protocol where it has one (KDE and
wlroots compositors), and the X11 clipboard otherwise, which GNOME keeps in step
with its Wayland clipboard.

The client sends its keyboard, mouse and scrolling to the host while its
window has focus. When an application on a Windows host hides and confines the
pointer, as games do, the client captures its pointer and sends raw mouse
motion; Ctrl+Alt+Shift+Z releases it, and a click captures it again. The client
also tells the host its keyboard layout, and Windows hosts switch to the same
one. On Linux the host performs them through uinput virtual
devices, which works under any Wayland compositor or X11, and needs write
access to `/dev/uinput`. To allow it for members of the `input` group:

```bash
echo uinput | sudo tee /etc/modules-load.d/uinput.conf
sudo modprobe uinput
echo 'KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"' | sudo tee /etc/udev/rules.d/60-qdp-uinput.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo usermod -aG input "$USER"
```

Log out and in again for the group change to apply. Without access the host
logs a warning and ignores input. A Linux host also accepts the client's
gamepads, each as a virtual Xbox 360 controller that games rumble; hosts on
other systems decline them. On Windows the host sends input with
`SendInput`; Windows keeps it from reaching programs run as administrator unless
the host runs as administrator too, and from the sign-in screen and elevation
prompts entirely.

The host streams its screen. On Linux it asks the desktop, through the
ScreenCast portal that GNOME, KDE and wlroots compositors provide, which
monitor to share; the first time, the desktop shows a dialog, and the choice is
remembered in the identity directory for later sessions. On macOS the host
captures the main display with ScreenCaptureKit; the first time, macOS asks to
allow Screen Recording for the terminal or app running `qdp`, and performing
the client's input needs Accessibility to be allowed for it as well, both under
System Settings, Privacy & Security. On Windows 10 and later the host captures
the primary monitor with DXGI Desktop Duplication, and holds the last image
while the sign-in screen or an elevation prompt is shown. If the screen cannot
be captured the host
streams a test pattern instead, and `--source screen` or `--source
test-pattern` forces one. Run `qdp help` for every option, and set `QDP_LOG=debug`
for more detail.

## Crates

| Crate | Contents |
|---|---|
| `qdp` | The protocol: messages, session state machines, video and input logic, with no I/O |
| `qdp-quic` | QUIC transport with quinn and rustls |
| `qdp-codec` | Software AV1 encoding with rav1e and decoding with rav1d |
| `qdp-runtime` | Host and client media pipelines |
| `qdp-desktop` | Screen capture and input injection for Linux, macOS and Windows hosts |
| `qdp-cli` | The `qdp` command |
