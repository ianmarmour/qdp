# qdp-codec

AV1 video and Opus audio for the Quick Desktop Protocol (QDP): encoding and
decoding in software everywhere, and in each platform's hardware where it
has it. QDP's video is AV1 only (Section 6.4 of the
[specification](../../docs/spec/06-video.md)).

## What's inside

| | Software | Hardware |
|---|---|---|
| Encoding | `Av1Encoder`, rav1e in low-latency mode | `VaapiEncoder` (VA-API on Linux); NVENC and Media Foundation on Windows |
| Decoding | `Av1Decoder`, rav1d | VideoToolbox (macOS, iOS), MediaCodec (Android), D3D11 video (Windows) |

- `open_encoder` and `open_decoder` open the platform's hardware encoder or
  decoder where it has one, and fall back to software otherwise.
- `ProcessDecoder` decodes in a separate process running
  `serve_decoder_on_stdio`, isolating the decoder from the client.
- `parse_obus` and `to_qdp_temporal_unit` check and convert AV1 bitstreams.
- Opus audio is coded in pure Rust.

Every encoder produces temporal units in the form QDP sends: no temporal
delimiters, a sequence header on every key frame, the minimum tile counts of
Section 6.4.4, and the colour description of standard dynamic range streams.

Hardware decoders hand their pictures over where they decoded them, such as a
VideoToolbox pixel buffer or a D3D11 texture, so that a window can draw them
without copying.

## Building

The software codecs use hand-written assembly, built with a C compiler and,
on x86-64, NASM. [Building](../../docs/building.md) lists what each system
needs.

## Examples and tests

- `tests/round_trip.rs` encodes frames with rav1e, carries them through
  QDP's video sender and receiver, and decodes them with rav1d.
- `examples/decoder_process_benchmark.rs` measures what decoding in a
  separate process costs:

  ```bash
  cargo run --release -p qdp-codec --example decoder_process_benchmark
  ```

## Documentation

```bash
cargo doc -p qdp-codec --open
```

## License

Apache-2.0, like the rest of [QDP](../../README.md).
