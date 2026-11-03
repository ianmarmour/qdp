# qdp-codec

Video and audio coding for QDP: AV1 and Opus, in software and in each
platform's hardware.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-codec --open`

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |
| NASM | 2.15+ | x86-64 | Building the codecs |
| libva | 2+ | Linux | Hardware encoding and decoding, loaded at run time |
| NVIDIA driver | 522+ | Windows | Hardware encoding, loaded at run time |

## License

`qdp-codec` is licensed under the [Apache License 2.0](../../LICENSE).
