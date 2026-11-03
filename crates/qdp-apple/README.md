# qdp-apple

> **Internal crate.** It exists for QDP's own programs, has no stable API,
> and isn't published to crates.io.

The core of the Mac, iPhone and iPad apps, which their Swift code in
[`apple`](../../apple/README.md) calls through the C interface in
[`include/qdp.h`](include/qdp.h).

A session is [`qdp-app`](../qdp-app/README.md)'s, as in the other apps. Its
video is decoded by VideoToolbox, or by rav1d where the device has no AV1
decoder, and drawn with Metal on a thread of its own, into a layer added over
the one the app gives. Pictures go from the session's thread straight to the
render thread, never through the main thread. The host's cursor is drawn over
the picture where the system draws no pointer, as with touch input on iOS.

## Building

`apple/build-core.sh` builds the core for iPhone and iPad, the iOS simulator
and Macs with Apple silicon, as `apple/Generated/QDPCore.xcframework`, which
Swift imports as `QDPCore`. [`apple`](../../apple/README.md) covers the rest.

The C functions exist only on macOS and iOS; the picture's placement they use
comes from `qdp-app`, which the workspace's tests cover.

## License

Apache-2.0, like the rest of [QDP](../../README.md).
