# qdp-codec

Video and audio coding for QDP: AV1 and Opus, in software and in each
platform's hardware.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-codec --open`

## Dependencies

The tools and native libraries it needs beyond its Cargo dependencies, at
the oldest versions that work:

| Dependency | Version |
|---|---|
| Rust | 1.87 |
| C compiler | Any |
| NASM, on x86-64 | 2.15 |
| libva, for hardware encoding on Linux | 2 |
| NVIDIA driver, for hardware encoding on Windows | 522 |

## License

Apache-2.0
