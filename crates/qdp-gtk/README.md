# QDP for Linux

The QDP app for Linux desktops, built with GTK 4 and libadwaita. To install
and use it, see [Installing on Linux](../../docs/setup/linux.md) and
[Using the apps](../../docs/setup/apps.md#linux).

## Dependencies

| Dependency | Version |
|---|---|
| Rust | 1.97.0 |
| GTK | 4.14 |
| libadwaita | 1.5 |
| gtk4-rs | 0.11.5 |
| libadwaita-rs | 0.9.2 |

## Build

On Linux, install the development files [Building](../../docs/building.md#linux)
lists, then:

```bash
cargo build --release -p qdp-gtk
```

The app runs the `qdp` command for each stream, so install or build that too.

## Test

```bash
cargo test -p qdp-gtk
```

## License

Apache-2.0
