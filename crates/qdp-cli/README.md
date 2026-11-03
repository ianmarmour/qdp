# qdp-cli

The `qdp` command, which shares a computer with `qdp serve` and connects to
one with `qdp connect`. [Initial Setup](../../docs/setup/README.md) shows how
to use it, and `qdp help` lists every option.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-cli --open`

## Usage

```text
Usage:
  qdp serve [options]          Serve this machine's desktop
  qdp connect HOST [options]   Connect to a host, by address or name,
                               with an optional :PORT (default 4433); through
                               a gateway, its ID there or its name inside the
                               organization's network
  qdp discover [--watch]       List the hosts advertising on the local network;
                               --watch writes a line as each comes and goes,
                               until standard input closes, for an app
  qdp pin [--identity DIR]     Print this host's key pin for clients
  qdp pin --client [--identity DIR]
                               Print this client's key pin for hosts
  qdp clients [--identity DIR] List the clients that paired with this host
  qdp clients remove WHICH [--identity DIR]
                               Forget a paired client, by its name or the
                               start of its key pin
  qdp hosts [--identity DIR]   List the hosts this client paired with
  qdp hosts remove WHICH [--identity DIR]
                               Forget a paired host, by its address or the
                               start of its key pin
  qdp service install [serve options]
                               Run the host as a service: on Linux a system
                               service from boot, streaming the console, the
                               login screen included (as root); on macOS a
                               LaunchAgent from login
  qdp service uninstall        Stop and remove the service
  qdp service status           Show whether the service runs
  qdp help                     Show this help

Serve options:
  --listen ADDRESS     Address to listen on (default 0.0.0.0:4433)
  --identity DIR       Directory holding the host's key (default ./qdp-identity)
  --token TOKEN        Accept clients presenting TOKEN (repeatable)
  --client-key HEX     Accept clients presenting the key HEX, from their
                       `qdp pin --client` (repeatable)
  --user NAME          Accept NAME with the password in QDP_PASSWORD
  --gateway-key HEX    Accept the users a qdp-gateway vouches for, whose
                       assertion key HEX `qdp-gateway keys` prints
                       (repeatable)
  --gateway-group NAME Accept only the gateway's users in the group NAME
                       (repeatable; default: every user it vouches for)
  --pairing on|off     Let new clients pair by typing a four-digit code that
                       this host shows in its log; paired clients are kept
                       in the identity directory (default on)
  --advertise on|off   Advertise this host on the local network with DNS-SD,
                       so that clients list it (default on)
  --source KIND        auto, screen, console or test-pattern (default auto: the
                       screen, or the test pattern if it cannot be captured);
                       console streams what the graphics card shows, the
                       login and lock screens included (Linux, as root)
  --size WIDTHxHEIGHT  Size of the test pattern (default 1280x720)
  --match-display on|off
                       Change the streamed monitor's resolution and scale to
                       the client's display for each session, as close as
                       its modes allow, and back afterwards; on KDE Plasma
                       and wlroots compositors such as sway and Hyprland
                       (default off: the host scales its frames instead)
  --fps N              Frames per second (default 60)
  --encoder KIND       auto, hardware or software (default auto: the GPU's
                       encoder when it has one, otherwise software)
  --gpu-capture on|off Hand the screen to the GPU's encoder in the GPU's own
                       memory, without copying it through the processor,
                       where the desktop and the driver allow; on Linux
                       (default on; off copies every frame through memory)
  --speed N            Software encoder speed preset, 0 to 10 (default 10)
  --quantizer N        Encoder quantizer, 1 to 255 (default 100)
  --clipboard WAY      both, send, receive or off: share the clipboard's text
                       both ways, only this machine's, only the client's, or
                       not at all (default both)
  --audio on|off       Stream this machine's sound, and present clients'
                       microphones to its applications where the platform
                       allows (default on)
  --passthrough WHICH  games, all or off: which HID devices clients may pass
                       through to this machine's applications (default games,
                       game controllers only; all includes keyboards, mice
                       and security keys)
  --resume SECONDS     How long a client whose connection dropped may
                       reconnect without its credentials, 0 for never
                       (default 300)
  --lock on|off        Lock this machine's desktop when a session ends other
                       than by its client disconnecting, as when the network
                       drops (default on)
  --privacy on|off     Cover this machine's displays and ignore its own
                       keyboard and mouse while a session streams the screen,
                       on Windows 10 version 2004 or later (default off)

Connect options:
  --pin HEX            The host's key pin, from `qdp pin` (default: the one
                       remembered from pairing with HOST)
  --pair               Pair with the host: type the four-digit code it shows,
                       after which the host and its key are remembered
  --name NAME          This device's name, which a host shows when pairing
                       (default: this machine's name)
  --token TOKEN        Authenticate with TOKEN; without --token, a token in the
                       QDP_TOKEN environment variable, which other users
                       cannot read as they can a command line
  --user NAME          Authenticate as NAME with the password in QDP_PASSWORD;
                       with --gateway, sign in to the gateway as NAME
  --gateway ADDRESS    Reach the host through the qdp-gateway at ADDRESS, a
                       name or address with an optional :PORT (default 443):
                       sign in with the directory's password, from
                       QDP_PASSWORD or asked for, and a code from an
                       authenticator app, whose QR code it shows the first
                       time; the gateway vouches for the user to the host
  --gateway-pin HEX    The gateway's key pin, from `qdp-gateway keys` (default:
                       its certificate, checked with the system's certificate
                       authorities)
  --identity DIR       Directory holding this client's key, with which it
                       authenticates when given neither --token nor --user,
                       the hosts it paired with, and its gateway sign-ins
                       (default ./qdp-client-identity)
  --server-name NAME   Name sent in the TLS handshake (default qdp-host)
  --frames N           Disconnect after N frames
  --output FILE        Write the decoded video to FILE as YUV4MPEG2
  --headless           Open no window; only log statistics
  --events             Report the session's progress as lines on standard
                       output, and read a pairing code, or a gateway's user
                       name, password and code, from standard input, for an
                       app that runs qdp connect for its stream
  --resolution WxH     Ask the host for video WIDTHxHEIGHT pixels, whatever
                       the window's size, and stretch it to fill the window,
                       to stream less at a large or full-screen window
                       (default: the window's size)
  --decoder KIND       auto, hardware or software (default auto: the GPU's
                       decoder when it has one, otherwise software)
  --decoder-process on|off
                       Decode the host's video in a separate process, so that
                       a decoder bug a malicious stream exploits cannot reach
                       the session; slower (default off)
  --clipboard WAY      both, send, receive or off: share the clipboard's text
                       both ways, only this machine's, only the host's, or
                       not at all (default both)
  --audio on|off       Play the host's sound (default on)
  --microphone         Send this machine's microphone to the host
  --touchpad           Send the touchpad's fingers for the host's own gestures,
                       rather than moving the pointer (Linux clients; needs
                       the input group)
  --passthrough ID     Pass the HID device VENDOR:PRODUCT, in hexadecimal as
                       lsusb shows it, through to the host as it is
                       (repeatable; Linux clients)

Set QDP_LOG to error, warn, info, debug or trace to change the log level, and
add TARGET=LEVEL after a comma to change it for one part, as in
QDP_LOG=info,qdp::window=debug.
```

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |
| NASM | 2.15+ | x86-64 | Building the codecs |
| PipeWire | 0.3+ | Linux | Screen capture and audio |
| libclang | 9+ | Linux | Building screen capture and audio |
| libudev | 1+ | Linux | Gamepads |
| libva | 2+ | Linux | Hardware encoding, loaded at run time |
| NVIDIA driver | 522+ | Windows | Hardware encoding, loaded at run time |

## Build

Set up your system as [Building](../../docs/building.md) describes, then:

```bash
cargo build --release -p qdp-cli
```

The command is built as `target/release/qdp`.

## Test

```bash
cargo test -p qdp-cli
```

## License

`qdp-cli` is licensed under the [Apache License 2.0](../../LICENSE).
