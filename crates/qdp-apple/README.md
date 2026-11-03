# qdp-apple

The core of the Mac, iPhone and iPad apps. The apps themselves are in
[`apple`](../../apple/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-apple --open`

## Dependencies

The native tools and libraries it needs beyond `Cargo.toml`. A `+` means that
version or later.

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| Xcode | 26+ | macOS | Building for Apple platforms |

## License

Apache-2.0
