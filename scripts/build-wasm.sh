#!/usr/bin/env bash
# Build the paged.sheet engine wasm (sheet-js) and land the wasm-bindgen
# `--target web` output in packages/sheet-bundle/bin/ — the path the
# manifest declares under capabilities.wasm[] (governance + the 100 MB app-wide
# plugin-cli size gate). The bundle loads it via the wbindgen glue (the
# core/canvas-wasm pattern), NOT via loadBundleWasm — BREAKAGE S-10.
#
# wasm-opt: CI pins binaryen (old apt binaryen breaks wasm-bindgen
# externref table grow — the "Table.grow failed" gotcha); locally it is
# applied when present, skipped with a warning when absent.
set -euo pipefail
cd "$(dirname "$0")/.."

OUT=packages/sheet-bundle/bin
# The budget is now 100 MB for the WHOLE APP including every plugin
# (maintainer decision, 2026-08-19), enforced as a SUM by the editor's
# scripts/wasm-budget.mjs. This per-artifact stop keeps a runaway build
# from sailing through unnoticed here; the number that governs is the app
# total. Mirrors plugin-sdk WASM_BUDGETS — change them together.
BUDGET=$((100 * 1000 * 1000))

cargo build --release --target wasm32-unknown-unknown -p sheet-js

# Pin check: wasm-bindgen-cli must match the Cargo.lock wasm-bindgen.
LOCKED=$(grep -A1 '^name = "wasm-bindgen"$' Cargo.lock | grep version | head -1 | cut -d'"' -f2)
CLI=$(wasm-bindgen --version | awk '{print $2}')
if [ "$LOCKED" != "$CLI" ]; then
  echo "error: wasm-bindgen-cli $CLI != Cargo.lock wasm-bindgen $LOCKED" >&2
  echo "       cargo install wasm-bindgen-cli --version $LOCKED" >&2
  exit 1
fi

wasm-bindgen target/wasm32-unknown-unknown/release/sheet_js.wasm \
  --target web --out-dir "$OUT"

if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -Oz "$OUT/sheet_js_bg.wasm" -o "$OUT/sheet_js_bg.wasm"
else
  echo "warning: wasm-opt not found — shipping unoptimized wasm (CI optimizes)" >&2
fi

SIZE=$(wc -c < "$OUT/sheet_js_bg.wasm" | tr -d ' ')
echo "sheet_js_bg.wasm: $SIZE bytes (budget $BUDGET)"
if [ "$SIZE" -gt "$BUDGET" ]; then
  echo "error: wasm artifact exceeds the 100 MB app wasm budget" >&2
  exit 1
fi
