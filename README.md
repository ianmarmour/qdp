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
are needed to build it, and gamepads use libudev:

```bash
sudo dnf install pipewire-devel clang-devel systemd-devel
```

On Debian and Ubuntu that is
`sudo apt install libpipewire-0.3-dev libclang-dev libudev-dev`, and on Arch
`sudo pacman -S pipewire clang systemd-libs`. Audio uses PipeWire on Linux too.
To build without screen capture, audio and gamepads instead, pass
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

The host sends video at the window's size, up to its screen's. To stream less
at a large or full-screen window, `--resolution 1920x1080` asks for that size
instead, whatever the window's, and the client stretches the picture to fill
it, keeping its shape: the picture is softer, but the host encodes and the
client decodes far fewer pixels. The client then asks for an interface scale as
much smaller as the picture is stretched, rounded to 25 %, so that the host's
text shows at the size the client's own would when the host matches it.

By default the host keeps its monitor as it is and scales its frames to the
client's display. With `qdp serve --match-display on`, the host instead
changes the streamed monitor's resolution and interface scale to the client's
for each session, so that the stream needs no scaling: the monitor takes the
mode closest to the client's display, by shape and then by size, at the
nearest refresh rate, and the client's scale. When the session ends the
monitor gets its own mode and scale back; they are saved in the identity
directory while changed, so a host that stops without putting them back does
when it next starts. The host changes the settings the way the desktop's own
display tools do: on KDE Plasma through KWin's output management, and on sway,
Hyprland, river, Wayfire, niri, COSMIC and other compositors sharing the
wlroots output management protocol through that, where a headless or virtual
monitor takes the client's size exactly.

With `--headless` the client opens no window and only logs statistics, and
`--output session.y4m` also writes the decoded video to a YUV4MPEG2 file, which
ffmpeg and most players open. To build without window support, for a machine
that only serves, use `cargo build --release --no-default-features`.

To use a password instead of a token, pass `--user NAME` on both sides with the
password in the `QDP_PASSWORD` environment variable; it is checked with
SCRAM-SHA-256-PLUS, so it never crosses the network.

A client can also authenticate with a key of its own, with no secret to type:
`qdp pin --client` prints the client's key pin (creating the key in
`./qdp-client-identity`, or `--identity DIR`), the host accepts it with
`qdp serve --client-key PIN`, and `qdp connect` given neither `--token` nor
`--user` presents the key.

If the connection drops, for example when the network changes, the client
reconnects on its own without the token or password, with a single-use token
the host gave it, which stays valid for five minutes after the connection ends;
`qdp serve --resume SECONDS` changes how long, and `--resume 0` turns it off.
When a session ends other than by the client disconnecting, because the network
dropped, a newer session replaced it or the host stopped, the host locks its
desktop so that it is not left unlocked: through logind on Linux, which GNOME,
KDE and `swayidle` lock for, and with the system's own lock on macOS and
Windows. `qdp serve --lock off` leaves the desktop as it is.

On Linux the host encodes on the GPU through VA-API when the GPU can encode
AV1 (AMD RDNA 3 or newer, Intel Arc or newer), and otherwise falls back to
software encoding. It needs the GPU's VA-API driver, which desktop
distributions install with Mesa (`mesa-va-drivers`) or Intel's media driver
(`intel-media-driver`); `vainfo` lists `VAProfileAV1Profile0` with
`VAEntrypointEncSlice` when hardware encoding is available. The host logs which
encoder it uses, and `--encoder software` or `--encoder hardware` forces one.
When packets are lost, the hardware encoder recovers from the last frame the
client decoded, a frame of ordinary size, rather than with a key frame many
times larger, on Mesa 24.3 or later and Intel's driver; the host's stats count
these as `recoveries`.

With hardware encoding, the screen stays in GPU memory from the compositor to
the encoder: the desktop shares it as DMA-BUFs, and the GPU's video processor
converts and scales it for the encoder, so no pixel passes through the
processor. This is what lets the host keep up at high resolutions and frame
rates, such as 4K at 120 frames a second. It needs a desktop that shares its
screen in GPU buffers (GNOME, KDE Plasma and wlroots compositors do) and a
VA-API driver with a video processor (`VAEntrypointVideoProc` under
`VAProfileNone` in `vainfo`). The host logs "encoding frames straight from the
GPU" when it does, and copies frames through memory otherwise, or from the
moment the driver cannot read one; `qdp serve --gpu-capture off` always
copies them. `QDP_LOG=info,qdp_codec=debug` logs how long each frame takes to
reach the encoder's input, encode and read back.

Clients decode on the GPU where they can: on Macs with hardware AV1 decoding
(Apple M3 and newer) through VideoToolbox, falling back to software decoding
elsewhere or if the hardware cannot handle a stream. On a Mac, the window
draws each picture with Metal straight from the buffer VideoToolbox decoded
it into, so that no pixel is copied between decoding and the screen.
`--decoder software` or `--decoder hardware` forces one. `qdp connect --decoder-process on` decodes in
a separate process, so that a decoder bug a malicious host's stream exploits
cannot reach the session's keys, clipboard or input; a decoder process that
fails is replaced. It costs some speed, since each picture is copied between
the processes: `cargo run --release -p qdp-codec --example
decoder_process_benchmark` measures how much on a given machine.

On macOS the client draws the video with Metal: each frame's planes go to the
GPU, which converts their colours and scales them into the window, and the
window asks a ProMotion display for up to 120 Hz. Elsewhere, or if Metal
cannot start, it converts and scales frames on every processor. The client
logs each second how many frames it drew and how long drawing them took.

The client draws the host's pointer as its own, so moving it has no network
latency, and follows the host's pointer when an application moves it. Hosts
report the pointer apart from the video on Windows, on macOS, and on Linux
desktops whose ScreenCast portal offers cursor metadata, such as GNOME and KDE;
on other Linux desktops it is part of the video.

The host streams its sound, and the client plays it; `--audio off` on either
side turns it off. `qdp connect --microphone` sends the client's microphone to
the host, which on Linux presents it to its applications as a source named
"QDP microphone"; Ctrl+Alt+Shift+M turns it on or off during the session, and
the window's title says when the microphone is on. Sound is
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
with its Wayland clipboard. A host started where `WAYLAND_DISPLAY` is not set,
as over SSH, finds the session's Wayland display in `XDG_RUNTIME_DIR` itself
and logs the one it uses.

The client sends its keyboard, mouse and scrolling to the host while its
window has focus. When an application on a Windows host hides and confines the
pointer, as games do, the client captures its pointer and sends raw mouse
motion; Ctrl+Alt+Shift+Z releases it, and a click captures it again. Linux and
macOS hosts cannot tell when an application captures the pointer, so they do
the same while one keeps the pointer hidden over the captured monitor for half
a second, as games that draw their own pointer do, and switch back once it has
shown it again for half a second with no button held, so that a pointer a game
shows only while dragging does not switch modes in the middle of the drag. The client
also tells the host its keyboard layout, and Windows hosts switch to the same
one. On Linux the host performs them through uinput virtual
devices, which works under any Wayland compositor or X11, and needs write
access to `/dev/uinput`, and `/dev/uhid` for passed-through devices. While a
button is held, its pointer reports the motion of each move along with the
position, as a mouse does and in the desktop's own units at its interface
scale, which the screen cast portal tells, so that games which follow the
mouse's motion, as many do to drag their interface's elements, see the drag
as far as the pointer goes. To allow
both for members of the `input` group:

```bash
printf 'uinput\nuhid\n' | sudo tee /etc/modules-load.d/qdp.conf
sudo modprobe -a uinput uhid
printf '%s\n' 'KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"' 'KERNEL=="uhid", GROUP="input", MODE="0660", OPTIONS+="static_node=uhid"' | sudo tee /etc/udev/rules.d/60-qdp-uinput.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo usermod -aG input "$USER"
```

Log out and in again for the group change to apply. Without access the host
logs a warning and ignores input. Touches on the client's window reach Linux
and Windows hosts as a touchscreen, which macOS hosts decline. On Windows
clients a pen's pressure, tilt, rotation, eraser and barrel button reach every
host as a pen; on other clients a pen moves the pointer. With `qdp connect
--touchpad`, a Linux client in the `input` group sends its touchpad's raw
fingers instead, so that a Linux host's own gestures apply; the touchpad then
works only on the host while the window has focus. The client also
sends the gamepads connected to it while its window has focus, and plays the
rumble the host's games set on them. A Linux host presents each as a virtual Xbox 360
controller; hosts on other systems decline them. A Linux client can also pass
a HID device through as it is, with `qdp connect --passthrough
VENDOR:PRODUCT` (the IDs `lsusb` shows, such as `054c:09cc`), which needs
read and write access to its `/dev/hidraw*` node, as the udev rules for game
controllers grant. Devices in a firmware update mode, such as bootloaders, are
never passed through, so that the host's software cannot reflash them. A Linux host recreates the device through uhid for its
games, rumble and lights included; by default the host accepts only game
controllers, and `qdp serve --passthrough all` or `--passthrough off`
accepts every device or none. On
Windows the host sends input with
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
while the sign-in screen or an elevation prompt is shown. On Linux and macOS
the host encodes each frame as the compositor delivers it, rather than at fixed
frame times, so that frames do not wait for the next one; it still sends no
more than `--fps` a second, and repeats the last frame when nothing changes.
Its statistics give `new_fps`, how many of the frames sent showed a new
screen. If the screen cannot be captured the host
streams a test pattern instead, and `--source screen` or `--source
test-pattern` forces one. On Linux the host finds where the streamed monitor
sits among the desktop's monitors through xdg-output, and places the client's
pointer on it, since the virtual pointer covers them all. Run `qdp help` for
every option, and set `QDP_LOG=debug`
for more detail. `QDP_LOG` also takes levels for single parts after the
default one: `QDP_LOG=info,qdp::window=debug` on the client and
`QDP_LOG=info,qdp_runtime::host=debug,qdp_runtime::host_cursor=debug` on the
host trace each pointer button, every move while a button is held, positions
the host ignores and the host's warps of the client's pointer.

## Crates

| Crate | Contents |
|---|---|
| `qdp` | The protocol: messages, session state machines, video and input logic, with no I/O |
| `qdp-quic` | QUIC transport with quinn and rustls |
| `qdp-codec` | Software AV1 encoding with rav1e and decoding with rav1d |
| `qdp-runtime` | Host and client media pipelines |
| `qdp-desktop` | Screen capture and input injection for Linux, macOS and Windows hosts |
| `qdp-cli` | The `qdp` command |
