# qdp-runtime

The host and client pipelines that turn QDP's protocol, transport and
codecs into working sessions.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp-runtime --open`

## Dependencies

The tools and native libraries it needs beyond its Cargo dependencies, at
the oldest versions that work:

| Dependency | Version |
|---|---|
| Rust | 1.87 |
| C compiler | Any |
| NASM, on x86-64 | 2.15 |

## License

Apache-2.0
