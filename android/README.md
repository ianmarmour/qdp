# QDP for Android

The Android app streams from a QDP host to phones, tablets and gaming
handhelds running Android 10 or later on 64-bit ARM. It's written in Kotlin
with Jetpack Compose and Material 3, around a Rust core,
[`qdp-android`](../crates/qdp-android/README.md), which it calls through JNI.

To install and use it, see [Installing on Android](../docs/install/android.md)
and [Connecting](../docs/connecting.md#android).

## Building

The app builds on Linux or a Mac. It needs:

- JDK 17 or later;
- the Android SDK with its NDK: Android Studio's SDK Manager installs both,
  the NDK under SDK Tools as NDK (Side by side);
- Rust's Android target:

  ```bash
  rustup target add aarch64-linux-android
  ```

Build it from this folder, giving the SDK's location, which is
`~/Android/Sdk` on Linux and `~/Library/Android/sdk` on a Mac:

```bash
ANDROID_HOME=~/Android/Sdk ./gradlew assembleRelease
```

This builds the Rust core first, with [`build-core.sh`](build-core.sh), into
`app/src/main/jniLibs/arm64-v8a`, then the app into
`app/build/outputs/apk/release/app-release.apk`. Replace `assembleRelease`
with `installRelease` to install it on a device connected with USB debugging.

[`ndk-env.sh`](ndk-env.sh) sets up the NDK's compilers for cargo; source it to
build or check the core by hand. It finds the NDK at `$ANDROID_NDK_HOME`, or
as the newest under `$ANDROID_HOME/ndk`.

### Signing

Release builds are signed with the key in the environment variables
`QDP_KEYSTORE`, `QDP_KEYSTORE_PASSWORD`, `QDP_KEY_ALIAS` and
`QDP_KEY_PASSWORD`, so that each build installs over the last.
[Releasing](../docs/releasing.md#signing-the-android-app) describes the key
CI signs with.

## Checking

```bash
./gradlew assembleRelease lintRelease
```

```bash
cargo clippy --target aarch64-linux-android -p qdp-android --all-targets -- -D warnings
```

## Code

The app's code is in `app/src/main/java/io/github/ianmarmour/qdp`:

| File | What it does |
|---|---|
| `MainActivity.kt` | The hosts screen: the computers on the network and those paired with, adding one by its address, and settings |
| `StreamActivity.kt` | Streams full screen: the video in a surface the core decodes into, the host's cursor over it, and the user's input sent back |
| `Native.kt` | The Rust core's JNI functions |
| `Discovery.kt` | Finds the hosts advertising on the local network |
| `HostList.kt`, `PairedHosts.kt` | The hosts screen's list and the hosts paired with |
| `TouchInput.kt`, `GamepadInput.kt`, `VirtualGamepadInput.kt` | Touches as a trackpad or touchscreen, controllers, and the on-screen controller |
| `Gateway.kt` | Signing in to an organization's gateway |
| `ui/` | The Compose screens: `home`, `pairing`, `settings`, `stream`, `gateway`, and shared `components` and `theme` |

The app's icon is drawn from QDP's logo by
[`branding/generate-icons.py`](../branding/README.md).
