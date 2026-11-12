# Packaging

Scripts that build QDP's packages from release builds. CI runs them for every
release; the `1` in a build is the package's release or build number.

| Package | Build |
|---|---|
| `qdp` RPM | `packaging/linux/build-rpm.sh target/release/qdp 1 qdp.rpm` |
| `qdp` Debian package | `packaging/linux/build-deb.sh qdp target/release/qdp 1 qdp.deb` |
| Linux app RPM | `packaging/linux/build-gtk-rpm.sh target/release/qdp-gtk 1 qdp-gtk.rpm` |
| Linux app AppImage | `packaging/linux/build-appimage.sh target/release/qdp-gtk target/release/qdp AppDir runtime-x86_64 qdp-gtk.AppImage` |
| Gateway RPM | `packaging/linux/build-gateway-rpm.sh target/release/qdp-gateway 1 qdp-gateway.rpm` |
| Gateway Debian package | `packaging/linux/build-deb.sh qdp-gateway target/release/qdp-gateway 1 qdp-gateway.deb` |
| Windows installer | `.\packaging\windows\build-msi.ps1 app qdp-windows-x64.msi 1` |
| Gateway installer for Windows | `.\packaging\windows\build-msi.ps1 gateway qdp-gateway-windows-x64.msi 1` |

## Requirements

- **RPMs:** `rpmbuild`, from `rpm-build` on Fedora or `rpm` on Debian and
  Ubuntu.
- **Debian packages:** `dpkg-deb`, which Debian and Ubuntu have.
- **AppImage:** the [AppImage runtime](https://github.com/AppImage/type2-runtime/releases)
  for x86-64, and on Ubuntu 24.04 `patchelf`, `squashfs-tools`,
  `librsvg2-common` and `adwaita-icon-theme`.
- **Windows installers:** the WiX Toolset 5 and its Firewall and Util
  extensions:

  ```powershell
  dotnet tool install --global wix --version 5.0.2
  wix extension add --global WixToolset.Firewall.wixext/5.0.2 WixToolset.Util.wixext/5.0.2
  ```

  The app's installer also needs the app built first:
  see [`windows`](../windows/README.md).
