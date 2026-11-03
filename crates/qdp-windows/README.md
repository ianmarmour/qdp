# qdp-windows

The Windows app's core. The app itself is in
[`windows`](../../windows/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-windows --open`

## Dependencies

The tools and native libraries it needs beyond its Cargo dependencies, at
the oldest versions that work:

| Dependency | Version |
|---|---|
| Rust | 1.87 |
| Visual Studio C++ build tools | 2022 |
| NASM | 2.15 |

## License

Apache-2.0
