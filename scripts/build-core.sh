#!/bin/zsh
# Builds the Rust core for iPhone, the simulator and Mac, generates the Swift
# bindings, and packs everything into Generated/StreamCore.xcframework.
#
# OUT=<dir> ONLY="<targets>" packs just those targets somewhere else (the UI tests
# build their own Simulator-only core this way). Generated/ always gets every
# target: a framework missing a platform breaks that platform's build.
set -euo pipefail
cd "$(dirname "$0")/../core"

PROFILE=${PROFILE:-release}
FLAG=$([[ $PROFILE == release ]] && echo --release || echo)
OUT=${OUT:-../Generated}
TARGETS=(aarch64-apple-ios aarch64-apple-ios-sim aarch64-apple-darwin)
if [[ -n "${ONLY:-}" ]]; then
  if [[ $OUT:A == ${PWD:h}/Generated ]]; then
    echo "✗ Generated/ needs every target; use OUT=<dir> with ONLY" >&2
    exit 1
  fi
  TARGETS=(${=ONLY})
fi

export IPHONEOS_DEPLOYMENT_TARGET=26.0
export MACOSX_DEPLOYMENT_TARGET=26.0
for t in $TARGETS; do
  cargo build $FLAG --lib --target $t
done

rm -rf $OUT/StreamCore.xcframework $OUT/headers
mkdir -p $OUT/headers

HOST_LIB=target/aarch64-apple-darwin/$PROFILE/libstream_core.dylib
[[ -f $HOST_LIB ]] || cargo build $FLAG --lib --target aarch64-apple-darwin
cargo run -q --bin uniffi-bindgen -- generate --library $HOST_LIB --language swift --out-dir $OUT/bindings
mv $OUT/bindings/stream_coreFFI.h $OUT/headers/
cat > $OUT/headers/StreamCoreRender.h <<'H'
#include <stdint.h>
/// Pulls `frames` of audio into the two channel buffers; returns frames of real audio.
uint32_t stream_core_render(float *left, float *right, uint32_t frames);
H
cat > $OUT/headers/module.modulemap <<'M'
module stream_coreFFI {
    header "stream_coreFFI.h"
    header "StreamCoreRender.h"
    export *
}
M
mv $OUT/bindings/stream_core.swift $OUT/StreamCore.swift
rm -rf $OUT/bindings

ARGS=()
for t in $TARGETS; do
  ARGS+=(-library target/$t/$PROFILE/libstream_core.a -headers $OUT/headers)
done
xcodebuild -create-xcframework $ARGS -output $OUT/StreamCore.xcframework >/dev/null
rm -rf $OUT/headers
echo "Built $OUT/StreamCore.xcframework ($PROFILE: $TARGETS)"
