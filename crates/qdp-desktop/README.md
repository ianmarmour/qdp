# qdp-desktop

Each operating system's side of QDP: screen capture, input, audio, the
clipboard, discovery and notifications.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-desktop --open`

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |
| NASM | 2.15+ | x86-64 | Building the codecs |
| PipeWire | 0.3+ | Linux | Screen capture and audio |
| libclang | 9+ | Linux | Building screen capture and audio |
| libudev | 1+ | Linux | Gamepads |

## License

`qdp-desktop` is licensed under the [Apache License 2.0](../../LICENSE).
