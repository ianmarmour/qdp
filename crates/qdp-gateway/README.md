# qdp-gateway

The QDP gateway, which gives people in an organization access to the
computers inside its network.
[Gateway Setup](../../docs/gateway.md) shows how to deploy it.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-gateway --open`

## Dependencies

The native tools and libraries it needs beyond `Cargo.toml`. A `+` means that
version or later.

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |

## License

Apache-2.0
