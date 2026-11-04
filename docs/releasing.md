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
