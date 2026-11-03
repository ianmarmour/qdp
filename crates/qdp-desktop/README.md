# qdp-desktop

Each operating system's side of QDP: screen capture, input, audio, the
clipboard, discovery and notifications.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-desktop --open`

## Dependencies

The tools and native libraries it needs beyond its Cargo dependencies, at
the oldest versions that work:

| Dependency | Version |
|---|---|
| Rust | 1.87 |
| C compiler | Any |
| NASM, on x86-64 | 2.15 |
| libclang, on Linux | 11 |
| PipeWire, on Linux | 0.3 |
| libudev, on Linux | 1 |

## License

Apache-2.0
