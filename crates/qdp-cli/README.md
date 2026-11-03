# qdp-cli

The `qdp` command: share this computer's desktop, or connect to one, with the
Quick Desktop Protocol (QDP).

- `qdp serve` shares the screen, or a test pattern.
- `qdp connect` shows a host's stream in a window, with the host's cursor as
  its pointer, or with `--headless` only logs statistics and can write the
  video to a file.
- `qdp discover`, `qdp pin`, `qdp clients`, `qdp hosts` and `qdp service`
  find hosts, show keys, manage paired devices and run the host as a service.

The user guides cover it: [Sharing a computer](../../docs/setup/hosting.md) and
[Using the apps](../../docs/setup/apps.md#the-command-line). `qdp help`
describes every option.

## Building

```bash
cargo build --release -p qdp-cli
```

[Building](../../docs/building.md) lists what each system needs.

## Features

| Feature | What it adds |
|---|---|
| `audio` | Sound and the microphone; on Linux it needs PipeWire's development files |
| `discovery` | Advertising hosts on the local network; on Linux through Avahi |
| `gamepad` | Sending the client's gamepads; on Linux it needs libudev's development files |
| `notifications` | Showing the host's pairing codes as desktop notifications |
| `screen-capture` | Capturing the host's screen; without it, hosts stream a test pattern |
| `window` | Showing the stream in a window; without it, `qdp connect` runs only headless, for machines that only serve |

All are on by default. `--no-default-features --features window` builds a
client without PipeWire or libudev.

## Code

| File | What it does |
|---|---|
| `src/args.rs` | The command line and its help text |
| `src/main.rs` | Each command: `qdp serve`, `qdp connect` and the rest |
| `src/window.rs`, `src/color.rs` | The stream window, and converting frames for it |
| `src/sink.rs` | What `qdp connect` does with frames until it has a window |
| `src/app_events.rs` | `qdp connect --events`, for apps that run `qdp connect` |
| `src/gateway.rs` | Signing in to a gateway from the terminal |
| `src/discover.rs` | `qdp discover` and the lines `--watch` writes |
| `src/identity.rs`, `src/paired.rs` | The host's and client's keys, and the devices they paired with |
| `src/service.rs` | `qdp service` on Linux and macOS |

## License

Apache-2.0, like the rest of [QDP](../../README.md).
