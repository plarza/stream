#!/bin/sh
# Build Release and install it on this Mac (~/Applications). The iPhone gets builds
# through TestFlight only: scripts/testflight.sh.
set -eu
set -o pipefail
cd "$(dirname "$0")/.."
scripts/build-core.sh
xcodegen generate >/dev/null
xcodebuild -project Stream.xcodeproj -scheme Stream-macOS -configuration Release -destination 'platform=macOS' \
  -derivedDataPath build build | grep -E 'error:|BUILD'
pkill -f "$HOME/Applications/Stream.app/Contents/MacOS/Stream" || true
rm -rf "$HOME/Applications/Stream.app" && cp -R build/Build/Products/Release/Stream.app "$HOME/Applications/"
echo "Installed ~/Applications/Stream.app"
