#!/usr/bin/env bash
# paged.sheet — Excel oracle lane, step 2: let the real Excel compute.
#
# Usage:  sheet-conformance/oracle/excel/drive.sh [family ...]
#   then: python3 sheet-conformance/oracle/excel/read.py
#
# MAINTAINER tool (macOS + Microsoft Excel 16). CI never runs it; CI consumes
# the committed recorded/*.tsv through tests/excel_oracle.rs.
#
# For each build/<family>.xlsx (from generate.py): stage a COPY inside Excel's
# own sandbox container, open it, `calculate full`, save as .xlsx, close, move
# the result back to build/<family>.excel.xlsx.
#
# The lifecycle is the one `~/paged/corpus/harness/convert-office.sh` learned
# the hard way (see its header): stage in the container — Excel cannot reach
# the repo and /private/tmp is reachable only by a grant that is not durable;
# quit gracefully before ever SIGKILLing; settle after launch; judge success by
# the OUTPUT FILE, never by the string AppleScript returns.

set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BUILD="${PAGED_ORACLE_BUILD:-$HERE/build}"
APP="Microsoft Excel"
CONTAINER="$HOME/Library/Containers/com.microsoft.Excel/Data/Documents"
TIMEOUT="${TIMEOUT:-300}"

DOMAIN=com.microsoft.Excel

# ── never touch a user's open work ──────────────────────────────────
# This script quits Excel (to pin its locale) and closes workbooks without
# saving. Refuse if anything is open.
if pgrep -x "$APP" >/dev/null 2>&1; then
    n_open="$(timeout 10 osascript -e "tell application \"$APP\" to count of workbooks" 2>/dev/null || echo "?")"
    if [ "$n_open" != "0" ]; then
        echo "error: Excel is running with $n_open open workbook(s) — save and quit it first" >&2
        exit 1
    fi
fi

mkdir -p "$CONTAINER"
SCRATCH="$(mktemp -d "$CONTAINER/paged-oracle.XXXXXX")" || {
    echo "error: cannot stage inside $CONTAINER" >&2; exit 1; }
cp "$HERE/recalc.applescript" "$SCRATCH/recalc.applescript"

# ── pin Excel to en-US for the run ──────────────────────────────────
# Excel computes in its UI language and region: under de-AT, CONCAT(TRUE) is
# "WAHR", VALUE("3.14") is a date and FIXED() swaps separators — 37 of the
# first run's 72 "disagreements" were that. The engine's dialect is en-US, so
# the oracle must be too. Per-app keys only (never the global locale); keys
# this script ADDS are deleted again on exit, pre-existing different values
# make it refuse rather than overwrite them. read.py double-checks through the
# locale-probe sheet, so a run that slipped past this cannot be recorded.
ADDED_KEYS=""
pin_key() {  # pin_key <key> <type-flag> <value> <expected-read>
    local cur
    if cur="$(defaults read "$DOMAIN" "$1" 2>/dev/null)"; then
        cur="$(echo "$cur" | tr -d ' \n"()')"
        if [ "$cur" != "$4" ]; then
            echo "error: $DOMAIN $1 is already set to '$cur' — not overwriting it" >&2
            return 1
        fi
    else
        defaults write "$DOMAIN" "$1" "$2" "$3" || return 1
        ADDED_KEYS="$ADDED_KEYS $1"
    fi
}
cleanup() {
    rm -rf "$SCRATCH"
    for k in $ADDED_KEYS; do defaults delete "$DOMAIN" "$k" >/dev/null 2>&1; done
}
trap cleanup EXIT

close_all() {
    timeout 15 osascript -e "tell application \"$APP\" to close every workbook saving no" \
        >/dev/null 2>&1
}

launch_app() {
    open -a "$APP" 2>/dev/null
    for _ in $(seq 1 25); do
        if timeout 8 osascript -e "tell application \"$APP\" to get version" >/dev/null 2>&1; then
            close_all
            sleep 5   # answering `get version` is not proof `open` works yet
            return 0
        fi
        sleep 1
    done
    echo "error: $APP never became scriptable — if macOS showed an Automation" >&2
    echo "  prompt, allow Terminal (or your shell host) to control Microsoft Excel in" >&2
    echo "  System Settings > Privacy & Security > Automation, then re-run." >&2
    return 1
}

kill_app() {
    close_all
    timeout 20 osascript -e "tell application \"$APP\" to quit saving no" >/dev/null 2>&1
    for _ in 1 2 3 4 5; do
        pgrep -x "$APP" >/dev/null 2>&1 || return 0
        sleep 1
    done
    pkill -x "$APP" 2>/dev/null; sleep 2
    pgrep -x "$APP" >/dev/null 2>&1 || return 0
    pkill -9 -x "$APP" 2>/dev/null; sleep 1
}

if [ $# -gt 0 ]; then
    FAMS=("$@")
else
    FAMS=()
    for f in "$BUILD"/*.xlsx; do
        case "$f" in *.excel.xlsx) continue ;; esac
        FAMS+=("$(basename "$f" .xlsx)")
    done
fi
[ ${#FAMS[@]} -gt 0 ] || { echo "error: nothing in $BUILD — run generate.py" >&2; exit 1; }

if pgrep -x "$APP" >/dev/null 2>&1; then kill_app; fi   # locale is read at launch
pin_key AppleLanguages -array en-US en-US || exit 1
pin_key AppleLocale -string en_US en_US || exit 1
launch_app || exit 1
ok=0; failed=0
for fam in "${FAMS[@]}"; do
    src="$BUILD/$fam.xlsx"
    dst="$BUILD/$fam.excel.xlsx"
    [ -f "$src" ] || { echo "  $fam: no $src"; failed=$((failed + 1)); continue; }
    rm -f "$dst" "$SCRATCH/in.xlsx" "$SCRATCH/out.xlsx"
    cp "$src" "$SCRATCH/in.xlsx"
    out="$(timeout "$TIMEOUT" osascript "$SCRATCH/recalc.applescript" \
        "$SCRATCH/in.xlsx" "$SCRATCH/out.xlsx" 2>&1 < /dev/null)"
    rc=$?
    if [ -f "$SCRATCH/out.xlsx" ] && [ "$(stat -f%z "$SCRATCH/out.xlsx")" -gt 1024 ]; then
        mv "$SCRATCH/out.xlsx" "$dst"
        ok=$((ok + 1))
        echo "  $fam: OK ($out)"
    else
        failed=$((failed + 1))
        echo "  $fam: FAIL rc=$rc ($out)"
        if [ $rc -eq 124 ] || [ $rc -eq 143 ]; then
            kill_app; launch_app || exit 1
        fi
    fi
done
kill_app
echo "excel oracle: recalculated=$ok failed=$failed of ${#FAMS[@]}"
[ "$failed" -eq 0 ]
