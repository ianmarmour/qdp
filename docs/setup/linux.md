# Installing QDP on Linux

On Linux, QDP has two parts:

- **The `qdp` command**, which shares this computer's desktop with
  `qdp serve`, and connects to other computers with `qdp connect`.
- **The QDP app**, which lists the computers to connect to and pairs with
  them. It runs `qdp` for each stream, so it needs the command too.

Both are built for x86-64.

## The `qdp` command

### Fedora and openSUSE

The RPM is for Fedora 40 or later and openSUSE Tumbleweed:

```bash
sudo dnf install https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.rpm
```

### Debian and Ubuntu

The Debian package is for Debian 12 or later and Ubuntu 22.04 or later:

```bash
curl -LO https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.deb
sudo apt install ./qdp-linux-x86_64.deb
```

### What the packages set up

Besides the command, the RPM and the Debian package install:

- **Input access.** A udev rule and kernel modules that let the host create
  virtual input devices (`/dev/uinput`, and `/dev/uhid` for passed-through
  devices), for the user at the machine and members of the `input` group.
- **A system service**, off until you turn it on, which streams the console
  from boot, the login screen included. [Sharing a computer](hosting.md#running-as-a-service)
  describes it.

### Other distributions

`qdp-linux-x86_64.tar.gz` holds the command alone. Extract it somewhere on
your `PATH`:

```bash
mkdir -p ~/.local/bin
curl -L https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.tar.gz | tar -xz -C ~/.local/bin
```

Then set up [input access](#input-without-the-packages) yourself.

## The QDP app

The app needs GTK 4.14 and libadwaita 1.5 or later: Fedora 40 and Ubuntu
24.04 or later have them.

On Fedora, install its RPM after the `qdp` RPM. It installs the app with its
menu entry, icons and AppStream metadata, so that it shows in the desktop's
menus and software center:

```bash
sudo dnf install https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gtk-linux-x86_64.rpm
```

Elsewhere, install it for your user by extracting
`qdp-gtk-linux-x86_64.tar.gz` into `~/.local`:

```bash
curl -L https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gtk-linux-x86_64.tar.gz | tar -xz -C ~/.local
```

The app runs the `qdp` installed beside it, or else the one on your `PATH`.

## Hardware encoding

A Linux host encodes on the GPU through VA-API when the GPU can encode AV1:
AMD RDNA 3 or newer, and Intel Arc or newer. Otherwise it encodes in
software, which uses far more of the processor.

Hardware encoding needs the GPU's VA-API driver, which desktop distributions
install with Mesa (`mesa-va-drivers`) or Intel's media driver
(`intel-media-driver`). To check, run `vainfo`: it lists
`VAProfileAV1Profile0` with `VAEntrypointEncSlice` when hardware encoding is
available. [Sharing a computer](hosting.md#encoding) covers the details.

## Input without the packages

The host performs the client's keyboard, mouse, touch and gamepad input
through uinput, and recreates passed-through devices through uhid. Both need
write access to their device nodes, which the packages grant. Without the
packages, grant it to members of the `input` group:

```bash
printf 'uinput\nuhid\n' | sudo tee /etc/modules-load.d/qdp.conf
sudo modprobe -a uinput uhid
printf '%s\n' 'KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"' 'KERNEL=="uhid", GROUP="input", MODE="0660", OPTIONS+="static_node=uhid"' | sudo tee /etc/udev/rules.d/60-qdp-uinput.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo usermod -aG input "$USER"
```

Log out and in again for the group change to apply. Without access, the host
logs a warning and ignores input.

## Upgrading and removing

To upgrade, install the newer package the same way. To remove QDP:

```bash
sudo dnf remove qdp-gtk qdp
```

```bash
sudo apt remove qdp
```

## Next steps

- [Share this computer](hosting.md) with `qdp serve`.
- [Connect to a computer](apps.md) from the app or with
  `qdp connect`.
