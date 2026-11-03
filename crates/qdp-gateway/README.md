# qdp-gateway

The QDP gateway, which gives people in an organization access to the
computers inside its network.
[Gateway Setup](../../docs/gateway.md) shows how to deploy it.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-gateway --open`

## Dependencies

The tools and native libraries it needs beyond its Cargo dependencies, at
the oldest versions that work:

| Dependency | Version |
|---|---|
| Rust | 1.87 |
| C compiler | Any |

## License

Apache-2.0
