# qdp-windows

The Windows app's core. The app itself is in
[`windows`](../../windows/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-windows --open`

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| Visual Studio C++ build tools | 2022+ | Windows | Building and linking |
| NASM | 2.15+ | Windows | Building the codecs |

## Build

On Windows, from the repository's root:

```powershell
cargo rustc -p qdp-windows --lib --release --crate-type cdylib
```

This builds the core as `qdp_windows.dll`. The app's build does the same:
see [`windows`](../../windows/README.md).

## Test

```bash
cargo test -p qdp-windows
```

## License

`qdp-windows` is licensed under the [Apache License 2.0](../../LICENSE).
