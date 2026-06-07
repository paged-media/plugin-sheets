// sheet.plugin.lower.mutations (placement half) — default frame
// placement: sizes the box from the IR's summed width/height, clamps to
// a max, sits at the fixed inset. Pure geometry.

import { describe, expect, it } from "vitest";

import {
  DEFAULT_INSET_PT,
  MAX_HEIGHT_PT,
  MAX_WIDTH_PT,
  defaultPlacement,
  totalHeightPt,
  totalWidthPt,
  type LoweredContent,
} from "../src";

function region(colW: number[], rowH: number[]): LoweredContent {
  return {
    cols: colW.map((widthPt, index) => ({ index, widthPt })),
    rows: rowH.map((heightPt, index) => ({ index, heightPt, cells: [] })),
    rules: { h: [], v: [] },
    merges: [],
  };
}

describe("sheet_plugin_lower_mutations: totals", () => {
  it("sums column widths and row heights", () => {
    const c = region([60, 40, 20], [16, 16, 16, 16]);
    expect(totalWidthPt(c)).toBe(120);
    expect(totalHeightPt(c)).toBe(64);
  });
});

describe("sheet_plugin_lower_mutations: defaultPlacement", () => {
  it("sizes the frame to the content total at the fixed inset", () => {
    const c = region([100, 100], [20, 20, 20]);
    const { pageId, bounds } = defaultPlacement("Page/u9", c);
    expect(pageId).toBe("Page/u9");
    const [top, left, bottom, right] = bounds;
    expect(top).toBe(DEFAULT_INSET_PT);
    expect(left).toBe(DEFAULT_INSET_PT);
    expect(right - left).toBe(200); // total width
    expect(bottom - top).toBe(60); // total height
  });

  it("clamps an oversized region to the maxima", () => {
    const c = region([5000], [5000]);
    const { bounds } = defaultPlacement("Page/u1", c);
    const [top, left, bottom, right] = bounds;
    expect(right - left).toBe(MAX_WIDTH_PT);
    expect(bottom - top).toBe(MAX_HEIGHT_PT);
  });

  it("an empty region is a degenerate inset-origin box", () => {
    const c = region([], []);
    const { bounds } = defaultPlacement("Page/u1", c);
    expect(bounds).toEqual([
      DEFAULT_INSET_PT,
      DEFAULT_INSET_PT,
      DEFAULT_INSET_PT,
      DEFAULT_INSET_PT,
    ]);
  });
});
