# qdp-runtime

The host and client media pipelines of the Quick Desktop Protocol (QDP). It
ties together the protocol logic of [`qdp`](../qdp/README.md), the codecs of
[`qdp-codec`](../qdp-codec/README.md) and the transport of
[`qdp-quic`](../qdp-quic/README.md) into working endpoints.

## What's inside

- **`serve`** runs a host session. It authenticates the client with a
  `HostAuthenticator`, configures one monitor, and streams frames from a
  `FrameSource`: it encodes them, sends their fragments, answers
  retransmission requests, and recovers from loss. The pointer the source
  reports goes to the client as cursor images, shapes and warps, and the
  desktop's audio as Opus or PCM through a `HostAudio` input.
- **`run`** runs a client session. It authenticates with
  `ClientCredentials`, configures its display and input devices, decodes the
  host's video into a `FrameSink` on a thread of its own, and sends feedback
  and clock synchronization. `ClientAudio` devices play the host's audio
  through an adaptive jitter buffer and send the user's microphone.
- **`open_tunnel`** signs a client in to a gateway and opens a host through
  it, for `run` to connect to through the `Tunnel` it returns.
- **`TestPattern`** is a synthetic source, for hosts without screen capture
  and for tests.
- **Pairing** runs the pairing exchange: on a host, one exchange at a time,
  backing off after failures and telling the host's user each code; on a
  client, the prompt that asks its user for the code.

The operating system's side, such as screen capture and input, comes from
outside through traits: [`qdp-desktop`](../qdp-desktop/README.md) implements
them for desktops, and the apps' cores for their platforms.

## Tests

`tests/streaming.rs` streams from a host to a client over a real QUIC
connection on the loopback interface, end to end.

## Documentation

```bash
cargo doc -p qdp-runtime --open
```

## License

Apache-2.0, like the rest of [QDP](../../README.md).
