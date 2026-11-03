# Installing QDP on Android

The app runs on phones, tablets and gaming handhelds with Android 10 or later
on 64-bit ARM.

It isn't in an app store yet. To install it:

1. On the device, download
   [`qdp-android-arm64.apk`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-android-arm64.apk).
2. Open it. Android asks you to allow the browser or file manager you opened
   it from to install apps; allow it, then install.

From a computer with the Android SDK's platform tools and the device
connected with USB debugging, `adb install qdp-android-arm64.apk` does the
same.

To upgrade, install a newer APK over the old one. If Android refuses because
the new build is signed with a different key, uninstall the app first.

## Hardware

The app decodes video with the device's hardware AV1 decoder, straight into
the screen, and in software on devices without one.

## Next steps

[Connect to a computer](../connecting.md#android) from the app.
