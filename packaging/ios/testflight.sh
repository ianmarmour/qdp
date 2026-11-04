#!/usr/bin/env bash
# Archives the iPhone and iPad app, signs it for the App Store and uploads it
# to TestFlight:
#
#   packaging/ios/testflight.sh TEAM_ID
#
# Run from the repository after apple/build-core.sh and xcodegen generate.
# Signs in with the App Store Connect API key, base64-encoded, in
# APPLE_API_KEY_P8_BASE64, with its ID in APPLE_API_KEY_ID and its issuer's in
# APPLE_API_ISSUER_ID. The key needs Admin access, with which Xcode makes the
# App Store certificate and profile itself. Xcode numbers each build after
# the last one App Store Connect has.
set -euo pipefail

if [ "$#" -ne 1 ] || [ -z "$1" ]; then
  echo "usage: $0 TEAM_ID" >&2
  exit 2
fi
team=$1
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

printf %s "$APPLE_API_KEY_P8_BASE64" | base64 --decode > "$work/key.p8"
auth=(-allowProvisioningUpdates -authenticationKeyPath "$work/key.p8"
  -authenticationKeyID "$APPLE_API_KEY_ID" -authenticationKeyIssuerID "$APPLE_API_ISSUER_ID")

xcodebuild -project apple/QDP.xcodeproj -scheme QDP-iOS -configuration Release \
  -destination 'generic/platform=iOS' -archivePath "$work/QDP.xcarchive" \
  CODE_SIGN_STYLE=Automatic DEVELOPMENT_TEAM="$team" "${auth[@]}" archive

cat > "$work/export.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>method</key>
	<string>app-store-connect</string>
	<key>destination</key>
	<string>upload</string>
	<key>signingStyle</key>
	<string>automatic</string>
	<key>teamID</key>
	<string>$team</string>
</dict>
</plist>
PLIST
xcodebuild -exportArchive -archivePath "$work/QDP.xcarchive" \
  -exportOptionsPlist "$work/export.plist" -exportPath "$work/export" "${auth[@]}"
