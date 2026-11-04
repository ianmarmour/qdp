# Sharing a computer

Any Linux, Windows or Mac computer can share its desktop. This page covers
what to set up on each system; `qdp help` lists every option.

## Running as a service

The host runs as a service, which keeps it running in the background:

- **Linux:** the RPM and Debian package install the service; turn it on with
  `sudo systemctl enable --now qdp`. Without the packages,
  `sudo qdp service install` sets it up. It runs from boot, with the
  [login screen](#streaming-the-login-screen) included, and its pairing codes
  show on the logged-in desktop and in `journalctl -u qdp -f`.
- **macOS:** `qdp service install` starts the host whenever you log in.
- **Windows:** the host can't run as a service yet, so run `qdp serve`.

`qdp serve` also runs the host in a terminal, for trying it out. The host
listens on UDP port 4433, so allow that port through any firewall between your
devices and the computer. Linux hosts show up in the apps on their own; add
Windows and Mac hosts by their address.

## Pairing

The first time a device connects, the host shows a four-digit code, which you
type on the device. A code lasts two minutes, and repeated wrong codes pause
pairing for a while.

To list the devices paired with this computer, or forget one:

```bash
qdp clients
qdp clients remove NAME
```

For the Linux service, run these with `sudo` and `--identity /var/lib/qdp`.

`--pairing off` turns pairing off. The host can also accept tokens, passwords,
client keys and your organization's [gateway](../gateway.md): `qdp help`
describes each.

## Platform notes

### Linux

- QDP shares the screen of GNOME, KDE Plasma and wlroots desktops such as
  sway and Hyprland. The first time, the desktop asks which monitor to share.
- The host needs access to the input devices it creates. The RPM and Debian
  package set this up; otherwise see
  [input without the packages](linux.md#input-without-the-packages).
- AMD RDNA 3 and Intel Arc GPUs, and newer, encode in hardware with their
  VA-API driver. Other computers encode in software.

### Windows

- NVIDIA RTX 40, AMD RX 7000 and Intel Arc GPUs, and newer, encode in
  hardware. Other computers encode in software.
- To control programs run as administrator, run the host as administrator
  too.

### macOS

- The first time a device connects, allow **Screen Recording** and
  **Accessibility** for `qdp`, or the terminal running it, under System
  Settings, Privacy & Security.
- Macs encode in software.

## Streaming the login screen

On Linux, the service streams whatever the computer's display shows, from the
login screen to any desktop, so that a device can log in and unlock as if at
the machine. To try it in a terminal, run `sudo qdp serve --source console`.
A display must be connected and on; a dummy plug can stand in for one.

## Security

- While a device is connected, the computer shows a notice saying so.
- If the connection drops, the device reconnects on its own for up to five
  minutes, without asking for credentials again. `--resume` changes how long.
- On Windows, `--privacy on` covers the computer's displays and ignores its
  own keyboard and mouse while a device is connected.
