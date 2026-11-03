# qdp-windows

The Windows app's core. The app itself is in
[`windows`](../../windows/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-windows --open`

## Dependencies

The native tools and libraries it needs beyond `Cargo.toml`. A `+` means that
version or later.

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| Visual Studio C++ build tools | 2022+ | Windows | Building and linking |
| NASM | 2.15+ | Windows | Building the codecs |

## License

Apache-2.0
