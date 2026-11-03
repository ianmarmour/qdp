# QDP for Linux

The QDP app for Linux desktops, built with GTK 4 and libadwaita. To install
and use it, see [Installing on Linux](../../docs/setup/linux.md) and
[Using the apps](../../docs/setup/apps.md#linux).

## Requirements

- GTK 4.14 and libadwaita 1.5 or later
- The development files [Building](../../docs/building.md#linux) lists
- The `qdp` command, which the app runs for each stream

## Build

```bash
cargo build --release -p qdp-gtk
```

## Test

```bash
cargo test -p qdp-gtk
```

## License

Apache-2.0
