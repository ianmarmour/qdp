# Continuous integration and releases

QDP's builds and releases run on GitHub Actions. This page is for
maintainers.

## When CI runs

[`.github/workflows/ci.yml`](../.github/workflows/ci.yml) checks QDP on Linux,
Windows and macOS (formatting, clippy and the tests), and builds the `qdp`
command for each system, the gateway, the apps and the installers. To save
runner minutes, it runs only:

- **when started by hand** on a branch, from the repository's Actions tab;
- **when a version tag is pushed**, such as `v0.2.0`.

Neither pull requests nor merges into `main` start it.

A run started by hand checks only what the branch changes, since Windows and
macOS runners cost two and ten times Linux's minutes. Linux always runs; the
Windows, Windows app, Windows gateway, macOS, Apple and Android jobs run only
when the branch touches their code or the build. These runs skip the release
builds, the packages and the macOS tests, which a tag's run keeps.

The Apple apps, the Windows app and the Windows gateway each have a workflow
of their own ([`apple.yml`](../.github/workflows/apple.yml),
[`windows-app.yml`](../.github/workflows/windows-app.yml),
[`windows-gateway.yml`](../.github/workflows/windows-gateway.yml)), which
`ci.yml` calls and which can also be started by hand on any branch. The
Windows workflows install, upgrade and remove their installers on the runner,
as a user or administrator would.

## Publishing a release

Push a version tag:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

Once every job of the tag's run passes, CI publishes the builds together as a
GitHub release. The newest release's files are always at
`https://github.com/ianmarmour/qdp/releases/latest/download/FILE`:

| File | What it is |
|---|---|
| `qdp-linux-x86_64.rpm` | The `qdp` command for Fedora 40 or later and openSUSE Tumbleweed, with its udev rule, kernel modules and system service |
| `qdp-linux-x86_64.deb` | The same for Debian 12 or later and Ubuntu 22.04 or later |
| `qdp-linux-x86_64.tar.gz` | The `qdp` command alone |
| `qdp-gtk-linux-x86_64.rpm` | The Linux app, with its desktop entry, icons and AppStream metadata |
| `qdp-gtk-linux-x86_64.tar.gz` | The Linux app, to extract into `~/.local` |
| `qdp-gateway-linux-x86_64.rpm`, `.deb`, `.tar.gz` | The gateway for Linux servers |
| `qdp-windows-x64.msi` | The Windows app and the `qdp` command |
| `qdp-windows-app-x64.zip` | The Windows app, to run without installing |
| `qdp-windows-x86_64.zip` | The `qdp` command for Windows alone |
| `qdp-gateway-windows-x64.msi`, `qdp-gateway-windows-x86_64.zip` | The gateway for Windows servers |
| `qdp-macos-app.zip` | The Mac app, signed ad hoc |
| `qdp-macos-arm64.tar.gz` | The `qdp` command for Macs with Apple silicon |
| `qdp-ios-unsigned.ipa` | The iPhone and iPad app, unsigned |
| `qdp-android-arm64.apk` | The Android app |

The release's title is the tag's version, and its notes list each file with
the systems it needs. The packages take their version from the workspace
(`version` in the root `Cargo.toml`), so raise it before tagging a new
version.

The Mac app and the `qdp` command for macOS are not notarized yet, so macOS
asks before opening them: the [installation guide](install/macos.md) says how
to open them anyway.

## Signing the Android app

So that each Android build installs over the last, sign them all with one key.
Create it once and keep it safe:

```bash
keytool -genkeypair -keystore qdp-release.keystore -alias qdp -keyalg RSA -keysize 4096 -validity 10000
```

Then give it to the repository as these secrets, under Settings, Secrets and
variables, Actions:

| Secret | Value |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | The keystore, base64-encoded, such as `base64 -i qdp-release.keystore \| pbcopy` copies on a Mac |
| `ANDROID_KEYSTORE_PASSWORD` | The keystore's password |
| `ANDROID_KEY_ALIAS` | The key's alias: `qdp` above |
| `ANDROID_KEY_PASSWORD` | The key's password |

Without the secrets, builds are signed with a debug key that CI keeps in its
cache between runs. If the cache loses it, the next build has a new key, and
installing it over an older build needs the app uninstalled first.

Local builds take the same key from the environment variables
`QDP_KEYSTORE`, `QDP_KEYSTORE_PASSWORD`, `QDP_KEY_ALIAS` and
`QDP_KEY_PASSWORD`.
