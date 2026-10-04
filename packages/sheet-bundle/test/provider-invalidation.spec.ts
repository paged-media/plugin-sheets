// sheet.edit.ops — which session changes re-read the host's binding
// providers (binding-provider/invalidation.ts): the text provider on a
// selection move or an edit inside the selection, the swatches provider
// only on unclassified changes. Pure: a fake session signal.

import { describe, expect, it } from "vitest";

import {
  a1Bounds,
  regionsOf,
  subscribeProviderInvalidation,
  touchesSelection,
  type SessionChange,
} from "../src";

describe("provider invalidation [sheet.edit.ops]", () => {
  it("a1Bounds parses cells, ranges and absolute refs; null otherwise", () => {
    expect(a1Bounds("B2")).toEqual({ firstRow: 1, firstCol: 1, lastRow: 1, lastCol: 1 });
    expect(a1Bounds("$C$10:A1")).toEqual({ firstRow: 0, firstCol: 0, lastRow: 9, lastCol: 2 });
    expect(a1Bounds("AA3:AB4")).toEqual({ firstRow: 2, firstCol: 26, lastRow: 3, lastCol: 27 });
    expect(a1Bounds("Sheet1!A1")).toBeNull();
  });

  it("regionsOf bounds the cells per sheet", () => {
    expect(
      regionsOf([
        { sheet: 0, row: 5, col: 1 },
        { sheet: 0, row: 2, col: 4 },
        { sheet: 1, row: 0, col: 0 },
      ]),
    ).toEqual([
      { sheet: 0, firstRow: 2, firstCol: 1, lastRow: 5, lastCol: 4 },
      { sheet: 1, firstRow: 0, firstCol: 0, lastRow: 0, lastCol: 0 },
    ]);
  });

  it("touchesSelection: same sheet and overlapping; an unparsable range overlaps", () => {
    const r = regionsOf([{ sheet: 0, row: 3, col: 3 }]);
    expect(touchesSelection({ sheet: 0, range: "C3:E5" }, r)).toBe(true);
    expect(touchesSelection({ sheet: 0, range: "A1:B2" }, r)).toBe(false);
    expect(touchesSelection({ sheet: 1, range: "C3:E5" }, r)).toBe(false);
    expect(touchesSelection({ sheet: 0, range: "?" }, r)).toBe(true);
    expect(touchesSelection(null, r)).toBe(false);
  });

  it("invalidates each provider only on the changes that move its answer", () => {
    const listeners: ((c: SessionChange) => void)[] = [];
    let sel: { sheet: number; range: string } | null = { sheet: 0, range: "A1:B2" };
    const counts = { swatches: 0, text: 0 };
    const sub = subscribeProviderInvalidation(
      {
        onDidChange: (l) => {
          listeners.push(l);
          return { dispose: () => listeners.splice(listeners.indexOf(l), 1) };
        },
        textSelectionRange: () => sel,
      },
      {
        swatches: { invalidate: () => (counts.swatches += 1) },
        text: { invalidate: () => (counts.text += 1) },
      },
    );
    const fire = (c: SessionChange) => listeners.forEach((l) => l(c));
    fire({ kind: "selection" }); // same range: nothing moved
    expect(counts).toEqual({ swatches: 0, text: 0 });
    sel = { sheet: 0, range: "C3" };
    fire({ kind: "selection" }); // moved
    expect(counts).toEqual({ swatches: 0, text: 1 });
    fire({ kind: "cells", regions: regionsOf([{ sheet: 0, row: 9, col: 9 }]) }); // outside
    expect(counts).toEqual({ swatches: 0, text: 1 });
    fire({ kind: "cells", regions: regionsOf([{ sheet: 0, row: 2, col: 2 }]) }); // inside
    expect(counts).toEqual({ swatches: 0, text: 2 });
    fire({ kind: "other" });
    expect(counts).toEqual({ swatches: 1, text: 3 });
    sub.dispose();
    fire({ kind: "other" });
    expect(counts).toEqual({ swatches: 1, text: 3 });
  });
});
