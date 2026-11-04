# Installing on a Mac

QDP for the Mac has two parts: the Quick Desktop app, and the `qdp` command,
which shares this Mac.

## Requirements

- A Mac with Apple silicon
- macOS 26 or later, for the app

## Install

Download [`qdp-macos-app.zip`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-macos-app.zip), open it, and move
Quick Desktop to your Applications folder.

To share this Mac, download
[`qdp-macos-arm64.tar.gz`](https://github.com/ianmarmour/qdp/releases/latest/download/qdp-macos-arm64.tar.gz), then extract the
command and move it onto your `PATH`:

```bash
tar -xzf qdp-macos-arm64.tar.gz
sudo mv qdp /usr/local/bin/
```

## Upgrade and remove

Install a newer download the same way to upgrade. To remove QDP, move the app
to the Trash and delete `/usr/local/bin/qdp`.

## Next steps

- [Share this computer](hosting.md)
- [Use the apps](apps.md)
