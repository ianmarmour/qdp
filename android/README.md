# QDP for Android

The QDP app for Android phones, tablets and handhelds, running Android 10 or
later on 64-bit ARM. To install and use it, see
[Installing on Android](../docs/setup/android.md) and
[Using the apps](../docs/setup/apps.md#android).

## Requirements

- Linux or macOS
- JDK 17 or later
- The Android SDK and NDK, which Android Studio's SDK Manager installs
- Rust's Android target: `rustup target add aarch64-linux-android`

## Build

From this folder, with `ANDROID_HOME` set to the SDK (`~/Android/Sdk` on
Linux, `~/Library/Android/sdk` on a Mac):

```bash
./gradlew assembleRelease
```

Use `installRelease` instead to install on a device connected with USB
debugging. Release builds are signed as [Releasing](../docs/releasing.md#signing-the-android-app)
describes.

## Test

```bash
./gradlew lintRelease
```

## License

Apache-2.0
