# Troubleshooting

## The computer doesn't show up

- Only Linux computers show up on their own. Add Windows and Mac computers by
  their address.
- The host listens on UDP port 4433: allow it through any firewall in
  between.
- An address on your home network, such as `192.168.1.20`, only works on that
  network. To connect from elsewhere, use an address that reaches the computer
  from there, such as one on a VPN, or your organization's
  [gateway](gateway.md).

## Pairing fails

- A code lasts two minutes: connect again for a new one.
- After several wrong codes, the host pauses pairing for a few minutes.
- If the app says a computer has a **different key**, it isn't the computer
  you paired with, or its identity was reset. If you expect that, forget it
  and pair again.

## The stream shows a test pattern or a still image

The host couldn't capture the screen:

- **Linux:** start the host inside a GNOME, KDE Plasma or wlroots desktop
  session, or [stream the login screen](setup/hosting.md#streaming-the-login-screen).
- **macOS:** allow Screen Recording for `qdp` under System Settings, Privacy &
  Security.
- **Windows:** the sign-in screen and administrator prompts show only to a
  host running as SYSTEM.

## The computer ignores input

- **Linux:** the host needs access to the input devices it creates. The
  packages set it up; otherwise see
  [input without the packages](setup/linux.md#input-without-the-packages).
- **macOS:** allow Accessibility for `qdp` under System Settings, Privacy &
  Security.
- **Windows:** to control programs run as administrator, run the host as
  administrator too.

## Video is slow or choppy

- Check the host's log for which encoder it uses. Without a GPU that encodes
  AV1 in hardware, it encodes in software, which struggles at large sizes; the
  [platform notes](setup/hosting.md#platform-notes) list the GPUs that work.
- Check the client's log for which decoder it uses. Clients with a GPU older
  than NVIDIA RTX 30, AMD RX 6000, Intel 11th-generation Core or Apple M3, and
  iPhones before the 15 Pro, decode in software. On Linux, the GPU also needs
  its [VA-API driver](setup/linux.md#requirements).
- Ask for a smaller resolution in the app's settings.

## An app won't install or open

- **Mac:** Quick Desktop and the command aren't notarized yet. Open the app
  once from System Settings, Privacy & Security, and run
  `xattr -d com.apple.quarantine qdp` before running the command.
- **Android:** if a new APK won't install over the old one, uninstall the app
  first.

## Logs

Set `QDP_LOG=debug` for more detail when you run `qdp`:

```bash
QDP_LOG=debug qdp serve
```

The Linux service logs to `journalctl -u qdp`, and the Mac service to
`~/Library/Logs/qdp.log`. When you
[report a problem](https://github.com/ianmarmour/qdp/issues), include the
logs of both sides.
