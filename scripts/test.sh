#!/bin/zsh
# End-to-end UI tests in the iOS Simulator (never touches this Mac's mouse or keyboard).
# Extra args go to xcodebuild, e.g. -only-testing:StreamUITests-iOS/iOSUITests/testSearchFindsSongs
#
# Everything the tests build lives in build/tests: a debug Simulator-only core and its
# own derived data, built from StreamTests.xcodeproj (the same project, pointed at that core). Nothing
# done for the Mac app or a deploy (Generated/, build/) can break or slow a test run,
# and each stage only rebuilds when its sources changed.
#
# The suite takes about a minute. Every stage has a hard budget: a stage that runs
# over is killed and the run fails, naming the stage, rather than dragging on.
set -eu
set -o pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
SIM="${SIM:-D8A496FC-0088-4FBB-BC81-631CF30BD8A0}"
T=$ROOT/build/tests
START=$SECONDS
mkdir -p $T

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
# changed <stamp> <paths…>: true when the stamp is missing or anything is newer.
changed() {
  local stamp=$1; shift
  [ ! -f "$stamp" ] || [ -n "$(find "$@" -newer "$stamp" -print -quit)" ]
}

if changed $T/.core core/src core/Cargo.toml core/Cargo.lock; then
  budget 300 "core" $T/core.log env PROFILE=debug ONLY=aarch64-apple-ios-sim OUT=$T/core scripts/build-core.sh \
    || { tail -20 $T/core.log; exit 1; }
  touch $T/.core
fi

budget 60 "boot simulator" $T/boot.log xcrun simctl bootstatus "$SIM" -b

run() {
  xcodebuild -project StreamTests.xcodeproj -scheme Stream-iOS -destination "platform=iOS Simulator,id=$SIM" \
    -derivedDataPath $T/dd "$@"
}
if changed $T/.app project.yml Shared iOS tests/Shared tests/iOSUITests $T/.core; then
  # The same project, pointed at the tests' own core.
  sed -e "s#Generated/#build/tests/core/#g" -e "s#^name: Stream\$#name: StreamTests#" project.yml > $T/project.yml
  xcodegen generate --spec $T/project.yml --project-root $ROOT --project $ROOT >/dev/null
  budget 300 "build" $T/build.log run build-for-testing -quiet \
    || { grep -E 'error:' $T/build.log || tail -20 $T/build.log; exit 1; }
  touch $T/.app
fi

# A run killed over budget can leave its runner hanging in the Simulator, and the next
# run would sit waiting on it; clear both out first.
xcrun simctl terminate "$SIM" network.aza.stream.uitests.xctrunner 2>/dev/null || true
xcrun simctl terminate "$SIM" network.aza.stream 2>/dev/null || true
budget 150 "tests" $T/test.log run test-without-building -skip-testing:StreamUITests-iOS/DemoFlow \
  -test-timeouts-enabled YES -default-test-execution-time-allowance 60 "$@" || rc=$?
grep -E 'Test Case .*(passed|failed)|error:|\*\* TEST' $T/test.log
echo "· total $((SECONDS - START))s"
exit ${rc:-0}
