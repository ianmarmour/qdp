# QDP documentation

- **New to QDP?** Install it on the computer you want to reach and on the
  device you'll use, then follow [Sharing a computer](hosting.md) and
  [Connecting](connecting.md).
- **Running QDP in an organization?** See [the gateway](gateway.md).
- **Working on QDP?** Start with [Contributing](../CONTRIBUTING.md) and
  [Building from source](building.md).

## Installing

| Platform | Guide |
|---|---|
| Linux | [The `qdp` command and the QDP app](install/linux.md) |
| Windows | [The QDP app and the `qdp` command](install/windows.md) |
| Mac | [The QDP app and the `qdp` command](install/macos.md) |
| Android | [The QDP app](install/android.md) |
| iPhone and iPad | [The Quick Desktop app](install/iphone-ipad.md) |

## Using QDP

| Guide | What it covers |
|---|---|
| [Sharing a computer](hosting.md) | `qdp serve`: pairing, platform setup, video, sound, input, security, streaming the login screen, running as a service |
| [Connecting](connecting.md) | Each app, and `qdp connect` |
| [The gateway](gateway.md) | Reaching hosts inside an organization's network: installing, configuring and using `qdp-gateway` |
| [Troubleshooting](troubleshooting.md) | Logs, and what to check when something doesn't work |

## Developing QDP

| Guide | What it covers |
|---|---|
| [Contributing](../CONTRIBUTING.md) | Reporting problems, making changes, code style |
| [Building from source](building.md) | Setting up Linux, macOS and Windows to build and test |
| [Architecture](architecture.md) | How the crates and apps fit together |
| [Releasing](releasing.md) | Continuous integration, releases, signing |
| [Packaging](../packaging/README.md) | The RPMs, Debian packages and Windows installers |

Each crate and app has a README of its own:
[the architecture overview](architecture.md#crates) lists them.

## The protocol

| Document | What it covers |
|---|---|
| [Specification](spec/README.md) | The Quick Desktop Protocol, draft 00, in 13 chapters |
| [Design rationale](design-rationale.md) | Goals, the reasoning and measurements behind each decision, alternatives rejected, open questions |
