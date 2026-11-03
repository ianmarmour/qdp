# qdp-app

What QDP's client apps share: sessions with reconnection and pairing,
settings, and the on-screen controller.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-app --open`

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
