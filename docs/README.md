# QDP documentation

## Initial Setup

[Initial Setup](setup/README.md) takes you from installing QDP to your first
connection in three steps. In depth:

| Guide | What it covers |
|---|---|
| [Installing on Linux](setup/linux.md) | The `qdp` command and the QDP app |
| [Installing on Windows](setup/windows.md) | The QDP app and the `qdp` command |
| [Installing on a Mac](setup/macos.md) | The QDP app and the `qdp` command |
| [Installing on Android](setup/android.md) | The QDP app |
| [Installing on iPhone and iPad](setup/iphone-ipad.md) | The Quick Desktop app |
| [Sharing a computer](setup/hosting.md) | `qdp serve`: pairing, platform notes, running as a service, the login screen, security |
| [Using the apps](setup/apps.md) | Each app, connecting through a gateway, and `qdp connect` |

## Troubleshooting

[Troubleshooting](troubleshooting.md) has fixes for common problems with
finding computers, pairing, video and input, and how to collect logs.

## Gateway Setup

[Gateway Setup](gateway.md) shows how to install QDP Gateway, expose it to the
internet, and grant access to endpoints.

## Specification

| Document | What it covers |
|---|---|
| [Specification](spec/README.md) | The Quick Desktop Protocol, draft 00, in 13 chapters |
| [Design rationale](design-rationale.md) | Goals, the reasoning and measurements behind each decision, alternatives rejected, open questions |

## Development

| Guide | What it covers |
|---|---|
| [Contributing](../CONTRIBUTING.md) | Reporting problems, making changes, code style |
| [Building from source](building.md) | Setting up Linux, macOS and Windows to build and test |
| [Architecture](architecture.md) | How the crates and apps fit together |
| [Releasing](releasing.md) | Continuous integration, releases, signing |
| [Packaging](../packaging/README.md) | The RPMs, Debian packages and Windows installers |

Each crate and app has a README of its own:
[the architecture overview](architecture.md#crates) lists them.
