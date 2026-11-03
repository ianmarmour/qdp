# Troubleshooting

- [Logs](#logs)
- [The host doesn't show up](#the-host-doesnt-show-up)
- [Pairing fails](#pairing-fails)
- [The stream shows a test pattern or a still image](#the-stream-shows-a-test-pattern-or-a-still-image)
- [The host ignores input](#the-host-ignores-input)
- [Video is slow or uses too much processor](#video-is-slow-or-uses-too-much-processor)
- [An app won't install or open](#an-app-wont-install-or-open)

## Logs

The `qdp` command and the gateway log to their terminal. Set `QDP_LOG` to
`error`, `warn`, `info`, `debug` or `trace` for more or less detail:

```bash
QDP_LOG=debug qdp serve
```

`QDP_LOG` also takes levels for single parts after the default one,
separated by commas. For example, to trace each pointer button, every move
while a button is held, the positions the host ignores, and the host's warps
of the client's pointer:

```bash
QDP_LOG=info,qdp::window=debug qdp connect 192.0.2.7
QDP_LOG=info,qdp_runtime::host=debug,qdp_runtime::host_cursor=debug qdp serve
```

`QDP_LOG=info,qdp_codec=debug` on the host logs how long each frame takes to
reach the encoder's input, encode and read back.

Services log elsewhere:

| Service | Logs |
|---|---|
| The Linux host service | `journalctl -u qdp -f` |
| The macOS host agent | `~/Library/Logs/qdp.log` |
| The Linux gateway | `journalctl -u qdp-gateway -f` |
| The Windows gateway | `C:\ProgramData\qdp-gateway` |

Both sides log statistics each second: frames sent and drawn, latency and
decode times. Most apps can show them over the stream: on Windows with
Ctrl+Alt+Shift+S, and from the stream's menu on Android, iPhone and iPad.

## The host doesn't show up

- **Is it a Linux host?** Only Linux hosts advertise themselves on the local
  network, through Avahi. Add Windows and Mac hosts by their address.
- **Is it advertising?** On a Linux computer, `qdp discover` lists the hosts
  advertising on the local network. A host started with `--advertise off`
  doesn't advertise.
- **Is the port open?** Hosts listen on UDP port 4433 by default; allow it
  through any firewall between the client and the host.
- **Is the address reachable from here?** A computer saved by its address on
  the local network, such as `192.168.1.20`, can't be reached from another
  network, such as mobile data. To reach it from anywhere, add it by an
  address that works from there, such as one on a VPN. In an organization, a
  [gateway](gateway.md) can reach hosts inside its network.

## Pairing fails

- **The code expired.** A code is good for 120 seconds. Connect again for a
  new one.
- **Pairing is paused.** After three wrong codes in a row, the host refuses
  pairing for 1 minute, then 3, then 5, and twice as long after each further
  wrong code.
- **Pairing is off.** A host started with `--pairing off` accepts only devices
  it already paired with, or other credentials.
- **"Different key — pair again".** A computer with the name of one you
  paired with presents a different key, as after its identity directory was
  replaced. If you expect that, forget the computer and pair with it again.
- **No code shows.** The host prints the code in its log. A service streaming
  the login screen shows it in the logged-in desktop and in its log, and only
  in its log when no one is logged in.

## The stream shows a test pattern or a still image

The host streams a test pattern when it can't capture the screen:

- **Linux:** the host needs a desktop with the ScreenCast portal (GNOME, KDE
  Plasma and wlroots compositors provide it), and the monitor chosen in the
  portal's dialog. To stream outside a desktop session, as at the login
  screen, use [`--source console`](hosting.md#streaming-the-login-screen).
- **macOS:** allow Screen Recording for the terminal or app running `qdp`,
  under System Settings, Privacy & Security.
- **Windows:** unless the host runs as SYSTEM, it holds the last image while
  the sign-in screen or an elevation prompt shows.

## The host ignores input

- **Linux:** the host needs write access to `/dev/uinput`, and logs a warning
  without it. The packages grant it; otherwise follow
  [the manual setup](install/linux.md#input-without-the-packages), then log
  out and in again.
- **macOS:** allow Accessibility for the terminal or app running `qdp`, under
  System Settings, Privacy & Security.
- **Windows:** input can't reach programs run as administrator unless the host
  runs as administrator too, nor the sign-in and lock screens unless it runs
  as SYSTEM in the console's session.
- **Gamepads and touch** reach only the hosts that accept them: see
  [Hosting](hosting.md#input).

## Video is slow or uses too much processor

- **Check the encoder.** The host logs which encoder it uses, and why it fell
  back to software if it did. Without a GPU that encodes AV1, it encodes in
  software, which uses far more processor at large sizes. On Linux, `vainfo`
  lists `VAProfileAV1Profile0` with `VAEntrypointEncSlice` when hardware
  encoding is available; install Mesa's VA-API driver (`mesa-va-drivers`) or
  Intel's (`intel-media-driver`) if it doesn't. `--encoder hardware` makes
  the host fail rather than fall back, with the reason in its log.
- **Check that frames stay on the GPU.** On Linux, the host logs "encoding
  frames straight from the GPU" when the screen goes from the compositor to
  the encoder without copies. It needs `VAEntrypointVideoProc` under
  `VAProfileNone` in `vainfo`.
- **Stream fewer pixels.** On a large or full-screen window, ask for a smaller
  resolution: the apps' settings choose one, and `qdp connect` takes
  `--resolution 1920x1080`.
- **Check the decoder.** Clients decode on the GPU where it has an AV1
  decoder, and in software otherwise; the installation guides list which
  devices have one.

## An app won't install or open

- **Mac:** the app and the `qdp` command aren't notarized yet. Open the app
  once from System Settings, Privacy & Security, and run
  `xattr -d com.apple.quarantine qdp` before running `qdp`. See
  [Installing on a Mac](install/macos.md).
- **Android:** if a newer APK won't install over the old one, it's signed with
  a different key: uninstall the app first.
