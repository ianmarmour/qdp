# Initial Setup

QDP has two sides: a **host** on the computer you want to reach, and a
**client** on each device you connect from. Setting them up takes three
steps.

## 1. Install QDP

Install the host on the computer you want to reach, and a client on each
device you'll connect from.

| Platform | Host | Client | Guide |
|---|:---:|:---:|---|
| Linux | ✅ | ✅ | [Installing on Linux](linux.md) |
| Windows | ✅ | ✅ | [Installing on Windows](windows.md) |
| macOS (Apple silicon) | ✅ | ✅ | [Installing on a Mac](macos.md) |
| Android | | ✅ | [Installing on Android](android.md) |
| iPhone and iPad | | ✅ | [Installing on iPhone and iPad](iphone-ipad.md) |

## 2. Share a computer

On the computer you want to reach, turn on the host's service, which starts it
now and keeps it running in the background:

- **Linux:** `sudo systemctl enable --now qdp`
- **macOS:** `qdp service install`
- **Windows:** the host can't run as a service yet, so run `qdp serve`.

The host listens on UDP port 4433, so allow that port through any firewall
between your devices and the computer. Each system needs a little preparation,
such as allowing Screen Recording on a Mac: see
[Sharing a computer](hosting.md#platform-notes).

## 3. Pair and connect

1. On your device, open the QDP app and choose the computer. Linux computers
   show up on their own; add Windows and Mac computers by their address.
2. Type the four-digit code the computer shows.

From then on, the two remember each other, and connecting takes one tap. From
a terminal, `qdp connect ADDRESS --pair` does the same.

## Going further

- [Sharing a computer](hosting.md): everything about the host, including
  pairing and other ways to sign in, video and encoding, sound, input,
  security, streaming the login screen, running as a service, and every
  option.
- [Using the apps](apps.md): each app's controls, connecting through an
  organization's gateway, and the `qdp connect` command line.
- [Troubleshooting](../troubleshooting.md): fixes for common problems, and
  how to collect logs.
