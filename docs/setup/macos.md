# Installing QDP on a Mac

QDP for the Mac runs on Macs with Apple silicon. It has two parts:

- **The QDP app**, for macOS 26 or later, which lists the computers to
  connect to, pairs with them, and streams from each in a window of its own.
- **The `qdp` command**, which shares this Mac's desktop with `qdp serve`,
  and can also connect to other computers with `qdp connect`.

Neither is notarized by Apple yet, so macOS asks before opening them the first
time, as below.

## The app

1. Download
   [`qdp-macos-app.zip`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-macos-app.zip)
   and open it, which extracts `QDP.app`.
2. Move `QDP.app` to your Applications folder.
3. Open it. macOS says it can't check the app for malicious software.
4. Open System Settings, Privacy & Security, and choose **Open Anyway** for
   QDP. From then on it opens like any other app.

## The `qdp` command

Download
[`qdp-macos-arm64.tar.gz`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-macos-arm64.tar.gz),
extract it, and let macOS run it:

```bash
tar -xzf qdp-macos-arm64.tar.gz
xattr -d com.apple.quarantine qdp
```

Move `qdp` to a folder on your `PATH`, such as `/usr/local/bin`.

The first time a device connects, macOS asks to allow **Screen Recording**
and **Accessibility** for the terminal or app running `qdp`, under System
Settings, Privacy & Security. [Sharing a computer](hosting.md#macos) explains why.

## Hardware

The app decodes video on the GPU on Macs with an AV1 decoder, Apple M3 and
newer, and in software on older Macs. A Mac host encodes in software.

## Building it yourself

On an Intel Mac, or to try changes, build QDP from source:
[`apple`](../../apple/README.md) covers the app, and
[Building](../building.md#macos) the command.

## Next steps

- [Connect to a computer](apps.md#mac) from the app.
- [Share this Mac](hosting.md) with `qdp serve`.
