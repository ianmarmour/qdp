#!/usr/bin/env bash
# Has Apple notarize a signed app or command, and staples the ticket to an
# app, so that it opens without a network connection too:
#
#   packaging/macos/notarize.sh "Quick Desktop.app"
#   packaging/macos/notarize.sh target/release/qdp
#
# Signs in with the App Store Connect API key, base64-encoded, in
# APPLE_API_KEY_P8_BASE64, with its ID in APPLE_API_KEY_ID and its issuer's
# in APPLE_API_ISSUER_ID.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: $0 PATH" >&2
  exit 2
fi
path=$1
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

printf %s "$APPLE_API_KEY_P8_BASE64" | base64 --decode > "$work/key.p8"
key=(--key "$work/key.p8" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID")
ditto -c -k --keepParent "$path" "$work/upload.zip"
xcrun notarytool submit "$work/upload.zip" "${key[@]}" --wait \
  --output-format json > "$work/result.json" || true
status=$(jq -r '.status // empty' "$work/result.json")
if [ "$status" != Accepted ]; then
  cat "$work/result.json" >&2
  id=$(jq -r '.id // empty' "$work/result.json")
  if [ -n "$id" ]; then
    xcrun notarytool log "$id" "${key[@]}" >&2
  fi
  exit 1
fi
if [[ "$path" == *.app ]]; then
  xcrun stapler staple "$path"
fi
