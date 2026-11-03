# Connecting to a computer

A **client** shows a host's desktop and sends it your keyboard, mouse, touch
and controllers. QDP has an app for each platform, and the `qdp connect`
command for Linux, Windows and macOS.

- [Finding and pairing](#finding-and-pairing)
- [The apps](#the-apps): [Android](#android), [iPhone and iPad](#iphone-and-ipad),
  [Mac](#mac), [Windows](#windows), [Linux](#linux)
- [Through your organization's gateway](#through-your-organizations-gateway)
- [The command line](#the-command-line)

## Finding and pairing

Every app lists two kinds of computers:

- **Your computers**, the ones this device paired with. Choosing one streams
  from it at once. The Mac, iPhone, iPad and Android apps show whether each is
  online by checking it at its address, so this works away from home too, as
  long as the address is reachable from where you are.
- **Nearby computers**, the hosts advertising on the local network. Linux
  hosts advertise; Windows and Mac hosts don't yet, so add them by their
  address.

To pair, choose a nearby computer, or add one by its address, and type the
four-digit code the host shows. From then on, the two remember each other.
To forget a paired computer, use its menu in the list.

A computer that isn't on the local network, such as one reached over a VPN,
can be added by its address. Apps also accept a host's key pin and a token
instead of pairing, for hosts set up that way (see
[Hosting](hosting.md#other-ways-to-sign-in)).

## The apps

### Android

The app streams full screen, on phones, tablets and gaming handhelds.

- **Touch** works as a trackpad by default: one finger moves the pointer, a
  tap clicks, two fingers right-click or scroll, and a press held still
  drags. The menu switches to sending touches as the host's own touchscreen.
- **Controllers.** A handheld's built-in controls and connected controllers
  are sent as gamepads, with rumble. A controller or D-pad also drives every
  screen of the app: A selects, B goes back, Y opens a computer's menu, X adds
  a computer, and Start opens settings.
- **Mouse and keyboard** work as on a computer.
- **The menu** opens with Back, or with Select, Start and both bumpers
  together. It shows the on-screen keyboard and statistics, switches the touch
  mode, and disconnects.
- **On-screen controller.** The menu's Controller tile shows a controller
  over the stream, for games on a device without one: a stick in each bottom
  corner, A, B, X and Y around the right stick, each side's trigger and bumper
  as a pill in its top corner, and View and Menu between them. While it shows,
  touches go to it. Connecting a real controller hides it until the last
  controller disconnects.

Leaving the app ends the session, and returning connects again.

### iPhone and iPad

The app streams full screen.

- **Touch** works as a trackpad or as the host's touchscreen, as Settings or
  the stream's menu chooses.
- **A keyboard, mouse or trackpad** attached to an iPad works as on a
  computer.
- **Controllers** are sent as gamepads, with rumble.
- **The menu** opens with the button in the top corner, or with Select, Start
  and both shoulder buttons together. It shows the on-screen keyboard, with a
  row of Esc, Tab and arrow keys above it, switches the touch mode, shows
  statistics, and disconnects.
- **On-screen controller.** The menu's Show Controller puts the same
  on-screen controller as on Android over the stream. Connecting a real
  controller hides it.

Settings choose the stream's resolution, whether to send the microphone, the
touch mode, and the decoder.

### Mac

Each stream opens in a window of its own, which can go full screen. Click
into it to send the keyboard and mouse. While the host's pointer is captured,
as in games, the window keeps the mouse until you press Control, Option,
Shift and Z together.

### Windows

The app streams full screen or in a window, as its settings say.

- **Input.** The stream sends the keyboard, mouse, touch, pen and gamepads,
  with rumble. In full screen, the Windows key, Alt+Tab and Alt+F4 act on the
  host.
- **Shortcuts.** Hold Ctrl, Alt and Shift, then press:

  | Key | Action |
  |---|---|
  | Z | Release the mouse |
  | X | Switch between full screen and a window |
  | S | Show or hide the stream's statistics |
  | M | Turn the microphone on or off |
  | Q | Disconnect |

A host added by its key pin and a token is listed once it has streamed, with
its token kept in Windows' Credential Locker.

### Linux

The QDP app follows the hosts on the local network and lists those it paired
with. Each stream opens in the window of `qdp connect`, which the app runs,
so the [command line's](#the-command-line) keys and options apply.

- **Preferences** (Ctrl+,) choose the video's resolution and decoding, sound,
  the microphone, the clipboard, and touchpad gestures for every stream, and
  your organization's gateway.
- **Hosts set up with a token** are added by their address, key pin and
  token. The token is kept in a file only you can read, and passed to
  `qdp connect` in the `QDP_TOKEN` environment variable, out of the command
  line.
- **Where it keeps things.** The client's key, paired hosts and gateway
  sign-ins are in `~/.local/share/qdp`, and its settings in `~/.config/qdp`.

## Through your organization's gateway

An organization can let you reach computers inside its network through a
[QDP gateway](gateway.md). To use one:

1. In the app's settings, enter the gateway's address and your user name, as
   your administrator gives them, and its key pin if they give one.
2. Add a computer by its ID on the gateway, or by its name inside the
   network. It's listed as reached through the gateway.
3. The first time you connect, the app asks for your password and shows a QR
   code. Scan it with an authenticator app, such as Google Authenticator,
   Microsoft Authenticator or Authy, then type the code it shows. On Android,
   the app can also add the gateway to an authenticator app on the same
   phone.

Later connections sign in with the session the gateway gave, without asking,
until it expires. The gateway vouches for you to the computer, and the app
checks the computer's key with the pin the gateway knows.

## The command line

`qdp connect` shows a host's stream in a window; closing the window
disconnects. `qdp help` describes every option.

### Pairing

The first time, connect with `--pair`, and type the four-digit code the host
shows. Add `:PORT` to the address for a port other than 4433, and `--name`
to choose the name the host keeps for this device:

```bash
qdp connect 192.0.2.7 --pair
```

After that, connecting needs only the address:

```bash
qdp connect 192.0.2.7
```

The client keeps the hosts it paired with in
`./qdp-client-identity/paired-hosts` (or the directory `--identity` names).
To list them, or forget one by its address or the start of its key pin:

```bash
qdp hosts
qdp hosts remove ADDRESS
```

### In the window

- The client sends its keyboard, mouse and scrolling while the window has
  focus. When the host captures the pointer, as games do, the window captures
  yours and sends raw mouse motion; Ctrl+Alt+Shift+Z releases it, and a click
  captures it again.
- Ctrl+Alt+Shift+M turns the microphone on or off, and the window's title
  says when it's on.
- The window sends the gamepads connected to this computer while it has
  focus, and plays the rumble the host's games set on them.
- Touches on the window reach the host as a touchscreen. On Windows clients,
  a pen's pressure, tilt, rotation, eraser and barrel button reach the host as
  a pen; on other clients a pen moves the pointer.

### Resolution

The host sends video at the window's size, up to its screen's. To stream less
at a large or full-screen window, `--resolution 1920x1080` asks for that size
whatever the window's, and the client stretches the picture to fill the
window, keeping its shape. The picture is softer, but the host encodes and
the client decodes far fewer pixels. The client then asks for an interface
scale smaller by as much as the picture is stretched, rounded to 25 %, so
that the host's text shows at the size the client's own would when the host
matches it.

### Decoding and drawing

The client decodes on the GPU where it can: on Macs with an AV1 decoder
(Apple M3 and newer) through VideoToolbox, and on Windows through D3D11
video. It decodes in software elsewhere, or if the hardware can't handle a
stream. `--decoder hardware` or `--decoder software` forces one.

On macOS, the window draws each picture with Metal straight from the buffer
VideoToolbox decoded it into, so that no pixel is copied between decoding and
the screen, and asks a ProMotion display for up to 120 Hz. Elsewhere, or if
Metal can't start, it converts and scales frames on the processor. The client
logs each second how many frames it drew and how long drawing took.

`--decoder-process on` decodes in a separate process, so that a decoder bug a
malicious host's stream exploits can't reach the session's keys, clipboard or
input; a decoder process that fails is replaced. It costs some speed, since
each picture is copied between the processes;
`cargo run --release -p qdp-codec --example decoder_process_benchmark`
measures how much on a given machine.

### Sound, microphone and clipboard

- The client plays the host's sound; `--audio off` turns it off.
- `--microphone` sends this computer's microphone to the host.
- The clipboard's text is shared both ways; `--clipboard send`, `receive` or
  `off` limits it.

### Linux devices

- **Touchpad gestures.** With `--touchpad`, a Linux client in the `input`
  group sends its touchpad's raw fingers, so that a Linux host's own gestures
  apply. The touchpad then works only on the host while the window has focus.

#### Passing a device through

A Linux client can pass a HID device through to the host as it is, with
`--passthrough VENDOR:PRODUCT`, the IDs `lsusb` shows, such as `054c:09cc`.
It needs read and write access to the device's `/dev/hidraw*` node, as the
udev rules for game controllers grant. The host decides which devices it
accepts: see [Hosting](hosting.md#input).

### Without a window

`--headless` opens no window and only logs statistics, and `--output
session.y4m` also writes the decoded video to a YUV4MPEG2 file, which ffmpeg
and most players open. `--frames N` disconnects after N frames.

### Signing in

| Option | What it does |
|---|---|
| `--pin PIN` | The host's key pin, from `qdp pin`, for a host this client hasn't paired with |
| `--token TOKEN` | Authenticate with a token; without it, a token in `QDP_TOKEN`, which other users can't read as they can a command line |
| `--user NAME` | Authenticate as NAME with the password in `QDP_PASSWORD` |
| `--identity DIR` | Directory holding this client's key, paired hosts and gateway sign-ins (default `./qdp-client-identity`) |
| `--gateway ADDRESS` | Reach the host through a gateway: see [the gateway guide](gateway.md#connecting-through-it) |
| `--gateway-pin PIN` | The gateway's key pin, where its certificate isn't from a trusted certificate authority |
| `--server-name NAME` | Name sent in the TLS handshake (default `qdp-host`) |

### For app developers

- `qdp discover` lists the hosts advertising on the local network, with their
  addresses and whether they offer pairing. With `--watch`, it writes a line
  as each comes and goes, until its standard input closes, for an app to
  follow ([the lines](../crates/qdp-cli/src/discover.rs)).
- `qdp connect --events` reports the session's progress on standard output,
  as lines such as `connected`, `pair first`, `paired PIN` and
  `failed ERROR` ([the events](../crates/qdp-app/src/events.rs)), and reads
  the code the host shows from standard input after each `pair` line. The
  Linux app drives `qdp connect` this way.
