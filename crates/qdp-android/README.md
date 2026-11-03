# qdp-android

The Android app's core. The app itself is in
[`android`](../../android/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-android --open`

## Dependencies

The native tools and libraries it needs beyond `Cargo.toml`. A `+` means that
version or later.

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| Android NDK | r25+ | Linux, macOS | Building for Android |

## License

Apache-2.0
