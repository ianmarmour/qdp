# Installing on Linux

QDP for Linux has two parts: the `qdp` command, which shares this computer
and connects to others, and the QDP app, which runs the command for each
stream.

## Requirements

- An x86-64 computer
- For the app, GTK 4.14 and libadwaita 1.5 or later, as on Fedora 40 and
  Ubuntu 24.04
- For video on the GPU, its VA-API driver: Mesa's for AMD, and
  intel-media-driver for Intel

## Install

On Fedora:

```bash
sudo dnf install https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.rpm
sudo dnf install https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gtk-linux-x86_64.rpm
```

On Debian and Ubuntu, install the command, and the app's tarball for your
user:

```bash
curl -LO https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.deb
sudo apt install ./qdp-linux-x86_64.deb
curl -L https://github.com/ianmarmour/qdp/releases/latest/download/qdp-gtk-linux-x86_64.tar.gz | tar -xz -C ~/.local
```

Elsewhere, extract the command onto your `PATH`, and set up input as below:

```bash
mkdir -p ~/.local/bin
curl -L https://github.com/ianmarmour/qdp/releases/latest/download/qdp-linux-x86_64.tar.gz | tar -xz -C ~/.local/bin
```

## Input without the packages

The packages let the host create the virtual input devices it needs. Without
them, let members of the `input` group create them, then log out and in
again:

```bash
printf 'uinput\nuhid\n' | sudo tee /etc/modules-load.d/qdp.conf
sudo modprobe -a uinput uhid
printf '%s\n' 'KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"' 'KERNEL=="uhid", GROUP="input", MODE="0660", OPTIONS+="static_node=uhid"' | sudo tee /etc/udev/rules.d/60-qdp-uinput.rules
sudo udevadm control --reload-rules && sudo udevadm trigger
sudo usermod -aG input "$USER"
```

## Upgrade and remove

Install a newer package the same way to upgrade. To remove QDP, run
`sudo dnf remove qdp-gtk qdp` or `sudo apt remove qdp`.

## Next steps

- [Share this computer](hosting.md)
- [Use the apps](apps.md)
