# qdp-gtk

QDP's app for Linux desktops, built with GTK 4 and libadwaita, whose look
GNOME uses and other desktops show well. It lists the computers on the local
network, which it follows with `qdp discover --watch`, and those it paired
with; it pairs with the code a host shows, and runs `qdp connect --events`
for each stream, following its progress.

[Installing on Linux](../../docs/setup/linux.md#the-qdp-app) and
[Using the apps](../../docs/setup/apps.md#linux) cover using it.

## Requirements

- GTK 4.14 and libadwaita 1.5 or later: Fedora 40 and Ubuntu 24.04 or later.
- The `qdp` command, installed beside the app or on the `PATH`.

## Building

With the development files [Building](../../docs/building.md#linux) lists:

```bash
cargo build --release -p qdp-gtk
./target/release/qdp-gtk
```

The app runs the `qdp` built beside it. To install your build for yourself,
with its menu entry and icons:

```bash
packaging/linux/stage-gtk.sh target/release/qdp-gtk ~/.local && cp target/release/qdp ~/.local/bin/
```

The RPM and the release's tarball build from the same layout: see
[`packaging`](../../packaging/README.md).

## License

Apache-2.0, like the rest of [QDP](../../README.md).
