# qdp-gateway

The QDP gateway, which gives people in an organization access to the
computers inside its network.
[Gateway Setup](../../docs/gateway.md) shows how to deploy it.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-gateway --open`

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |

## Build

On Linux or Windows, set up your system as
[Building](../../docs/building.md) describes, then:

```bash
cargo build --release -p qdp-gateway
```

The gateway is built as `target/release/qdp-gateway`. Its packages build from
there: see [Packaging](../../packaging/README.md).

## Test

```bash
cargo test -p qdp-gateway
```

## License

`qdp-gateway` is licensed under the [Apache License 2.0](../../LICENSE).
