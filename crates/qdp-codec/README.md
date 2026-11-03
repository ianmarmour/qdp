# qdp-codec

Video and audio coding for QDP: AV1 and Opus, in software and in each
platform's hardware.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-codec --open`

## Dependencies

The native tools and libraries it needs beyond `Cargo.toml`. A `+` means that
version or later.

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |
| NASM | 2.15+ | x86-64 | Building the codecs |
| libva | 2+ | Linux | Hardware encoding, loaded at run time |
| NVIDIA driver | 522+ | Windows | Hardware encoding, loaded at run time |

## License

Apache-2.0
