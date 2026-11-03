# qdp-apple

The core of the Mac, iPhone and iPad apps. The apps themselves are in
[`apple`](../../apple/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-apple --open`

## Dependencies

The tools and native libraries it needs beyond its Cargo dependencies, at
the oldest versions that work:

| Dependency | Version |
|---|---|
| Rust | 1.87 |
| Xcode | 26 |

## License

Apache-2.0
