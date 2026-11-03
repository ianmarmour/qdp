# qdp-android

> **Internal crate.** It exists for QDP's own programs, has no stable API,
> and isn't published to crates.io.

The Android app's Rust core, which the app's Kotlin code in
[`android`](../../android/README.md) calls through JNI.

The app shows a session in a `SurfaceView` and hands its surface to the core,
which runs the session on a thread of its own. Video is decoded by MediaCodec
straight into the surface or, on a device without an AV1 decoder, by rav1d
and drawn there on the processor. Audio plays and captures through AAudio.
The app passes on the user's keys, pointer, touches and gamepads, and reads
the session's events: its progress, the video's size, the host's cursor and
pointer mode, and gamepad rumble.

The session itself, its events and settings come from
[`qdp-app`](../qdp-app/README.md), shared with the other apps.

## Building

The app's build runs `android/build-core.sh`, which builds the core as
`libqdp_android.so` for 64-bit ARM. To check it alone:

```bash
rustup target add aarch64-linux-android
cargo clippy --target aarch64-linux-android -p qdp-android --all-targets
```

The JNI functions, logging and drawing exist only on Android; the rest builds
everywhere, so that the workspace's tests cover it.

## License

Apache-2.0, like the rest of [QDP](../../README.md).
