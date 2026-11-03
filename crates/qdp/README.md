# qdp

The Quick Desktop Protocol itself: its messages, wire format and session
logic, with no networking. It follows the
[specification](../../docs/spec/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp --open`

## Dependencies

The tools and native libraries it needs beyond its Cargo dependencies, at
the oldest versions that work:

| Dependency | Version |
|---|---|
| Rust | 1.87 |
| C compiler | Any |

## License

Apache-2.0
