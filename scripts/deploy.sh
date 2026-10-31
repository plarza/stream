#!/bin/sh
# Build Release, install on this Mac (~/Applications) and on the iPhone, and launch on the iPhone.
set -eu
set -o pipefail
cd "$(dirname "$0")/.."
DEVICE="${DEVICE:-00008140-0012601E0A0B001C}"
scripts/build-core.sh
xcodegen generate >/dev/null
xcodebuild -project Stream.xcodeproj -scheme Stream-macOS -configuration Release -destination 'platform=macOS' \
  -derivedDataPath build build | grep -E 'error:|BUILD'
pkill -f "$HOME/Applications/Stream.app/Contents/MacOS/Stream" || true
rm -rf "$HOME/Applications/Stream.app" && cp -R build/Build/Products/Release/Stream.app "$HOME/Applications/"
echo "Installed ~/Applications/Stream.app"
xcodebuild -project Stream.xcodeproj -scheme Stream-iOS -configuration Release -destination "id=$DEVICE" \
  -derivedDataPath build -allowProvisioningUpdates build | grep -E 'error:|BUILD'
xcrun devicectl device install app --device "$DEVICE" "$PWD/build/Build/Products/Release-iphoneos/Stream.app" | tail -2
xcrun devicectl device process launch --device "$DEVICE" network.aza.stream | tail -1 || true
