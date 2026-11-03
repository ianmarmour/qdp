# qdp-apple

The core of the Mac, iPhone and iPad apps. The apps themselves are in
[`apple`](../../apple/README.md).

It's part of [QDP](../../README.md): the
[architecture overview](../../docs/architecture.md) shows how the crates fit
together. It's internal to QDP, with no stable API.

API documentation: `cargo doc -p qdp-apple --open`

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| Xcode | 26+ | macOS | Building for Apple platforms |

## Build

On a Mac with Apple silicon, from the `apple` folder:

```bash
./build-core.sh
```

This builds the core for Macs, iPhones, iPads and the iOS simulator, for the
apps' Xcode project: see [`apple`](../../apple/README.md).

## Test

```bash
cargo test -p qdp-apple
```

## License

`qdp-apple` is licensed under the [Apache License 2.0](../../LICENSE).
