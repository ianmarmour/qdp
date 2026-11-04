# Releasing

QDP builds and releases with GitHub Actions. This page is for maintainers.

## When CI runs

To save runner minutes, CI runs only when started by hand from the
repository's Actions tab, or when a version tag is pushed. Pull requests and
merges into `main` don't start it. A run started by hand checks only the
platforms the branch changes.

## Publishing a release

1. Raise `version` in the root `Cargo.toml`, which every package and app
   takes its version from, then `cargo update --workspace`, and merge that
   change.
2. Tag the release, and push the tag:

   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

Once every build passes, CI publishes the release: the pull requests merged
since the last release, with every build attached.

## Signing the Android app

So that each Android build installs over the last, every build is signed with
the same key. Create one and keep it safe:

```bash
keytool -genkeypair -keystore qdp-release.keystore -alias qdp -keyalg RSA -keysize 4096 -validity 10000
```

Give it to the repository as these secrets, under Settings, Secrets and
variables, Actions:

| Secret | Value |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | The keystore, base64-encoded |
| `ANDROID_KEYSTORE_PASSWORD` | The keystore's password |
| `ANDROID_KEY_ALIAS` | The key's alias, `qdp` above |
| `ANDROID_KEY_PASSWORD` | The key's password |

Local builds read the same key from `QDP_KEYSTORE`, `QDP_KEYSTORE_PASSWORD`,
`QDP_KEY_ALIAS` and `QDP_KEY_PASSWORD`.

## Signing the Mac apps

Releases sign the Mac app and the `qdp` command with the project's Developer
ID, and have Apple notarize them, so that macOS opens them without a warning.
A release fails without these secrets; other runs sign ad hoc.

| Secret | Value |
|---|---|
| `APPLE_DEVELOPER_ID_P12_BASE64` | The Developer ID Application certificate with its private key, exported from Keychain Access as a `.p12` without a password, base64-encoded |
| `APPLE_API_KEY_P8_BASE64` | A team API key from App Store Connect, under Users and Access, Integrations, base64-encoded |
| `APPLE_API_KEY_ID` | The API key's ID |
| `APPLE_API_ISSUER_ID` | The issuer ID above the list of API keys |

To base64-encode a file and copy it:

```bash
base64 -i developer-id.p12 | pbcopy
```
