# QDP for Android

The QDP app for Android phones, tablets and handhelds, running Android 10 or
later on 64-bit ARM. To install and use it, see
[Installing on Android](../docs/setup/android.md) and
[Using the apps](../docs/setup/apps.md#android).

## Dependencies

| Dependency | Version |
|---|---|
| Rust | 1.97.0 |
| JDK | 17 |
| Gradle | 8.14.3 |
| Android Gradle Plugin | 8.11.1 |
| Kotlin | 2.2.21 |
| Jetpack Compose BOM | 2026.06.01 |
| Android SDK platform | 35 |
| Android NDK | Latest |
| Minimum Android | 10 (API 29) |

## Build

On Linux or macOS, install the SDK and NDK with Android Studio's SDK Manager,
and Rust's Android target:

```bash
rustup target add aarch64-linux-android
```

Then, from this folder, with `ANDROID_HOME` set to the SDK (`~/Android/Sdk` on
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
