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

Every Android build is signed with the same key, so that each installs over
the last; Google Play knows the app by it. A release fails without it; other
runs sign with a debug key. Create it once, as a keystore without a password,
and keep it safe, as it can't be made again:

```bash
openssl req -x509 -newkey rsa:4096 -nodes -keyout qdp-key.pem -out qdp-cert.pem -days 10000 -subj "/CN=Ian Armour"
openssl pkcs12 -export -inkey qdp-key.pem -in qdp-cert.pem -name qdp -out qdp-upload.keystore -passout pass:
rm qdp-key.pem qdp-cert.pem
```

Give it to the repository as these secrets, under Settings, Secrets and
variables, Actions:

| Secret | Value |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | The keystore, base64-encoded |
| `ANDROID_KEYSTORE_PASSWORD` | The keystore's password, if it has one |

Local builds read the same key from `QDP_KEYSTORE` and
`QDP_KEYSTORE_PASSWORD`.

## Publishing to Google Play

Releases upload the Android app to Google Play's closed testing track, as a
Google Cloud service account; without its key, they skip the upload.

1. Upload the first App Bundle by hand in Play Console, from a CI run's
   `play-bundle` artifact: Google takes uploads through its API only once an
   app has one.
2. In Google Cloud, enable the Google Play Android Developer API, and create a
   service account and a JSON key for it.
3. In Play Console, under Users and permissions, invite the service account's
   email, with permission to release the app to testing tracks.
4. Add the JSON key, as it is, as the `GOOGLE_PLAY_SERVICE_ACCOUNT_JSON`
   secret.

Until the app is first published, Google takes only draft releases: roll each
one out from Play Console.

## Signing the Apple apps

Releases sign the Mac app and the `qdp` command with the project's Developer
ID, and have Apple notarize them, so that macOS opens them without a warning.
They also upload the iPhone and iPad app to TestFlight. A release fails
without these secrets; other runs sign ad hoc.

| Secret | Value |
|---|---|
| `APPLE_DEVELOPER_ID_P12_BASE64` | The Developer ID Application certificate with its private key, exported from Keychain Access as a `.p12` without a password, base64-encoded |
| `APPLE_API_KEY_P8_BASE64` | A team API key with Admin access from App Store Connect, under Users and Access, Integrations, base64-encoded |
| `APPLE_API_KEY_ID` | The API key's ID |
| `APPLE_API_ISSUER_ID` | The issuer ID above the list of API keys |

To base64-encode a file and copy it:

```bash
base64 -i developer-id.p12 | pbcopy
```
