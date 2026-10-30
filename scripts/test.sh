#!/bin/sh
# End-to-end UI tests in the iOS Simulator (never touches this Mac's mouse or keyboard).
# Builds only when something changed since the last build; extra args go to xcodebuild,
# e.g. -only-testing:MusicUITests-iOS/iOSUITests/testSearchFindsSongs
set -eu
set -o pipefail
cd "$(dirname "$0")/.."
SIM="${SIM:-4C675D57-6CF7-4DDA-ACF4-4075857B2010}"
STAMP=build/.built-for-testing
[ -d Generated/MusicCore.xcframework ] || PROFILE=debug scripts/build-core.sh
xcrun simctl boot "$SIM" 2>/dev/null || true
run() {
  xcodebuild -project Music.xcodeproj -scheme Music-iOS -destination "platform=iOS Simulator,id=$SIM" \
    -derivedDataPath build "$@"
}
if [ ! -f "$STAMP" ] || [ -n "$(find project.yml Shared iOS tests/Shared tests/iOSUITests Generated -newer "$STAMP" -print -quit)" ]; then
  xcodegen generate --use-cache >/dev/null
  mkdir -p build
  run build-for-testing -quiet >build/build.log 2>&1 || { grep -E 'error:' build/build.log || tail -20 build/build.log; exit 1; }
  touch "$STAMP"
fi
run test-without-building -skip-testing:MusicUITests-iOS/DemoFlow "$@" 2>&1 | grep -E 'Test Case .*(passed|failed)|error:|\*\* TEST'
