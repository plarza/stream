#!/bin/zsh
# End-to-end UI tests in the iOS Simulator (never touches this Mac's mouse or keyboard).
# Builds only when something changed since the last build; extra args go to xcodebuild,
# e.g. -only-testing:MusicUITests-iOS/iOSUITests/testSearchFindsSongs
#
# The suite should take about a minute. Every stage has a hard budget: a stage that
# runs over is killed and the run fails, naming the stage, rather than dragging on.
set -eu
set -o pipefail
cd "$(dirname "$0")/.."
SIM="${SIM:-BC845076-8DE4-4379-88C8-F0F002C9CA47}"
STAMP=build/.built-for-testing
START=$SECONDS
mkdir -p build

# budget <seconds> <stage> <log> <command…>: runs the command into the log, killing it past the budget.
budget() {
  local secs=$1 stage=$2 log=$3; shift 3
  local t=$SECONDS
  "$@" >"$log" 2>&1 &
  local pid=$!
  ( sleep "$secs"; kill -TERM $pid 2>/dev/null && pkill -TERM -P $pid 2>/dev/null ) &
  local watchdog=$!
  local rc=0
  wait $pid || rc=$?
  kill $watchdog 2>/dev/null || true
  if (( SECONDS - t >= secs )); then
    echo "✗ $stage ran over its ${secs}s budget; see $log" >&2
    exit 1
  fi
  echo "· $stage $((SECONDS - t))s"
  return $rc
}

[ -d Generated/MusicCore.xcframework ] || PROFILE=debug ONLY=aarch64-apple-ios-sim scripts/build-core.sh
budget 60 "boot simulator" build/boot.log xcrun simctl bootstatus "$SIM" -b
run() {
  xcodebuild -project Music.xcodeproj -scheme Music-iOS -destination "platform=iOS Simulator,id=$SIM" \
    -derivedDataPath build "$@"
}
if [ ! -f "$STAMP" ] || [ -n "$(find project.yml Shared iOS tests/Shared tests/iOSUITests Generated -newer "$STAMP" -print -quit)" ]; then
  xcodegen generate --use-cache >/dev/null
  budget 300 "build" build/build.log run build-for-testing -quiet \
    || { grep -E 'error:' build/build.log || tail -20 build/build.log; exit 1; }
  touch "$STAMP"
fi
budget 150 "tests" build/test.log run test-without-building -skip-testing:MusicUITests-iOS/DemoFlow \
  -test-timeouts-enabled YES -default-test-execution-time-allowance 60 "$@" || rc=$?
grep -E 'Test Case .*(passed|failed)|error:|\*\* TEST' build/test.log
echo "· total $((SECONDS - START))s"
exit ${rc:-0}
