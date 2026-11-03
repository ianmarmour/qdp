# qdp-cli

The `qdp` command, which shares a computer with `qdp serve` and connects to
one with `qdp connect`. [Initial Setup](../../docs/setup/README.md) shows how
to use it, and `qdp help` lists every option.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-cli --open`

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

## License

Apache-2.0
