# qdp-android

The Android app's core. The app itself is in
[`android`](../../android/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-android --open`

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| Android NDK | r25+ | Linux, macOS | Building for Android |

## Build

On Linux or macOS, from the `android` folder, with `ANDROID_HOME` set to the
Android SDK:

```bash
./build-core.sh
```

This builds the core for 64-bit ARM, into the app. The app's own build runs
it too: see [`android`](../../android/README.md).

## Test

```bash
cargo test -p qdp-android
```

## License

`qdp-android` is licensed under the [Apache License 2.0](../../LICENSE).
