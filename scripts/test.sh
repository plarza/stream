#!/bin/zsh
# Stream's tests. Neither part touches this Mac's mouse or keyboard.
#
#   scripts/test.sh          core tests, then the UI tests
#   scripts/test.sh core     core tests only: the library, sync between devices, playback,
#                            the queue and History, through the API the apps call (~1s
#                            once built). Run these while working.
#   scripts/test.sh ui ...   UI tests only, in the Simulator; extra args go to xcodebuild,
#                            e.g. -only-testing:StreamUITests-iOS/iOSUITests/testSearchFindsSongs
#
# Builds run at utility priority, so the Mac stays responsive. The UI tests build into
# build/tests (their own debug Simulator core and derived data, so nothing done for the
# Mac app or a deploy can break or slow them), rebuild only what changed, run on their
# own "Stream Tests" simulator, and take one run at a time: a second run waits for the
# first instead of fighting it for the Simulator. Every stage has a hard budget, and a
# stage that runs over is killed and named.
set -eu
set -o pipefail
# Everything here, builds included, yields to whatever you're doing on the Mac.
[[ -n "${STREAM_TEST_LOW:-}" ]] || STREAM_TEST_LOW=1 exec taskpolicy -c utility "$0" "$@"
cd "$(dirname "$0")/.."
ROOT=$PWD
T=$ROOT/build/tests
START=$SECONDS
MODE=all
case "${1:-}" in core | ui) MODE=$1; shift ;; esac
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

if [[ $MODE != ui ]]; then
  budget 300 "core tests" $T/core-tests.log cargo test --manifest-path core/Cargo.toml --test core \
    || { grep -E '^test .*FAILED|panicked|^  left|^ right|error(\[|:)' -A3 $T/core-tests.log | head -60; exit 1; }
  grep -E '^test result' $T/core-tests.log
fi
[[ $MODE == core ]] && exit 0

# One UI run at a time, across every checkout and worktree.
if [[ -z "${STREAM_TEST_LOCK:-}" ]]; then
  export STREAM_TEST_LOCK=1
  lockf -s -t 0 /tmp/stream-ui-tests.lock true 2>/dev/null || echo "· waiting for another UI test run to finish"
  exec lockf -s -t 900 /tmp/stream-ui-tests.lock "$0" ui "$@"
fi

# The tests' own simulator, made on the newest iOS if it's missing.
SIM=$(xcrun simctl list devices available -j | python3 -c '
import json, sys
for rt, ds in json.load(sys.stdin)["devices"].items():
    for d in ds:
        if d["name"] == "Stream Tests": print(d["udid"]); sys.exit()')
if [[ -z $SIM ]]; then
  RUNTIME=$(xcrun simctl list runtimes available -j | python3 -c '
import json, sys
print([r["identifier"] for r in json.load(sys.stdin)["runtimes"] if r["platform"] == "iOS"][-1])')
  SIM=$(xcrun simctl create "Stream Tests" com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro "$RUNTIME")
fi

if changed $T/.core core/src core/Cargo.toml core/Cargo.lock; then
  budget 300 "core for the Simulator" $T/core.log env PROFILE=debug ONLY=aarch64-apple-ios-sim OUT=$T/core scripts/build-core.sh \
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
# One simulator, no clones: parallel testing boots a copy per worker, which costs far
# more than it saves for a suite this size.
budget 180 "UI tests" $T/test.log run test-without-building \
  -parallel-testing-enabled NO -collect-test-diagnostics never \
  -test-timeouts-enabled YES -default-test-execution-time-allowance 60 "$@" || rc=$?
grep -E 'Test Case .*(passed|failed)|error:|\*\* TEST' $T/test.log
echo "· total $((SECONDS - START))s"
exit ${rc:-0}
