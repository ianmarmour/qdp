<img src="branding/qdp-orbit-q.svg" width="96" height="96" alt="The QDP logo: an ivory Q on deep teal">

# qdp
Quick Desktop Protocol (QDP): a fast and modern alternative to RDP and VNC

The protocol is specified in [`docs/spec`](docs/spec), with the reasoning behind
its design in [`docs/design-rationale.md`](docs/design-rationale.md).

## Building

Every build needs Rust 1.87 or later, which [rustup](https://rustup.rs)
installs (CI builds with 1.97), and a C compiler, which the codecs use for
their hand-written assembly. Start from a clone of the repository:

```bash
git clone https://github.com/ianmarmour/qdp.git && cd qdp
```

`cargo build --release` builds the `qdp` command, which is both the host and
the client, and the gateway, `qdp-gateway`, into `target/release`. The apps
build with their platforms' tools, as below.

### On Linux

Install the compiler, NASM (for x86-64), PipeWire's and libudev's development
files and libclang for screen capture, audio and gamepads, and GTK 4's and
libadwaita's for the Linux app. On Fedora:

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
PipeWire and libudev, pass `--no-default-features --features window`.

The packages build from those programs: the RPMs with `rpmbuild` (`rpm-build`
on Fedora, `rpm` on Debian and Ubuntu), and the Debian packages with
`dpkg-deb`, which Debian and Ubuntu have:

```bash
packaging/linux/build-rpm.sh target/release/qdp 1 qdp.rpm
```

```bash
packaging/linux/build-deb.sh qdp target/release/qdp 1 qdp.deb
```

```bash
packaging/linux/build-deb.sh qdp-gateway target/release/qdp-gateway 1 qdp-gateway.deb
```

The Android app builds on Linux or a Mac. It needs JDK 17 or later, the
Android SDK with its NDK (Android Studio's SDK Manager installs both, the NDK
under SDK Tools as NDK (Side by side)), and Rust's Android target:

```bash
rustup target add aarch64-linux-android
```

Build it, which builds its Rust core first, into
`android/app/build/outputs/apk/release/app-release.apk`, or replace
`assembleRelease` with `installRelease` to install it on a device connected
with USB debugging. On a Mac the SDK is in `~/Library/Android/sdk`:

```bash
cd android && ANDROID_HOME=~/Android/Sdk ./gradlew assembleRelease
```

### On macOS

Install Xcode 26 from the App Store, which brings the compiler, open it once
to accept its license, and install XcodeGen, which writes the apps' Xcode
project, with [Homebrew](https://brew.sh). Macs with Apple silicon need
nothing else; Intel Macs also need NASM (`brew install nasm`) for `qdp`, and
the apps run only on Apple silicon.

```bash
brew install xcodegen
```

Build `qdp` and `qdp-gateway`:

```bash
cargo build --release
```

For the Mac, iPhone and iPad apps, add Rust's Apple targets:

```bash
rustup target add aarch64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim
```

Then build their Rust core and generate the Xcode project, from the `apple`
folder:

```bash
cd apple && ./build-core.sh && xcodegen generate
```

Build the Mac app, signed ad hoc, into `apple/build/Build/Products/Release/QDP.app`:

```bash
xcodebuild -project QDP.xcodeproj -scheme QDP-macOS -configuration Release -destination 'generic/platform=macOS' -derivedDataPath build CODE_SIGN_IDENTITY=- CODE_SIGN_STYLE=Manual DEVELOPMENT_TEAM= build
```

```bash
open build/Build/Products/Release/QDP.app
```

For the iPhone or iPad app, open the project in Xcode, choose the `QDP-iOS`
scheme and your device, choose your team under Signing & Capabilities, and
run it:

```bash
open QDP.xcodeproj
```

Rebuild the core with `./build-core.sh` after changing its Rust code; Xcode
picks it up on its next build. The Android app builds on a Mac as on
[Linux](#on-linux).

### On Windows

Install, from a terminal, the Visual Studio C++ build tools, which Rust uses
to compile and link; Rust; NASM; Git; and, for the Windows app, the .NET 8
SDK:

```powershell
winget install Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

```powershell
foreach ($id in 'Rustlang.Rustup', 'NASM.NASM', 'Git.Git', 'Microsoft.DotNet.SDK.8') { winget install --exact --id $id }
```

NASM's installer does not add itself to the `PATH`: add
`C:\Program Files\NASM` to your user's, then open a new terminal, which has
it and the other installs' changes:

```powershell
[Environment]::SetEnvironmentVariable('Path', [Environment]::GetEnvironmentVariable('Path', 'User') + ';C:\Program Files\NASM', 'User')
```

Build `qdp.exe` and `qdp-gateway.exe`:

```powershell
cargo build --release
```

Build the Windows app, which builds its Rust core first, into
`windows\publish\QDP.exe`, and zips it into `qdp-windows-app-x64.zip`:

```powershell
.\windows\build.ps1
```

The installers need the [WiX Toolset](https://wixtoolset.org) 5, a .NET
tool, and the gateway's installer its firewall and utility extensions:

```powershell
dotnet tool install --global wix --version 5.0.2
```

```powershell
wix extension add --global WixToolset.Firewall.wixext/5.0.2 WixToolset.Util.wixext/5.0.2
```

Build the app's installer, with `qdp.exe`, after `windows\build.ps1`, and the
gateway's; the last number is the installer's build, which each upgrade
raises:

```powershell
.\packaging\windows\build-msi.ps1 app qdp-windows-x64.msi 1
```

```powershell
.\packaging\windows\build-msi.ps1 gateway qdp-gateway-windows-x64.msi 1
```

## Trying it

Build the `qdp` command as [Building](#building) says.

On the host, serve. The host creates its key in `./qdp-identity` the first
time, listens on UDP port 4433 unless given `--listen`, and advertises itself
on the local network with DNS-SD as `_qdp._udp`, so that clients such as the
Android app list it (through Avahi on Linux; `--advertise off` stops it):

```bash
./target/release/qdp serve
```

On the client, pair with the host the first time: connect with `--pair`, and
type the four-digit code the host shows (Section 4.4.9 of the spec): it prints
the code, and on Linux, macOS and Windows shows it on the screen too. Add
`:PORT` to the address for another port, and `--name` to choose the name the
host keeps for this device. The stream opens in a window, and closing the
window disconnects:

```bash
./target/release/qdp connect 192.0.2.7 --pair
```

After that the two remember each other: the client keeps the host's key in
`./qdp-client-identity/paired-hosts`, the host the client's in
`./qdp-identity/paired-clients`, and connecting needs only the address:

```bash
./target/release/qdp connect 192.0.2.7
```

A code is good for 120 seconds and one try at a time; after three wrong codes
in a row the host refuses pairing for 1, then 3, then 5 minutes, and twice as
long after each further wrong code. `qdp clients` lists the paired clients, and
`qdp clients remove NAME` forgets one, by its name or the start of its pin; on
the client, `qdp hosts` lists the paired hosts, and `qdp hosts remove ADDRESS`
forgets one, by its address or the start of its pin.

A host can also be served with a token, as before pairing, and a client given
its pin: `qdp pin` prints the host's pin, `qdp serve --token my-secret` accepts
the token beside paired clients, and `--pairing off` turns pairing off:

```bash
./target/release/qdp connect 192.0.2.7 --pin PIN --token my-secret
```

A host inside an organization's network can also accept the users a
`qdp-gateway` signs in with the organization's directory and a TOTP code
([Section 13](docs/spec/13-gateway.md) of the spec): `--gateway-key KEY`
trusts the gateway whose assertion key `qdp-gateway keys` prints, and
`--gateway-group NAME` admits only its users in that group:

```bash
./target/release/qdp serve --gateway-key KEY --gateway-group "QDP Users"
```

`qdp discover` lists the hosts advertising on the local network, through Avahi
on Linux, with their addresses and whether they offer pairing; with `--watch`
it writes a line as each comes and goes, until its standard input closes, for
an app to follow (see [its lines](crates/qdp-cli/src/discover.rs)). An app can
drive `qdp connect` for its stream window with `--events`, which reports the
session's progress on standard output as the lines the apps' cores report (see
[`qdp-app`'s events](crates/qdp-app/src/events.rs)), such as `connected`,
`pair first`, `paired PIN` and `failed ERROR`, and reads the code the host
shows from standard input after each `pair` line.

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
Windows. `qdp serve --lock off` leaves the desktop as it is. While a session
streams the screen, a notice says that a device sees and controls it, so that
whoever is at the machine knows. On Linux it is a desktop notification, and on
Windows a banner at the top of the main display, over every window, which screen
captures leave out, so that the stream does not show it; both go when the
session ends. On macOS it is a notification in Notification Center, which shows
it as the user's settings say. On Windows, `qdp serve --privacy on` also covers
every display while a session streams the screen, saying that the computer is in
use from another device, and ignores its own keyboard and mouse, so that only
the device streaming sees and controls it; Ctrl+Alt+Delete still works. The
covers stay out of the stream, which needs Windows 10 version 2004 or later.

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

On Windows the host encodes on NVIDIA GPUs that encode AV1 (GeForce RTX 40 or
newer) with NVENC, through the NVIDIA driver (version 522 or later); on AMD
(Radeon RX 7000 or newer) and Intel (Arc or newer) GPUs with the AV1 encoder
their driver registers with Media Foundation; and otherwise in software. Each
hardware encoder is used only once its first frame is checked to suit clients,
and the log names the one used. Hardware encoders on Windows recover from
losses with a key frame.

With hardware encoding on Linux, the screen stays in GPU memory from the compositor to
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
as far as the pointer goes. The RPM and Debian package below allow both for
the user at the machine and members of the `input` group. Otherwise, to allow both for members
of the `input` group:

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
the host runs as administrator too, and from the secure desktop (the sign-in
and lock screens, Ctrl+Alt+Delete and elevation prompts) unless the host runs
as SYSTEM in the console's session. Such a host follows the secure desktop
whenever it shows, capturing and controlling it like any other, and turns
Ctrl+Alt+Delete pressed on the client into the real one with `SendSAS`, which
needs the SoftwareSASGeneration policy to allow services.

The host streams its screen. On Linux it asks the desktop, through the
ScreenCast portal that GNOME, KDE and wlroots compositors provide, which
monitor to share; the first time, the desktop shows a dialog, and the choice is
remembered in the identity directory for later sessions. On macOS the host
captures the main display with ScreenCaptureKit; the first time, macOS asks to
allow Screen Recording for the terminal or app running `qdp`, and performing
the client's input needs Accessibility to be allowed for it as well, both under
System Settings, Privacy & Security. On Windows 10 and later the host captures
the primary monitor with DXGI Desktop Duplication; unless it runs as SYSTEM, it
holds the last image while the sign-in screen or an elevation prompt is shown. On Linux and macOS
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

On Linux, `qdp serve --source console`, run as root, streams the console
instead: whatever the graphics card shows on its first display that is on,
read from the framebuffers it scans out through DRM. Unlike the portal, which
exists only inside a desktop session, it shows the login screen, the lock
screen, any desktop and the text console, and follows the display as the login
screen hands it to the desktop someone logs into, so that a device can log in
and unlock as at the machine. Input reaches whatever shows through uinput,
whoever's session it is, since driving the console is what the host is for. The
frames stay in GPU memory for the hardware encoder, which the console needs
unless its buffers are linear, including the 10-bit colour KDE and others scan
out, which the GPU's video processor converts, and the pointer is read from the
card's cursor plane. With `--match-display on`, the display takes the client's
mode and interface scale as it does when the host runs in the session: the
session helper asks the compositor, and the display gets its own back when the
session ends; at the login screen it keeps its own. A display must be connected
and on, a dummy plug standing in for one on a machine without a monitor. With several monitors, only the first
is streamed; the session helper tells where it is among them, so that the
pointer and touches land on it, while at the login screen the pointer assumes
it is the whole desktop. A console stream
plays the sound of the desktop session shown when it starts, through that
user's PipeWire, and presents the client's microphone there; one that starts at
the login screen has no sound until the device connects again. The clipboard
and the host's notices, such as pairing codes and that a device streams the
screen, reach the desktop session shown on the console through a helper the
host starts as its user, `qdp session-helper`, since a session's notification
service and clipboard serve only its own user; with no one logged in, pairing
codes show only in the log.

To serve the console from boot, before anyone logs in, run the host as a system
service. `sudo qdp service install` writes a systemd unit that runs
`qdp serve --source console` as root with the `serve` options given after it,
turns it on and starts it; a `qdp` run from outside the system's programs, such
as a build in a home directory, which SELinux keeps services from running, is
installed as `/usr/local/bin/qdp` for the service to run; `sudo qdp service status` shows how it is doing, and
`sudo qdp service uninstall` removes it. The RPM and Debian package ship the
same service, off until `sudo systemctl enable --now qdp`. The service keeps the host's key and
paired devices in `/var/lib/qdp`, so `sudo qdp clients --identity /var/lib/qdp`
lists them, and pairing codes show in the logged-in desktop and in
`journalctl -u qdp -f`. To keep the devices paired with a host run by hand, copy its
identity directory before the service first starts:

```bash
sudo install -d -m 700 /var/lib/qdp && sudo cp -a qdp-identity/. /var/lib/qdp/
```

On macOS, `qdp service install` runs the host as a LaunchAgent instead: it
starts at every login, in the user's session where macOS lets it capture the
screen, restarts if it fails, and is never throttled as background work. macOS
asks to allow Screen Recording and Accessibility for `qdp` the first time a
device connects. The agent keeps the host's key in
`~/Library/Application Support/qdp` and logs to `~/Library/Logs/qdp.log`;
`qdp service status` and `qdp service uninstall` show and remove it. It serves
only while someone is logged in: streaming the login window needs a signed
app, which QDP does not have yet.

## Android client

The Android app in [`android`](android) streams from a host to phones,
tablets and gaming handhelds running Android 10 or later on 64-bit ARM. It
decodes AV1 with the device's hardware decoder, straight into the screen, and
in software on devices without one. [Building](#on-linux) says how to build
it and install it on a device.

The app lists the hosts on the local network, as `qdp serve` advertises them,
and the hosts it paired with, as cards. Tapping a new host pairs with it: the
host prints a four-digit code, which the app asks for, and from then on tapping
it connects at once. A card's menu, which holding it also opens, forgets a
paired host. A host the app does not find can be added by its address, and the
dialog's Advanced section reaches it with its pin and a token. With the
organization's [gateway](#gateway) set in its settings, a host added by its ID
on the gateway or its name inside the network is reached through it, and
listed as such: the app asks for the user's password and a code from their
authenticator app, offering the first time to add the gateway to an
authenticator app on the phone, or showing its QR code for one on another
device, and later sessions sign in with the token the gateway gave. A
controller or D-pad drives every screen: A selects, B goes back, Y opens a
card's menu, X adds a host and Start opens settings.
The app streams full screen. A handheld's built-in controls and connected controllers are sent as
gamepads, with rumble. Touching the screen works as a trackpad by default: one
finger moves the pointer, a tap clicks, two fingers right-click or scroll, and
a press held still drags; the menu switches to sending touches as the host's
own. A mouse and keyboard work as on a computer. Back, or Select, Start and
both bumpers together, opens the menu, which also shows the on-screen keyboard
and statistics and disconnects. Its Controller tile, off as each stream
starts, shows an on-screen controller for games on a device without one: a
stick in each bottom corner, A, B, X and Y around the right one, each side's
trigger and bumper as a pill in its top corner, and View and Menu between
them, which the host sees as a gamepad of its own; while it shows, touches go
to it. A controller connected takes its place: it steps aside, and leaves the
menu, until the last controller disconnects. Leaving the app ends the session, and
returning connects again.

## Mac, iPhone and iPad apps

The apps in [`apple`](apple) stream from a host to Macs with Apple silicon and
to iPhones and iPads, running macOS or iOS 26 or later. They are SwiftUI apps
around the same Rust core as the Android app, [`qdp-apple`](crates/qdp-apple),
which decodes AV1 with VideoToolbox where the device has an AV1 decoder (M3
and later Macs, iPhone 15 Pro and later, iPads with an M4 or A17 Pro) and in
software otherwise, and draws with Metal on a thread of its own, as `qdp
connect` does on a Mac. [Building](#on-macos) says how to build them. On
iPhone and iPad the app is called Quick Desktop.

Both apps list the hosts on the local network and those they paired with;
choosing a new host pairs with it by the code the host shows, and choosing a
paired one streams at once. On a Mac each stream opens in a window of its own,
which can go full screen; clicking into it sends the keyboard and mouse, and
while the host's pointer is relative the mouse is captured until Control,
Option, Shift and Z are pressed together. On iPhone and iPad touches work as a
trackpad or as the host's touchscreen, a keyboard, mouse or trackpad attached
to an iPad work as on a computer, and controllers are sent as gamepads with
rumble; the button at the top corner, or Select, Start and both shoulder
buttons together, opens the stream's menu. The menu's Show Controller puts the
same on-screen controller as on Android over the stream, in Liquid Glass, for
games on an iPhone or iPad without one; a controller connected takes its
place there too.

## Windows app

The app in [`windows`](windows) streams from a host to PCs running Windows 10
or later on x86-64. It is a WinUI 3 app around a Rust core,
[`qdp-windows`](crates/qdp-windows), which decodes AV1 with the GPU through
D3D11 video where the GPU has an AV1 decoder (NVIDIA RTX 30 and later, AMD
RX 6000 and later, Intel Arc and Xe, Snapdragon X) and in software otherwise.
Each stream shows in a window of the core's own, drawn with a flip-model swap
chain by a render thread that takes pictures straight from the decoder, so
that full screen, the system hands it to the display without composing it.

Install it with the release's `qdp-windows-x64.msi`, which needs nothing else
installed first: the app carries its own .NET and Windows App SDK, and QDP's
programs their own C runtime. It installs the app in `C:\Program Files\QDP`
with a Start menu shortcut, and the `qdp` command, for hosting a PC, on the
PATH; installing a later build upgrades it in place, and Settings, Apps
removes it. The app also runs without installing from its folder, the
release's `qdp-windows-app-x64.zip`. [Building](#on-windows) says how to
build it and its installer.

The app lists the hosts on the local network and those it connected to
before; choosing a new host pairs with it by the code the host shows, and
choosing one paired with streams at once, full screen or in a window as its
settings say. A host added by its key pin and a token is listed too once it
streamed, its token kept in Windows' Credential Locker. With the
organization's [gateway](#gateway) set in its settings, a computer added by
its ID on the gateway or its name inside the network is reached through it,
and listed as such: the app asks for the user's password and a code from
their authenticator app, showing the first time the QR code that adds the
gateway to it, and later sessions sign in with the token the gateway gave,
kept in the Credential Locker until it expires.
The stream sends the keyboard, mouse, touch, pen and gamepads, with rumble;
in full screen the Windows key, Alt+Tab and Alt+F4 act on the host. Holding
Ctrl, Alt and Shift, Z releases the mouse, X switches between full screen and
a window, S shows or hides the stream's statistics, M turns the microphone on
or off, and Q disconnects.

## Linux app

The app in [`qdp-gtk`](crates/qdp-gtk) is QDP's for Linux desktops: a GTK 4
and libadwaita app, which GNOME uses and KDE Plasma and other desktops show
well, needing GTK 4.14 and libadwaita 1.5 or later (Ubuntu 24.04, Fedora 40
and later). It lists the hosts on the local network, which it follows with
`qdp discover --watch`, and those it paired with; choosing a new host pairs
with it by the code the host shows, and choosing a paired one streams at once,
in the window `qdp connect` opens, which the app runs with `--events` and
follows. A host set up with a key pin and a token is added by its address,
pin and token instead, the token kept in a file only the user may read and
passed to `qdp connect` in `QDP_TOKEN`, out of the command line. Its
preferences (Ctrl+,) choose the video's resolution and decoding, sound, the
microphone, the clipboard and touchpad gestures for every stream, and the
organization's [gateway](#gateway): its address, the user's name and, if its
administrator gives one, its key pin. With a gateway set, a computer added by
its ID on the gateway or its name inside the network is reached through it,
and listed under it; the app asks for the user's password and a code from
their authenticator app, showing the QR code that adds the gateway to the app
the first time, and later connections sign in with the session `qdp` keeps,
asking nothing until it expires. It keeps the client's key, the hosts it
paired with and its gateway sign-ins in `~/.local/share/qdp`, its settings
and gateway in `~/.config/qdp`, and runs the `qdp` installed beside it, or else
the one on the `PATH`. The release's `qdp-gtk-linux-x86_64.rpm`, with `qdp`'s RPM,
installs it with its desktop entry, icons and AppStream metadata, so that it
shows in the desktop's menus and software center; its
`qdp-gtk-linux-x86_64.tar.gz`, extracted into `~/.local`, installs it for one
user. [Building](#on-linux) says how to build it; run the build with
`./target/release/qdp-gtk`, or install it for yourself, with its menu entry
and icons:

```bash
packaging/linux/stage-gtk.sh target/release/qdp-gtk ~/.local && cp target/release/qdp ~/.local/bin/
```

## Gateway

`qdp-gateway` ([`crates/qdp-gateway`](crates/qdp-gateway)) lets users outside
an organization's network reach the QDP hosts inside it through one public
endpoint ([Section 13](docs/spec/13-gateway.md) of the spec). The organization
brings its own accounts: each user signs in with their domain credentials,
checked with its directory, LDAP or Active Directory over TLS, then always a
TOTP code from an authenticator app such as Google Authenticator, Microsoft
Authenticator or Authy. A user without one yet is shown a QR code to add it
to their app the first time, and asked for a code at every sign-in after;
`enrollment = "admin-only"` leaves enrolling to administrators instead. It
tunnels their QDP sessions, end to end, to the hosts it lists for them or
that its routes let them open by their names inside the network. It stands
apart from QDP's apps and hosts, and runs as a service on Linux and Windows
servers.

Deploying it takes one setting on each side, none of them per host:

1. On the gateway: the directory, and a route to the hosts' network, such as
   every name under `desktops.corp.example.com` in `10.20.0.0/16` for the
   group `QDP Users`.
2. On every host: the gateway's assertion key, which `qdp-gateway keys`
   prints, the same on every host, as `qdp serve --gateway-key KEY` (or in
   the service's options), which configuration management can set fleet-wide.

When a user opens a host, the gateway learns its key with a handshake of its
own inside the network, vouches for the user to that key, and tells the client
to verify the host by it. That trusts the network's names for hosts'
identities, as users inside it do; a host listed in `[[host]]` with the pin
`qdp pin` prints is verified by that pin instead, and
`learn_host_keys = false` vouches only for such hosts.

On Linux, install the release's `qdp-gateway-linux-x86_64.rpm` or, on Debian
and Ubuntu, `qdp-gateway-linux-x86_64.deb`, which make the same, or run
`sudo qdp-gateway service install`, which makes the user `qdp-gateway`, a
hardened systemd unit and, the first time, an example configuration in
`/etc/qdp-gateway/gateway.toml` to edit. On Windows, install the release's
`qdp-gateway-windows-x64.msi`: it installs the gateway, on the PATH, and its
service, which runs as LocalService from boot, with its configuration, keys
and log in `C:\ProgramData\qdp-gateway`, readable only by it and
administrators, an example configuration to edit there, and a firewall rule
for UDP port 443. Once the configuration is edited and the directory
account's password is in `directory-password` beside it, start the service;
upgrades keep the configuration and start it again. Without the installer,
`qdp-gateway service install` from an administrator's terminal makes the same
service and, once configured, opens the firewall for its port; run it after
moving the gateway off port 443 too. Then:

```bash
sudo qdp-gateway check                  # the configuration and directory account
sudo qdp-gateway login alice build-01   # alice's sign-in, and whether she may open build-01
sudo qdp-gateway totp enroll alice      # an otpauth:// URI for alice's authenticator app
sudo qdp-gateway keys                   # the pin clients pin, the key hosts trust
sudo qdp-gateway service install        # start it, now and at every boot
```

Users connect through it with `qdp connect`, naming the host by its ID on the
gateway or its name inside the network:

```bash
qdp connect build-01 --gateway gw.corp.example.com --user alice
```

The first time, it asks for alice's domain password and shows a QR code to
scan with her authenticator app, then a code from the app; a mistyped code is
asked for again with the same QR code. The gateway then vouches for her to the
host, and the client verifies the host's key with the pin the gateway knows,
refusing a host whose key differs from one it pinned itself. Later
connections sign in with the session token kept in the client's identity
directory, without asking, until it expires. `--gateway-pin` pins the
gateway's key, from `qdp-gateway keys`, where its certificate is not from a
certificate authority the system trusts.

## Builds and releases

GitHub Actions ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) checks
QDP on Linux, Windows and macOS: formatting, clippy and the tests. It runs only
when started by hand on a branch from the Actions tab, or when a version tag is
pushed; neither pull requests nor merges into `main` start it. It builds the
`qdp` command for each system and the Android, Apple and Windows apps, and once
every build of a tag's run passed it publishes them together as a GitHub
release. The newest release's files are always at
`https://github.com/ianmarmour/qdp/releases/latest/download/FILE`, such as
`qdp-android-arm64.apk`. On Linux, `qdp-linux-x86_64.rpm` packages the `qdp`
command for Fedora 40 or later and openSUSE Tumbleweed, with the udev rule and
kernel modules its input needs, and `qdp-linux-x86_64.deb` the same for Debian
12 or later and Ubuntu 22.04 or later:

```bash
sudo dnf install https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.rpm
```

```bash
curl -LO https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.deb
sudo apt install ./qdp-linux-x86_64.deb
```

[`packaging/linux/build-rpm.sh`](packaging/linux/build-rpm.sh) builds the RPM
from a built `qdp`, with `rpmbuild`, and
[`packaging/linux/build-deb.sh`](packaging/linux/build-deb.sh) the Debian
packages of `qdp` and `qdp-gateway`, with `dpkg-deb`. Pushing a version tag
publishes a release of that version:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

So that each Android build installs over the last, sign them all with one key.
Create it once, keep it safe, and give it to the repository as the secrets
`ANDROID_KEYSTORE_BASE64` (the keystore, base64-encoded),
`ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS` (`qdp` below) and
`ANDROID_KEY_PASSWORD`, under Settings, Secrets and variables, Actions:

```bash
keytool -genkeypair -keystore qdp-release.keystore -alias qdp -keyalg RSA -keysize 4096 -validity 10000
```

```bash
base64 -i qdp-release.keystore | pbcopy
```

Without the secrets, builds are signed with a debug key that CI keeps in its
cache between runs; if the cache loses it, the next build has a new key, and
installing it over an older one needs the app uninstalled first. Local builds take the same
key from `QDP_KEYSTORE`, `QDP_KEYSTORE_PASSWORD`, `QDP_KEY_ALIAS` and
`QDP_KEY_PASSWORD`.

## Crates

| Crate | Contents |
|---|---|
| `qdp` | The protocol: messages, session state machines, video and input logic, with no I/O |
| `qdp-quic` | QUIC transport with quinn and rustls |
| `qdp-codec` | Software AV1 encoding with rav1e and decoding with rav1d |
| `qdp-runtime` | Host and client media pipelines |
| `qdp-desktop` | Screen capture and input injection for Linux, macOS and Windows hosts |
| `qdp-cli` | The `qdp` command |
| `qdp-gtk` | The Linux app, in GTK 4 and libadwaita, which runs `qdp` for its streams |
| `qdp-android` | The Android app's Rust core, which its Kotlin code calls through JNI |
