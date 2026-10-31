#!/bin/sh
# Archive the iPhone app (Release) and upload it to TestFlight. Pass a build number,
# or it uses a timestamp. Needs the app's App Store Connect record (network.aza.stream).
set -eu
set -o pipefail
cd "$(dirname "$0")/.."
ASC_KEY_ID="${ASC_KEY_ID:-JH8H9T3AN9}"
ASC_ISSUER_ID="${ASC_ISSUER_ID:-570b1823-77bf-407c-b0a6-8b4c726f5741}"
AUTH="-allowProvisioningUpdates -authenticationKeyPath $HOME/.appstoreconnect/private_keys/AuthKey_$ASC_KEY_ID.p8 -authenticationKeyID $ASC_KEY_ID -authenticationKeyIssuerID $ASC_ISSUER_ID"
BUILD="${1:-$(date +%Y%m%d%H%M)}"
scripts/build-core.sh
xcodegen generate >/dev/null
xcodebuild -project Stream.xcodeproj -scheme Stream-iOS -configuration Release -destination 'generic/platform=iOS' \
  -archivePath build/Stream.xcarchive CURRENT_PROJECT_VERSION="$BUILD" $AUTH archive | grep -E 'error:|ARCHIVE'
cat > build/export.plist <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>method</key><string>app-store-connect</string>
  <key>destination</key><string>upload</string>
  <key>teamID</key><string>H5A6B4L9W8</string>
</dict></plist>
PLIST
xcodebuild -exportArchive -archivePath build/Stream.xcarchive -exportOptionsPlist build/export.plist \
  -exportPath build/export $AUTH 2>&1 | grep -E 'error:|EXPORT|Upload' | tee build/export.log
! grep -q 'EXPORT FAILED' build/export.log
