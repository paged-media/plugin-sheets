#!/usr/bin/env bash
# Build the COUNTING engine wasm: sheet-js with the `perf-counters` feature,
# which adds `perfCounters()` / `resetPerfCounters()` (the sheet-calc work
# counters) to the wasm. It lands in packages/sheet-bundle/bin-perf/
# (gitignored), never in bin/: the shipped wasm (scripts/build-wasm.sh)
# carries no counter code.
#
# Used by the TS perf harness (packages/sheet-bundle/test/perf/): with
# PERF_ENGINE=1 and this artifact present, the sessions boot it and
# `PERF_SHOW=1` prints the engine's own counts beside the door counts.
# Trended, not gated — CI does not build it.
#
# A separate --target-dir keeps the feature build from overwriting the
# release artifact build-wasm.sh reads.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=packages/sheet-bundle/bin-perf
TARGET_DIR=${CARGO_TARGET_DIR:-target}/wasm-perf

cargo build --release --target wasm32-unknown-unknown -p sheet-js \
  --features perf-counters --target-dir "$TARGET_DIR"

LOCKED=$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | grep version | head -1 | cut -d'"' -f2)
CLI=$(wasm-bindgen --version | awk '{print $2}')
if [ "$LOCKED" != "$CLI" ]; then
  echo "error: wasm-bindgen-cli $CLI != Cargo.lock wasm-bindgen $LOCKED" >&2
  exit 1
fi

mkdir -p "$OUT"
wasm-bindgen "$TARGET_DIR/wasm32-unknown-unknown/release/sheet_js.wasm" \
  --target web --out-dir "$OUT"
echo "counting wasm: $OUT/sheet_js_bg.wasm ($(wc -c < "$OUT/sheet_js_bg.wasm" | tr -d ' ') bytes)"
