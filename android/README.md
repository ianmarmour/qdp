# QDP for Android

The QDP app for Android phones, tablets and handhelds, running Android 10 or
later on 64-bit ARM. To install and use it, see
[Installing on Android](../docs/setup/android.md) and
[Using the apps](../docs/setup/apps.md#android).

## Dependencies

| Dependency | Version | Platform | Needed for |
|---|---|---|---|
| Rust | 1.97.0 | Linux, macOS | Building the app's Rust core |
| Android NDK | r25+ | Linux, macOS | Building the app's Rust core |
| JDK | 17 | Linux, macOS | Running Gradle |
| Gradle | 8.14.3 | Linux, macOS | Building the app |
| Android Gradle Plugin | 8.11.1 | Linux, macOS | Building the app |
| Android SDK platform | 35 | Linux, macOS | Building the app |
| Kotlin | 2.2.21 | Linux, macOS | The app's code |
| Jetpack Compose BOM | 2026.06.01 | Linux, macOS | The app's interface |

The app runs on Android 10 (API 29) or later.

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

QDP for Android is licensed under the [Apache License 2.0](../LICENSE).
