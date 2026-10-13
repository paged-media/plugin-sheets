/*
 * This file is part of paged (https://paged.media).
 *
 * paged is free software: you may redistribute it and/or modify it under the
 * terms of the GNU Affero General Public License, version 3, as published by
 * the Free Software Foundation, OR under the Paged Media Enterprise License
 * (PMEL), a commercial license available from And The Next GmbH. Full
 * copyright and license information is available in LICENSE.md, distributed
 * with this source code.
 *
 * paged is distributed in the hope that it will be useful, but WITHOUT ANY
 * WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS
 * FOR A PARTICULAR PURPOSE. See the licenses for details.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    AGPL-3.0-only OR Paged Media Enterprise License (PMEL)
 */

// sheet.plugin.lower.mutations (placement half) — default frame
// placement: sizes the box from the IR's summed width/height, clamps to
// a max, sits at the fixed inset. Pure geometry.

import { describe, expect, it } from "vitest";

import {
  DEFAULT_INSET_PT,
  FIT_ALLOWANCE_PT,
  MAX_HEIGHT_PT,
  MAX_WIDTH_PT,
  defaultPlacement,
  tableFrameHeightPt,
  tableLeadInPt,
  tableRowHeightsPt,
  TABLE_LEAD_IN_LINE,
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

/** A region whose every row holds one cell of text (`texts[r]`). */
function textRegion(rowH: number[], texts: string[]): LoweredContent {
  return {
    cols: [{ index: 0, widthPt: 80 }],
    rows: rowH.map((heightPt, index) => ({
      index,
      heightPt,
      cells: [{ col: 0, text: texts[index] ?? "", align: "general" as const }],
    })),
    rules: { h: [], v: [] },
    merges: [],
  };
}

// SHEET-1 (found filming the editor): a 7-row price list placed in a frame
// as tall as its 7 × 15 pt workbook rows came out overset. The engine lays
// a placed table out one line down the frame and grows a row to its tallest
// cell; the frame has to be sized to THAT. The numbers below are the real
// engine's (lower-fit.spec.ts pins the same against it).
describe("sheet_plugin_lower_mutations: a native table's frame height [sheet.lower.page]", () => {
  it("adds the engine's lead-in line: the ascender plus 0.4 of the 12 pt default", () => {
    // The engine's metric-less estimate (ascender 0.8 em): measured 14.41.
    expect(tableLeadInPt({ defaultAscenderPt: 9.6 }, true)).toBeCloseTo(14.4, 6);
    // Inter (ascender 0.96875 em = 11.625 pt at 12): measured 16.43.
    expect(tableLeadInPt({ defaultAscenderPt: 11.625 }, true)).toBeCloseTo(16.425, 6);
  });

  it("an engine that places the table in the story's first paragraph adds no lead-in", () => {
    expect(tableLeadInPt({ defaultAscenderPt: 11.625 }, false)).toBe(0);
  });

  it("the price list's 105 pt of rows needs the lead-in on top — 121.4 pt under Inter", () => {
    const c = textRegion([15, 15, 15, 15, 15, 15, 15], ["Item", "Notebooks", "Pencils", "Erasers", "Rulers", "Markers", "Total"]);
    const fit = { defaultAscenderPt: 11.625, cellAscenderPt: () => 11.625 };
    const h = tableFrameHeightPt(c, fit);
    // The engine needs 121.44 pt (bisected against the real engine with
    // Inter, which leaves the lead-in line); the IR alone said 105 and the
    // last rows were overset.
    expect(totalHeightPt(c)).toBe(105);
    const leadIn = TABLE_LEAD_IN_LINE ? 16.425 : 0;
    expect(h).toBeCloseTo(105 + leadIn + FIT_ALLOWANCE_PT, 6);
    if (TABLE_LEAD_IN_LINE) {
      expect(h).toBeGreaterThanOrEqual(121.44);
      expect(h).toBeLessThanOrEqual(121.44 + 1);
    }
  });

  it("a row grows to its tallest cell's first baseline, never shrinks below the workbook's", () => {
    const c = textRegion([15, 15, 30], ["small", "big", "tall row"]);
    const fit = {
      defaultAscenderPt: 9.6,
      cellAscenderPt: (r: number) => (r === 1 ? 21 : 9.6),
    };
    expect(tableRowHeightsPt(c, fit)).toEqual([15, 21, 30]);
  });

  it("an empty cell asks for nothing", () => {
    const c = textRegion([15], [""]);
    const fit = { defaultAscenderPt: 9.6, cellAscenderPt: () => 40 };
    expect(tableRowHeightsPt(c, fit)).toEqual([15]);
  });

  it("an empty region needs no frame", () => {
    expect(tableFrameHeightPt(region([], []), { defaultAscenderPt: 9.6 })).toBe(0);
  });

  it("defaultPlacement sizes to the fit when given one, clamped like any placement", () => {
    const c = textRegion([15, 15], ["a", "b"]);
    const fit = { defaultAscenderPt: 9.6 };
    const [top, , bottom] = defaultPlacement("Page/u1", c, fit).bounds;
    expect(bottom - top).toBeCloseTo(30 + tableLeadInPt(fit) + FIT_ALLOWANCE_PT, 6);
    const tall = textRegion(Array.from({ length: 100 }, () => 15), []);
    const [t2, , b2] = defaultPlacement("Page/u1", tall, fit).bounds;
    expect(b2 - t2).toBe(MAX_HEIGHT_PT);
  });
});
