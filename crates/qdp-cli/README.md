# qdp-cli

The `qdp` command, which shares a computer with `qdp serve` and connects to
one with `qdp connect`. [Initial Setup](../../docs/setup/README.md) shows how
to use it, and `qdp help` lists every option.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-cli --open`

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
| libva, for hardware encoding on Linux | 2 |
| NVIDIA driver, for hardware encoding on Windows | 522 |

## License

Apache-2.0
