# qdp

The Quick Desktop Protocol itself: its messages, wire format and session
logic, with no networking. It follows the
[specification](../../docs/spec/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together.

API documentation: `cargo doc -p qdp --open`

## Dependencies

The native tools and libraries it needs beyond `Cargo.toml`. A `+` means that
version or later.

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |

## License

Apache-2.0
