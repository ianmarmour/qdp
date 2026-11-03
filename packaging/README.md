# Packaging

The scripts here build QDP's packages from programs already built with
`cargo build --release`. CI runs them for each release:
[Releasing](../docs/releasing.md) lists what it publishes.

| Package | For | Script |
|---|---|---|
| `qdp` RPM | Fedora 40 or later, openSUSE Tumbleweed | [`linux/build-rpm.sh`](linux/build-rpm.sh) |
| `qdp` Debian package | Debian 12 or later, Ubuntu 22.04 or later | [`linux/build-deb.sh`](linux/build-deb.sh) |
| `qdp-gtk` RPM | Fedora 40 or later | [`linux/build-gtk-rpm.sh`](linux/build-gtk-rpm.sh) |
| `qdp-gateway` RPM | Fedora 40 or later, RHEL 9 or later | [`linux/build-gateway-rpm.sh`](linux/build-gateway-rpm.sh) |
| `qdp-gateway` Debian package | Debian 12 or later, Ubuntu 22.04 or later | [`linux/build-deb.sh`](linux/build-deb.sh) |
| QDP installer | Windows 10 or later | [`windows/build-msi.ps1`](windows/build-msi.ps1) |
| QDP Gateway installer | Windows Server 2019 or later | [`windows/build-msi.ps1`](windows/build-msi.ps1) |

Each package's version is the workspace's, `version` in the root
`Cargo.toml`. The Linux scripts also take a release number, such as a CI
build's, and the Windows script a build number, which each upgrade raises.

## Linux packages

The RPMs need `rpmbuild`: the `rpm-build` package on Fedora, and `rpm` on
Debian and Ubuntu. The Debian packages need `dpkg-deb`, which Debian and
Ubuntu have.

```bash
packaging/linux/build-rpm.sh target/release/qdp 1 qdp.rpm
packaging/linux/build-deb.sh qdp target/release/qdp 1 qdp.deb
packaging/linux/build-gtk-rpm.sh target/release/qdp-gtk 1 qdp-gtk.rpm
packaging/linux/build-gateway-rpm.sh target/release/qdp-gateway 1 qdp-gateway.rpm
packaging/linux/build-deb.sh qdp-gateway target/release/qdp-gateway 1 qdp-gateway.deb
```

[`linux/stage-gtk.sh`](linux/stage-gtk.sh) lays out the Linux app as it
installs, under a prefix: `bin/qdp-gtk`, and `share/` with its desktop entry,
icons, AppStream metadata and license. The RPM and the release's tarball
build from it, and staging into `~/.local` installs the app for one user:

```bash
packaging/linux/stage-gtk.sh target/release/qdp-gtk ~/.local
```

What the Linux packages hold:

| File | What it is |
|---|---|
| [`linux/qdp.spec`](linux/qdp.spec), [`linux/deb/qdp`](linux/deb/qdp) | The `qdp` package |
| [`linux/60-qdp.rules`](linux/60-qdp.rules) | The udev rule giving the user at the machine and the `input` group access to `/dev/uinput` and `/dev/uhid` |
| [`linux/qdp-modules.conf`](linux/qdp-modules.conf) | Loads the `uinput` and `uhid` kernel modules at boot |
| [`linux/qdp.service`](linux/qdp.service) | The host's system service, which streams the console, off until enabled |
| [`linux/qdp-gtk.spec`](linux/qdp-gtk.spec) | The Linux app's package |
| [`linux/qdp-gateway.spec`](linux/qdp-gateway.spec), [`linux/deb/qdp-gateway`](linux/deb/qdp-gateway) | The gateway's package |
| [`linux/qdp-gateway.service`](linux/qdp-gateway.service), [`linux/qdp-gateway.sysusers`](linux/qdp-gateway.sysusers) | The gateway's hardened systemd unit, and its user |
| [`gateway.toml`](gateway.toml) | The gateway's example configuration, which the installers put in place |

## Windows installers

The installers need the [WiX Toolset](https://wixtoolset.org) 5, a .NET tool,
and the gateway's installer its firewall and utility extensions:

```powershell
dotnet tool install --global wix --version 5.0.2
```

```powershell
wix extension add --global WixToolset.Firewall.wixext/5.0.2 WixToolset.Util.wixext/5.0.2
```

Build the app's installer, with `qdp.exe`, after
[`windows\build.ps1`](../windows/README.md), and the gateway's after
`cargo build --release -p qdp-gateway`. The last number is the installer's
build:

```powershell
.\packaging\windows\build-msi.ps1 app qdp-windows-x64.msi 1
```

```powershell
.\packaging\windows\build-msi.ps1 gateway qdp-gateway-windows-x64.msi 1
```

[`windows/QDP.wxs`](windows/QDP.wxs) and
[`windows/QDPGateway.wxs`](windows/QDPGateway.wxs) describe the installers.

### Testing the installers

[`windows/try-app-msi.ps1`](windows/try-app-msi.ps1) installs the app's
installer as a user would, opens the installed app, upgrades it to a later
build, and removes it. [`windows/try-gateway-msi.ps1`](windows/try-gateway-msi.ps1)
does the same for the gateway as an administrator would, configuring and
starting its service. Both fail if any step doesn't work, and leave msiexec's
logs in the current folder.
