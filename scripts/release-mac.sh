#!/bin/sh
# Archive the Mac app (Release), sign it with Developer ID, notarize and staple it, and
# publish it as a GitHub release of plarza/stream. Pass a build number, or it uses a
# timestamp. Signs through the App Store Connect key, so it needs no certificate here.
set -eu
set -o pipefail
cd "$(dirname "$0")/.."
ASC_KEY_ID="${ASC_KEY_ID:-JH8H9T3AN9}"
ASC_ISSUER_ID="${ASC_ISSUER_ID:-570b1823-77bf-407c-b0a6-8b4c726f5741}"
KEY="$HOME/.appstoreconnect/private_keys/AuthKey_$ASC_KEY_ID.p8"
AUTH="-allowProvisioningUpdates -authenticationKeyPath $KEY -authenticationKeyID $ASC_KEY_ID -authenticationKeyIssuerID $ASC_ISSUER_ID"
BUILD="${1:-$(date +%Y%m%d%H%M)}"
VERSION=$(sed -nE 's/^ *MARKETING_VERSION: "(.*)"/\1/p' project.yml | head -1)
TAG="v$VERSION-$BUILD"
OUT=build/mac-release
rm -rf "$OUT" && mkdir -p "$OUT"

scripts/build-core.sh
xcodegen generate >/dev/null
xcodebuild -project Stream.xcodeproj -scheme Stream-macOS -configuration Release -destination 'generic/platform=macOS' \
  -archivePath "$OUT/Stream.xcarchive" CURRENT_PROJECT_VERSION="$BUILD" $AUTH archive | grep -E 'error:|ARCHIVE'
cat > "$OUT/export.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>developer-id</string>
  <key>destination</key><string>export</string>
  <key>signingStyle</key><string>automatic</string>
  <key>teamID</key><string>H5A6B4L9W8</string>
</dict></plist>
PLIST
xcodebuild -exportArchive -archivePath "$OUT/Stream.xcarchive" -exportOptionsPlist "$OUT/export.plist" \
  -exportPath "$OUT" $AUTH 2>&1 | grep -E 'error:|EXPORT' | tee "$OUT/export.log"
! grep -q 'EXPORT FAILED' "$OUT/export.log"

# Apple checks it, then the ticket is stapled on so it opens offline too.
ditto -c -k --keepParent "$OUT/Stream.app" "$OUT/notarize.zip"
xcrun notarytool submit "$OUT/notarize.zip" --key "$KEY" --key-id "$ASC_KEY_ID" --issuer "$ASC_ISSUER_ID" --wait | tee "$OUT/notary.log"
grep -q 'status: Accepted' "$OUT/notary.log"
xcrun stapler staple "$OUT/Stream.app"
ZIP="$OUT/Stream-$VERSION-$BUILD-mac.zip"
ditto -c -k --keepParent "$OUT/Stream.app" "$ZIP"

# Notes: what changed since the last release (or the last 20 commits for the first).
PREV=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)
{
  echo "Stream for Mac (Apple silicon, macOS 27). Unzip and drag Stream.app to Applications."
  echo
  echo "## Changes"
  if [ -n "$PREV" ]; then git log --format='- %s' "$PREV..HEAD"; else git log -20 --format='- %s'; fi
} > "$OUT/notes.md"
git tag "$TAG" && git push -q origin "$TAG" && git push -q github "$TAG"
gh release create "$TAG" -R plarza/stream --title "Stream $VERSION ($BUILD)" --notes-file "$OUT/notes.md" "$ZIP"
