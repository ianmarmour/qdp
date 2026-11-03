# Architecture

QDP is a Cargo workspace of Rust crates, plus a native app for each client
platform built around a Rust core. The [specification](spec/README.md)
defines the protocol itself.

## A session

A **host** shares its desktop, and a **client** shows it. A session between
them is one QUIC connection:

1. The client connects, checks the host's key, and signs in: with its own key,
   a pairing code the first time, a token, a password, or a gateway's word.
2. The host captures its screen, encodes it as AV1, and sends each frame as
   datagrams. The client reassembles, decodes and draws it, and asks again for
   anything lost.
3. Both sides share a clock, and the client reports when each frame reached the
   screen, so that the host can pace itself to the client and the network.
4. Sound flows both ways, the client draws the host's pointer itself, input
   goes to the host, and the clipboard is shared on request.

## Crates

Each layer builds on the ones above it:

| Layer | Crates |
|---|---|
| Protocol | [`qdp`](../crates/qdp/README.md) |
| Transport and codecs | [`qdp-quic`](../crates/qdp-quic/README.md), [`qdp-codec`](../crates/qdp-codec/README.md) |
| Pipelines | [`qdp-runtime`](../crates/qdp-runtime/README.md) |
| Integration | [`qdp-desktop`](../crates/qdp-desktop/README.md), [`qdp-app`](../crates/qdp-app/README.md) |
| Programs | [`qdp-cli`](../crates/qdp-cli/README.md), and the apps' cores: [`qdp-android`](../crates/qdp-android/README.md), [`qdp-apple`](../crates/qdp-apple/README.md), [`qdp-windows`](../crates/qdp-windows/README.md) |

Three stand apart: [`qdp-gateway`](../crates/qdp-gateway/README.md), the
gateway server, [`qdp-gtk`](../crates/qdp-gtk/README.md), the Linux app,
which runs the `qdp` command for its streams, and
[`qdp-bench`](../crates/qdp-bench/README.md), which measures QDP, RDP and VNC
for the [benchmarks](../bench/README.md).

## Apps

| App | Built with | Core |
|---|---|---|
| [Android](../android/README.md) | Kotlin and Jetpack Compose | `qdp-android` |
| [Mac, iPhone and iPad](../apple/README.md) | SwiftUI | `qdp-apple` |
| [Windows](../windows/README.md) | WinUI 3 and C# | `qdp-windows` |
| [Linux](../crates/qdp-gtk/README.md) | GTK 4 and libadwaita | the `qdp` command |

Each app decodes video on the GPU where the device can, and draws it without
going through the app's main thread.
