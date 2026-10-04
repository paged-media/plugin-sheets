#!/usr/bin/env python3
# paged.sheet — Excel oracle lane: find the cases Excel refuses to enter.
#
# Usage:  python3 find_rejects.py <family> [...]     (needs macOS + Excel)
#
# When ONE formula in a workbook is something Excel's parser rejects (an arity
# violation such as `=FV(0.1,5)`), Excel declines the whole file with -50
# "parameter error" and records nothing for the other cases. This bisects a
# family's cases down to the culprits and prints `file<TAB>id` lines to paste
# into excel-rejects.tsv. Cases already listed there are excluded first, so a
# clean family prints nothing.

from __future__ import annotations

import os
import shutil
import subprocess
import sys
import tempfile

import generate as g
import oracle_cases as oc

CONTAINER = os.path.expanduser("~/Library/Containers/com.microsoft.Excel/Data/Documents")
SCPT = str(oc.HERE / "recalc.applescript")


def opens(stage: str, subset: list[oc.Case]) -> bool:
    real = oc.load_family
    oc.load_family = lambda _f: list(subset)
    try:
        g.write_family("probe")
    finally:
        oc.load_family = real
    src, dst = os.path.join(stage, "probe.xlsx"), os.path.join(stage, "out.xlsx")
    if os.path.exists(dst):
        os.remove(dst)
    subprocess.run(["timeout", "180", "osascript", SCPT, src, dst],
                   capture_output=True, text=True)
    return os.path.exists(dst)  # judged by the artifact, not the reply


def culprits(stage: str, sub: list[oc.Case]) -> list[oc.Case]:
    if opens(stage, sub):
        return []
    if len(sub) == 1:
        return sub
    h = len(sub) // 2
    return culprits(stage, sub[:h]) + culprits(stage, sub[h:])


def main(argv: list[str]) -> int:
    stage = tempfile.mkdtemp(dir=CONTAINER, prefix="paged-rejects.")
    os.environ["PAGED_ORACLE_BUILD"] = stage
    try:
        known = oc.rejected_keys()
        for fam in argv or oc.families():
            cases = [c for c in oc.load_family(fam) if c.key not in known]
            for c in culprits(stage, cases):
                print(f"{c.file}\t{c.id}\t# {c.formula}")
    finally:
        shutil.rmtree(stage, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
