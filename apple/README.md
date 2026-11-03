# QDP for Mac, iPhone and iPad

The QDP apps for Macs with Apple silicon, and for iPhone and iPad, where the
app is called Quick Desktop. Both need macOS or iOS 26 or later. To install
and use them, see [Installing on a Mac](../docs/setup/macos.md),
[Installing on iPhone and iPad](../docs/setup/iphone-ipad.md) and
[Using the apps](../docs/setup/apps.md).

## Dependencies

What it builds with. A `+` means that version or later; other versions are exact.

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| Rust | 1.97.0 | macOS | Building the apps' Rust core |
| Xcode | 26+ | macOS | Building the apps |
| XcodeGen | Any | macOS | Generating the Xcode project |
| Swift | 5 | macOS | The apps' code |

The apps run on macOS, iOS and iPadOS 26.0 or later.

## Build

On a Mac with Apple silicon, install XcodeGen and Rust's Apple targets:

```bash
brew install xcodegen
rustup target add aarch64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim
```

Then, from this folder, build the apps' Rust core and generate the Xcode
project:

```bash
./build-core.sh && xcodegen generate && open QDP.xcodeproj
```

Then choose the `QDP-macOS` or `QDP-iOS` scheme and run it. For an iPhone or
iPad, choose your team under Signing & Capabilities first; generating the
project again resets it. Run `./build-core.sh` again after changing any Rust
code.

## Test

The core's tests run with the workspace's: `cargo test --workspace`.

## License

Apache-2.0
