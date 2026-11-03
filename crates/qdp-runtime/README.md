# qdp-runtime

The host and client pipelines that turn QDP's protocol, transport and
codecs into working sessions.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-runtime --open`

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |
| NASM | 2.15+ | x86-64 | Building the codecs |

## License

`qdp-runtime` is licensed under the [Apache License 2.0](../../LICENSE).
