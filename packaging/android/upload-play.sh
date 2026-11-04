#!/usr/bin/env bash
# Uploads an Android App Bundle to a Google Play track as a release:
#
#   packaging/android/upload-play.sh BUNDLE.aab PACKAGE TRACK NAME
#
# Signs in as the Google Cloud service account whose JSON key is in
# GOOGLE_PLAY_SERVICE_ACCOUNT_JSON, which Play Console must let release the
# app. Play takes only draft releases until the app is first published, so
# one it refuses to release outright is left a draft, for Play Console to
# roll out. Needs curl, jq and openssl.
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "usage: $0 BUNDLE.aab PACKAGE TRACK NAME" >&2
  exit 2
fi
bundle=$1
package=$2
track=$3
name=$4
api="https://androidpublisher.googleapis.com/androidpublisher/v3/applications/$package"
upload="https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/$package"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

base64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

# An access token, for a JWT the service account's key signs.
jq -r .private_key <<< "$GOOGLE_PLAY_SERVICE_ACCOUNT_JSON" > "$work/key.pem"
now=$(date +%s)
header=$(printf '{"alg":"RS256","typ":"JWT"}' | base64url)
claims=$(jq -cn --arg iss "$(jq -r .client_email <<< "$GOOGLE_PLAY_SERVICE_ACCOUNT_JSON")" --argjson now "$now" \
  '{iss: $iss, scope: "https://www.googleapis.com/auth/androidpublisher",
    aud: "https://oauth2.googleapis.com/token", iat: $now, exp: ($now + 3600)}' | base64url)
signature=$(printf '%s.%s' "$header" "$claims" | openssl dgst -sha256 -sign "$work/key.pem" | base64url)

# Calls Google with curl's ARGUMENTS and prints its answer; one refusing
# goes to standard error too, and fails.
call() {
  local answer
  if answer=$(curl --fail-with-body -sS "$@"); then
    printf '%s\n' "$answer"
  else
    printf '%s\n' "$answer" | tee /dev/stderr
    return 1
  fi
}

signin=$(call https://oauth2.googleapis.com/token \
  --data-urlencode grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer \
  --data-urlencode "assertion=$header.$claims.$signature")
token=$(jq -r .access_token <<< "$signin")

play() {
  call -H "Authorization: Bearer $token" "$@"
}

# A release, of the uploaded bundle, at STATUS.
release() {
  jq -cn --arg track "$track" --arg name "$name" --arg code "$code" --arg status "$1" \
    '{track: $track, releases: [{name: $name, versionCodes: [$code], status: $status}]}'
}

edit=$(play -H "Content-Type: application/json" -d '{}' "$api/edits")
edit=$(jq -r .id <<< "$edit")
code=$(play -H "Content-Type: application/octet-stream" --data-binary "@$bundle" \
  "$upload/edits/$edit/bundles?uploadType=media")
code=$(jq -r .versionCode <<< "$code")
if ! answer=$(play -X PUT -H "Content-Type: application/json" -d "$(release completed)" \
  "$api/edits/$edit/tracks/$track"); then
  if [[ "$answer" != *"draft app"* ]]; then
    exit 1
  fi
  echo "::notice::Google Play takes only drafts until the app is published: roll out $name from Play Console."
  play -X PUT -H "Content-Type: application/json" -d "$(release draft)" \
    "$api/edits/$edit/tracks/$track" > /dev/null
fi
play -d '' "$api/edits/$edit:commit" > /dev/null
echo "uploaded $name, version code $code, to the $track track"
