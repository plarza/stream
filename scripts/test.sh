#!/bin/sh
# End-to-end UI tests in the iOS Simulator (never touches this Mac's mouse or keyboard).
set -eu
set -o pipefail
cd "$(dirname "$0")/.."
SIM="${SIM:-4C675D57-6CF7-4DDA-ACF4-4075857B2010}"
[ -d Generated/MusicCore.xcframework ] || PROFILE=debug scripts/build-core.sh
xcodegen generate >/dev/null
xcodebuild -project Music.xcodeproj -scheme Music-iOS -destination "platform=iOS Simulator,id=$SIM" \
  -derivedDataPath build test -skip-testing:MusicUITests-iOS/DemoFlow "$@" | grep -E 'Test Case .*(passed|failed)|error:|\*\* TEST'
