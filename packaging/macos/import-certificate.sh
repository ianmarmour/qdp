#!/usr/bin/env bash
# Imports the Developer ID Application certificate into a keychain of its own
# that codesign and Xcode search, for this CI run only:
#
#   packaging/macos/import-certificate.sh
#
# Reads the certificate, a base64-encoded .p12, from
# APPLE_DEVELOPER_ID_P12_BASE64, and its password from
# APPLE_DEVELOPER_ID_P12_PASSWORD.
set -euo pipefail

keychain="$RUNNER_TEMP/signing.keychain-db"
password=$(openssl rand -hex 24)
certificate="$RUNNER_TEMP/developer-id.p12"
trap 'rm -f "$certificate"' EXIT

printf %s "$APPLE_DEVELOPER_ID_P12_BASE64" | base64 --decode > "$certificate"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$certificate" -k "$keychain" -f pkcs12 \
  -P "$APPLE_DEVELOPER_ID_P12_PASSWORD" -T /usr/bin/codesign
# Exported certificates leave out the authority that issued them, which
# codesign needs to sign.
curl -fsSL https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer \
  -o "$RUNNER_TEMP/DeveloperIDG2CA.cer"
security import "$RUNNER_TEMP/DeveloperIDG2CA.cer" -k "$keychain"
security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
  -k "$password" "$keychain" > /dev/null
read -ra searched <<< "$(security list-keychains -d user | tr -d '"')"
security list-keychains -d user -s "$keychain" "${searched[@]}"
