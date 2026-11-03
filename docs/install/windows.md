# Installing QDP on Windows

QDP for Windows runs on Windows 10 or later, on x86-64. It has two parts:

- **The QDP app**, which lists the computers to connect to, pairs with them,
  and streams from them.
- **The `qdp` command**, which shares this PC's desktop with `qdp serve`.

## The installer

Download and run
[`qdp-windows-x64.msi`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-windows-x64.msi).
It needs nothing installed first: the app carries its own .NET and Windows App
SDK, and QDP's programs their own C runtime.

It installs:

- the app in `C:\Program Files\QDP`, with a Start menu shortcut;
- the `qdp` command, on the `PATH`, for sharing this PC.

Installing a later build upgrades it in place. To remove QDP, use Settings,
Apps.

## Without installing

- [`qdp-windows-app-x64.zip`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-windows-app-x64.zip)
  holds the app, which runs from its folder.
- [`qdp-windows-x86_64.zip`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-windows-x86_64.zip)
  holds the `qdp` command alone.

## Hardware

The app decodes video on the GPU where the GPU has an AV1 decoder: NVIDIA
RTX 30 and later, AMD RX 6000 and later, Intel Arc and Xe, and Snapdragon X.
Elsewhere it decodes in software.

A Windows host encodes on the GPU where the GPU encodes AV1:

- NVIDIA GeForce RTX 40 or newer, with NVENC, on driver version 522 or later;
- AMD Radeon RX 7000 or newer, and Intel Arc or newer, with the AV1 encoder
  their drivers provide through Media Foundation.

Elsewhere it encodes in software. [Hosting](../hosting.md#encoding) covers the
details.

## Next steps

- [Connect to a computer](../connecting.md#windows) from the app.
- [Share this PC](../hosting.md) with `qdp serve`.
