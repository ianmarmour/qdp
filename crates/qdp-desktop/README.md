# qdp-desktop

Each operating system's side of QDP: screen capture, input, audio, the
clipboard, discovery and notifications.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-desktop --open`

## Dependencies

The native tools and libraries it needs beyond `Cargo.toml`. A `+` means that
version or later.

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |
| NASM | 2.15+ | x86-64 | Building the codecs |
| PipeWire | 0.3+ | Linux | Screen capture and audio |
| libclang | 9+ | Linux | Building screen capture and audio |
| libudev | 1+ | Linux | Gamepads |

## License

Apache-2.0
