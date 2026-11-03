# QDP for Mac, iPhone and iPad

These SwiftUI apps stream from a QDP host to Macs with Apple silicon running
macOS 26 or later, and to iPhones and iPads running iOS or iPadOS 26 or
later. On iPhone and iPad the app is called Quick Desktop. Both apps share
one Rust core, [`qdp-apple`](../crates/qdp-apple/README.md), which they call
through C.

To install and use them, see [Installing on a Mac](../docs/setup/macos.md),
[Installing on iPhone and iPad](../docs/setup/iphone-ipad.md) and
[Using the apps](../docs/setup/apps.md#the-apps).

## Setting up

The apps build on a Mac with Apple silicon. You need:

- Xcode 26, from the App Store, opened once to accept its license;
- [XcodeGen](https://github.com/yonaskolb/XcodeGen), which writes the Xcode
  project from [`project.yml`](project.yml):

  ```bash
  brew install xcodegen
  ```

- Rust's Apple targets:

  ```bash
  rustup target add aarch64-apple-darwin aarch64-apple-ios aarch64-apple-ios-sim
  ```

Then, from this folder, build the Rust core and generate the Xcode project:

```bash
./build-core.sh && xcodegen generate
```

[`build-core.sh`](build-core.sh) builds the core for iPhone and iPad, the
iOS simulator and Macs, into `Generated/QDPCore.xcframework`, which Swift
imports as `QDPCore`. Run it again after changing the core's Rust code, or
any crate it uses; Xcode picks it up on its next build.

## The Mac app

Build it, signed ad hoc, into `build/Build/Products/Release/QDP.app`, and
open it:

```bash
xcodebuild -project QDP.xcodeproj -scheme QDP-macOS -configuration Release -destination 'generic/platform=macOS' -derivedDataPath build CODE_SIGN_IDENTITY=- CODE_SIGN_STYLE=Manual DEVELOPMENT_TEAM= build
```

```bash
open build/Build/Products/Release/QDP.app
```

## iPhone and iPad

1. Open the project in Xcode:

   ```bash
   open QDP.xcodeproj
   ```

2. Choose the `QDP-iOS` scheme and your device.
3. Choose your team: in the Project navigator, select the project, then the
   `QDP-iOS` target, then Signing & Capabilities.
4. Run it.

`xcodegen generate` writes the project anew, so choose your team again after
running it.

## Code

| Folder | What it holds |
|---|---|
| [`Shared`](Shared) | What both apps share: the computers list, pairing, adding a computer, settings, the gateway's sign-in, the session's model, and `Core.swift`, the Swift side of the core's C interface |
| [`iOS`](iOS) | The iPhone and iPad app: the stream screen and its menu, touch and keyboard input, and the on-screen controller |
| [`macOS`](macOS) | The Mac app: a window for each stream |
| `Generated` | What `build-core.sh` and XcodeGen write: the core, its build settings and the apps' `Info.plist` files; Git ignores it |

[`.github/workflows/apple.yml`](../.github/workflows/apple.yml) builds the
core and both apps, the Mac app signed ad hoc and the iPhone and iPad app
unsigned, for each release.

The apps' icons are drawn from QDP's logo by
[`branding/generate-icons.py`](../branding/README.md).
