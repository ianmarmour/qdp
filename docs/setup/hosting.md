# Sharing a computer

A **host** is a computer that shares its desktop. Any Linux, Windows or Mac
computer can host with the `qdp serve` command, which the
[installation guides](README.md#1-install-qdp) set up.

- [Getting started](#getting-started)
- [Pairing and signing in](#pairing-and-signing-in)
- [Platform notes](#platform-notes): [Linux](#linux), [Windows](#windows),
  [macOS](#macos)
- [Video](#video)
- [Sound and microphone](#sound-and-microphone)
- [Clipboard](#clipboard)
- [Input](#input)
- [Security during a session](#security-during-a-session)
- [Streaming the login screen](#streaming-the-login-screen)
- [Running as a service](#running-as-a-service)
- [Options](#options)

## Getting started

Start the host:

```bash
qdp serve
```

The first time, it creates its key in `./qdp-identity` (or the directory
`--identity` names). It listens on UDP port 4433, unless `--listen` gives
another address; if a firewall protects the host, allow that port.

Linux hosts advertise themselves on the local network with DNS-SD, as
`_qdp._udp` through Avahi, so that the apps list them; `--advertise off`
stops it. Windows and Mac hosts don't advertise yet, so add them in the apps
by their address.

On a client, choose the host and type the four-digit code it shows: see
[Using the apps](apps.md). From then on, the two remember each other and
connect without a code.

## Pairing and signing in

### Pairing

When a new device connects, the host shows a four-digit code, which the user
types on the device. The host prints the code in its log and, on Linux,
macOS and Windows, shows it on the screen too.

- A code is good for 120 seconds and one try at a time.
- After three wrong codes in a row, the host refuses pairing for 1 minute,
  then 3, then 5, and twice as long after each further wrong code.
- `--pairing off` turns pairing off.

The host keeps the devices it paired with in `paired-clients` in its identity
directory. To list them, or forget one by its name or the start of its key
pin:

```bash
qdp clients
qdp clients remove NAME
```

### Other ways to sign in

Besides pairing, a host can accept:

- **Tokens.** `qdp serve --token my-secret` accepts clients presenting the
  token. Since such a client hasn't paired, it also needs the host's key pin,
  which `qdp pin` prints:

  ```bash
  qdp connect 192.0.2.7 --pin PIN --token my-secret
  ```

- **Passwords.** Pass `--user NAME` on both sides, with the password in the
  `QDP_PASSWORD` environment variable. It's checked with
  SCRAM-SHA-256-PLUS, so it never crosses the network.
- **Client keys.** `qdp pin --client` on the client prints its key pin, and
  `qdp serve --client-key PIN` accepts it. A client given neither `--token`
  nor `--user` presents its key.
- **A gateway's users.** In an organization, `--gateway-key KEY` accepts the
  users a `qdp-gateway` vouches for, and `--gateway-group NAME` only those in
  that group. See [the gateway guide](../gateway.md).

## Platform notes

### Linux

- **Desktops.** The host captures the screen through the ScreenCast portal,
  which GNOME, KDE Plasma and wlroots compositors such as sway and Hyprland
  provide. The first time, the desktop asks which monitor to share, and the
  host remembers the choice in its identity directory.
- **Input** goes through uinput virtual devices, which work under any Wayland
  compositor or X11. The host needs write access to `/dev/uinput`, and to
  `/dev/uhid` for passed-through devices. The RPM and Debian package grant
  it; otherwise, follow [the manual setup](linux.md#input-without-the-packages).
  Without access, the host logs a warning and ignores input.
- **Multiple monitors.** The host finds where the streamed monitor sits among
  the desktop's monitors through xdg-output, and places the pointer on it.
- **Over SSH.** A host started where `WAYLAND_DISPLAY` isn't set finds the
  session's Wayland display in `XDG_RUNTIME_DIR` itself, and logs the one it
  uses.
- **Before anyone logs in**, the host can stream the console instead: see
  [Streaming the login screen](#streaming-the-login-screen).

### Windows

- **Capture.** The host captures the primary monitor with DXGI Desktop
  Duplication, on Windows 10 and later. Unless it runs as SYSTEM, it holds
  the last image while the sign-in screen or an elevation prompt shows.
- **Input** goes through `SendInput`. Windows keeps it from reaching:
  - programs run as administrator, unless the host runs as administrator too;
  - the secure desktop (the sign-in and lock screens, Ctrl+Alt+Delete and
    elevation prompts), unless the host runs as SYSTEM in the console's
    session. Such a host follows the secure desktop whenever it shows, and
    turns Ctrl+Alt+Delete pressed on the client into the real one with
    `SendSAS`, which needs the SoftwareSASGeneration policy to allow
    services.
- **Keyboard layout.** The host switches to the client's keyboard layout.
- **Privacy mode** can cover the host's displays during a session: see
  [Security during a session](#security-during-a-session).

### macOS

- **Permissions.** The host captures the main display with
  ScreenCaptureKit. The first time, macOS asks to allow **Screen Recording**
  for the terminal or app running `qdp`, and performing the client's input
  needs **Accessibility** too, both under System Settings, Privacy & Security.
- **Input** is posted as Quartz events.
- **Discovery.** Mac hosts don't advertise on the network yet: add them in
  the apps by their address.

## Video

### Resolution

The host sends video at the size of the client's window or display, up to its
own screen's, and scales its frames to fit. Clients can ask for less: see
[Using the apps](apps.md#resolution).

With `--match-display on`, the host instead changes the streamed monitor's
resolution and interface scale to the client's for each session, so that the
stream needs no scaling:

- The monitor takes the mode closest to the client's display, by shape and
  then by size, at the nearest refresh rate, and the client's scale. A
  headless or virtual monitor takes the client's size exactly.
- When the session ends, the monitor gets its own mode and scale back. They're
  saved in the identity directory while changed, so a host that stops without
  restoring them does so when it next starts.
- It works on KDE Plasma, through KWin's output management, and on sway,
  Hyprland, river, Wayfire, niri, COSMIC and other compositors that share the
  wlroots output management protocol.

### Frame rate

On Linux and macOS, the host encodes each frame as the compositor delivers
it, rather than at fixed times, so that frames don't wait. It still sends no
more than `--fps` frames a second (60 by default), and repeats the last frame
when nothing changes. Its statistics give `new_fps`, how many of the frames
sent showed a new screen.

### Encoding

Video is AV1. The host encodes on the GPU where it can, and in software
otherwise; it logs which encoder it uses, and `--encoder hardware` or
`--encoder software` forces one.

- **Linux** encodes through VA-API on GPUs that encode AV1: AMD RDNA 3 or
  newer, and Intel Arc or newer. It needs the GPU's VA-API driver: Mesa's
  (`mesa-va-drivers`) or Intel's media driver (`intel-media-driver`).
  `vainfo` lists `VAProfileAV1Profile0` with `VAEntrypointEncSlice` when
  hardware encoding is available. When packets are lost, the encoder
  recovers from the last frame the client decoded, a frame of ordinary size,
  rather than with a much larger key frame, on Mesa 24.3 or later and Intel's
  driver; the host's statistics count these as `recoveries`.
- **Windows** encodes with NVENC on NVIDIA GeForce RTX 40 or newer (driver
  522 or later), and with the AV1 encoder AMD's (Radeon RX 7000 or newer)
  and Intel's (Arc or newer) drivers provide through Media Foundation. Each
  hardware encoder is used only once its first frame checks out for clients.
  Windows' hardware encoders recover from losses with a key frame.
- **macOS** encodes in software.

`--speed` (0 to 10, default 10, the fastest) and `--quantizer` (1 to 255,
default 100) tune the software encoder.

#### Keeping frames on the GPU

With hardware encoding on Linux, the screen stays in GPU memory from the
compositor to the encoder: the desktop shares it as DMA-BUFs, and the GPU's
video processor converts and scales it for the encoder, so that no pixel
passes through the processor. This is what lets the host keep up with high
resolutions and frame rates, such as 4K at 120 frames a second.

It needs a desktop that shares its screen in GPU buffers (GNOME, KDE Plasma
and wlroots compositors do) and a VA-API driver with a video processor
(`VAEntrypointVideoProc` under `VAProfileNone` in `vainfo`). The host logs
"encoding frames straight from the GPU" when it does. Otherwise, or from the
moment the driver can't read a frame, it copies frames through memory;
`--gpu-capture off` always copies them.

### Test pattern

If the screen can't be captured, the host streams a test pattern instead.
`--source screen` or `--source test-pattern` forces one, and `--size` sets
the pattern's size (1280x720 by default).

## Sound and microphone

The host streams its sound, coded with Opus or sent as PCM; `--audio off`
turns it off.

- **Linux** captures what plays on the default output, through PipeWire.
- **Windows** captures the default output by loopback.
- **macOS 13 and later** captures the desktop's sound with ScreenCaptureKit,
  which needs Screen Recording to be allowed.

A client can also send its microphone. A Linux host presents it to its
applications as a source named "QDP microphone". Mac and Windows hosts can't
yet, since they'd need a virtual audio driver.

## Clipboard

The host and the client share their clipboards' text: text copied on either
side can be pasted on the other. Text a password manager marks as secret is
never shared. `--clipboard send` or `--clipboard receive` limits sharing to
one direction, and `--clipboard off` turns it off; clients have the same
option.

On Linux, the host uses the Wayland compositor's data control protocol where
it has one (KDE Plasma and wlroots compositors), and the X11 clipboard
otherwise, which GNOME keeps in step with its Wayland clipboard.

## Input

The host performs the client's keyboard, mouse, scrolling, touch, pen and
gamepad input. How it does so on each system is under
[Platform notes](#platform-notes).

- **The pointer.** The client draws the host's pointer as its own, so moving
  it has no network delay. Hosts report the pointer apart from the video on
  Windows, on macOS, and on Linux desktops whose ScreenCast portal offers
  cursor metadata, such as GNOME and KDE Plasma; on other Linux desktops it's
  part of the video.
- **Games.** When an application on a Windows host hides and confines the
  pointer, as games do, the client captures its pointer and sends raw mouse
  motion. Linux and Mac hosts can't tell when an application captures the
  pointer, so they do the same while one keeps the pointer hidden over the
  streamed monitor for half a second, and switch back once it has shown again
  for half a second with no button held.
- **Dragging.** While a button is held, the pointer reports the motion of
  each move along with its position, as a mouse does, so that games which
  follow the mouse's motion see the drag as far as the pointer goes.
- **Touch.** Touches reach Linux and Windows hosts as a touchscreen. Mac
  hosts decline them.
- **Gamepads.** A Linux host presents each of the client's gamepads as a
  virtual Xbox 360 controller, rumble included. Hosts on other systems
  decline them.
- **Passed-through devices.** A Linux host recreates the HID devices a client
  passes through (see [Using the apps](apps.md#passing-a-device-through))
  with uhid, for its games, rumble and lights included. By default it accepts
  only game controllers; `--passthrough all` accepts every device and
  `--passthrough off` none. Devices in a firmware update mode, such as
  bootloaders, are never passed through, so that the host's software can't
  reflash them.

## Security during a session

- **Who is watching.** While a session streams the screen, a notice tells
  whoever is at the machine that a device sees and controls it: a desktop
  notification on Linux, a notification in Notification Center on macOS, and
  on Windows a banner at the top of the main display, over every window, which
  the stream leaves out. They go when the session ends.
- **Locking.** When a session ends other than by the client disconnecting,
  because the network dropped, a newer session replaced it, or the host
  stopped, the host locks its desktop so that it isn't left unlocked: through
  logind on Linux, which GNOME, KDE Plasma and `swayidle` lock for, and with
  the system's own lock on macOS and Windows. `--lock off` leaves the desktop
  as it is.
- **Reconnecting.** If the connection drops, for example when the network
  changes, the client reconnects on its own with a single-use token the host
  gave it, without asking for credentials again. The token stays valid for
  five minutes after the connection ends; `--resume SECONDS` changes how
  long, and `--resume 0` turns this off.
- **Privacy mode (Windows).** `qdp serve --privacy on` covers every display
  while a session streams the screen, saying that the computer is in use from
  another device, and ignores the host's own keyboard and mouse, so that only
  the device streaming sees and controls it. Ctrl+Alt+Delete still works.
  The covers stay out of the stream. It needs Windows 10 version 2004 or
  later.

## Streaming the login screen

On Linux, `qdp serve --source console`, run as root, streams the console
instead of a desktop session: whatever the graphics card shows on its first
display that's on, read from the framebuffers it scans out through DRM.

Unlike the portal, which exists only inside a desktop session, the console
shows the login screen, the lock screen, any desktop and the text console. It
follows the display as the login screen hands it to the desktop someone logs
into, so that a device can log in and unlock as if at the machine. Input
reaches whatever shows, whoever's session it is.

- **Hardware.** The frames stay in GPU memory for the hardware encoder,
  which the console needs unless its buffers are linear. The GPU's video
  processor converts the 10-bit colour KDE Plasma and others scan out, and
  the pointer is read from the card's cursor plane.
- **A display must be connected and on.** A dummy plug can stand in for one
  on a machine without a monitor. With several monitors, only the first is
  streamed.
- **Matching the client's display.** With `--match-display on`, the display
  takes the client's mode and interface scale as it does in a desktop
  session, and gets its own back when the session ends. At the login screen
  it keeps its own.
- **Sound** comes from the desktop session shown when the stream starts,
  through that user's PipeWire, which also gets the client's microphone. A
  stream that starts at the login screen has no sound until the device
  connects again.
- **The desktop session's user.** The clipboard and the host's notices, such
  as pairing codes and the notice that a device streams the screen, reach the
  desktop session on the console through `qdp session-helper`, which the host
  starts as that session's user, since a session's notification service and
  clipboard serve only its own user. It also tells the host where the
  streamed monitor sits among several, so that the pointer and touches land
  on it; at the login screen, the pointer assumes the monitor is the whole
  desktop. With no one logged in, pairing codes show only in the log.

## Running as a service

### Linux

To serve the console from boot, before anyone logs in, run the host as a
system service:

```bash
sudo qdp service install
```

This writes a systemd unit that runs `qdp serve --source console` as root,
with any `serve` options given after `install`, turns it on and starts it. A
`qdp` run from outside the system's programs, such as a build in a home
directory, which SELinux keeps services from running, is installed as
`/usr/local/bin/qdp` for the service to run. `sudo qdp service status` shows
how it's doing, and `sudo qdp service uninstall` removes it.

The RPM and Debian package ship the same service, off until you turn it on:

```bash
sudo systemctl enable --now qdp
```

The service keeps the host's key and paired devices in `/var/lib/qdp`, so
`sudo qdp clients --identity /var/lib/qdp` lists them. Pairing codes show in
the logged-in desktop and in `journalctl -u qdp -f`. To keep the devices
paired with a host you ran by hand, copy its identity directory before the
service first starts:

```bash
sudo install -d -m 700 /var/lib/qdp && sudo cp -a qdp-identity/. /var/lib/qdp/
```

### macOS

On macOS, `qdp service install` runs the host as a LaunchAgent instead. It
starts at every login, in your session, where macOS lets it capture the
screen; it restarts if it fails, and macOS never throttles it as background
work. macOS asks to allow Screen Recording and Accessibility for `qdp` the
first time a device connects.

The agent keeps the host's key in `~/Library/Application Support/qdp` and
logs to `~/Library/Logs/qdp.log`. `qdp service status` and
`qdp service uninstall` show and remove it.

It serves only while someone is logged in: streaming the login window needs a
signed app, which QDP doesn't have yet.

### Windows

The host doesn't run as a Windows service yet: run `qdp serve` in your
session.

## Options

`qdp help` describes every option. The ones for `qdp serve`:

| Option | What it does | Default |
|---|---|---|
| `--listen ADDRESS` | Address to listen on | `0.0.0.0:4433` |
| `--identity DIR` | Directory holding the host's key and paired devices | `./qdp-identity` |
| `--pairing on\|off` | Let new devices pair with a code | `on` |
| `--advertise on\|off` | Advertise on the local network (Linux) | `on` |
| `--token TOKEN` | Accept clients presenting the token (repeatable) | |
| `--client-key PIN` | Accept the client with this key pin (repeatable) | |
| `--user NAME` | Accept NAME with the password in `QDP_PASSWORD` | |
| `--gateway-key KEY` | Accept the users this gateway vouches for (repeatable) | |
| `--gateway-group NAME` | Accept only the gateway's users in this group (repeatable) | every user |
| `--source KIND` | `auto`, `screen`, `console` or `test-pattern` | `auto` |
| `--size WxH` | Size of the test pattern | `1280x720` |
| `--match-display on\|off` | Change the monitor's mode and scale to the client's | `off` |
| `--fps N` | Most frames per second | `60` |
| `--encoder KIND` | `auto`, `hardware` or `software` | `auto` |
| `--gpu-capture on\|off` | Keep frames in GPU memory for the encoder (Linux) | `on` |
| `--speed N` | Software encoder speed, 0 to 10 | `10` |
| `--quantizer N` | Encoder quantizer, 1 to 255 | `100` |
| `--clipboard WAY` | `both`, `send`, `receive` or `off` | `both` |
| `--audio on\|off` | Stream sound and present microphones | `on` |
| `--passthrough WHICH` | `games`, `all` or `off`: which devices clients may pass through | `games` |
| `--resume SECONDS` | How long a dropped client may reconnect without credentials | `300` |
| `--lock on\|off` | Lock the desktop when a session ends unexpectedly | `on` |
| `--privacy on\|off` | Cover the displays during a session (Windows) | `off` |

`qdp service install` takes the same options after `install`.
