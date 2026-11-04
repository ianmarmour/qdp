#!/usr/bin/env bash
# Imports the Developer ID Application certificate into a keychain of its own
# that codesign and Xcode search, for this CI run only:
#
#   packaging/macos/import-certificate.sh
#
# Reads the certificate, a base64-encoded .p12 without a password, from
# APPLE_DEVELOPER_ID_P12_BASE64.
set -euo pipefail

keychain="$RUNNER_TEMP/signing.keychain-db"
password=$(openssl rand -hex 24)
certificate="$RUNNER_TEMP/developer-id.p12"
authority="$RUNNER_TEMP/DeveloperIDG2CA.cer"
trap 'rm -f "$certificate" "$authority"' EXIT

# Imports a file, leaving out what the keychain has already: a .p12 can hold
# a certificate twice.
import_file() {
  local output
  if ! output=$(security import "$@" -k "$keychain" 2>&1); then
    if [[ "$output" != *"already exists"* ]]; then
      echo "$output" >&2
      return 1
    fi
  fi
  echo "$output"
}

printf %s "$APPLE_DEVELOPER_ID_P12_BASE64" | base64 --decode > "$certificate"
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
import_file "$certificate" -f pkcs12 -P "" -T /usr/bin/codesign
# codesign needs the authority that issued the certificate, which exports
# can leave out.
if ! security find-certificate -c "Developer ID Certification Authority" "$keychain" > /dev/null 2>&1; then
  curl -fsSL https://www.apple.com/certificateauthority/DeveloperIDG2CA.cer -o "$authority"
  import_file "$authority"
fi
security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
  -k "$password" "$keychain" > /dev/null
read -ra searched <<< "$(security list-keychains -d user | tr -d '"')"
security list-keychains -d user -s "$keychain" "${searched[@]}"

if ! security find-identity -v -p codesigning "$keychain" | grep -q '"Developer ID Application'; then
  echo "::error::The certificate has no Developer ID Application identity with its private key." >&2
  security find-identity -p codesigning "$keychain" >&2
  exit 1
fi
