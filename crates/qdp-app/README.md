# qdp-app

What QDP's client apps share: sessions with reconnection and pairing,
settings, and the on-screen controller.

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-app --open`

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| C compiler | Any | All | Building bundled C code |
| NASM | 2.15+ | x86-64 | Building the codecs |

## License

`qdp-app` is licensed under the [Apache License 2.0](../../LICENSE).
