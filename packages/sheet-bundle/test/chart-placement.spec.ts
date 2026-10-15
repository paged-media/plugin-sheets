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

// Where a new chart goes on its page (`chartBoxOnPage`), as InDesign places
// new content: inside the margins, beside the selection (below, above,
// right, left — in that order), shrunk only when the page cannot hold it.
// Before, it went below the selection regardless and hung off a page whose
// selection sat near the foot (the real-engine case is in
// lower-chart-fit.spec.ts).

import { describe, expect, it } from "vitest";

import { chartBoxOnPage, type ChartPage } from "../src/lower-chart";

// A Letter page with 36 pt margins: the margin box is 36..576 × 36..756.
const LETTER: ChartPage = { width: 612, height: 792, margins: [36, 36, 36, 36] };
const sel = (top: number, left: number, bottom: number, right: number) => ({ top, left, bottom, right });

describe("chartBoxOnPage [sheet.chart.engine]", () => {
  it("below the selection when there is room, at the chart's size", () => {
    expect(chartBoxOnPage(LETTER, sel(100, 60, 200, 300))).toEqual({
      top: 212,
      left: 60,
      width: 360,
      height: 240,
    });
  });

  it("above it when the foot of the page has no room", () => {
    const at = chartBoxOnPage(LETTER, sel(600, 60, 700, 300));
    expect(at).toEqual({ top: 600 - 12 - 240, left: 60, width: 360, height: 240 });
  });

  it("slid inside the right margin when the selection sits far right", () => {
    const at = chartBoxOnPage(LETTER, sel(100, 400, 200, 560));
    expect(at.left + at.width).toBe(576);
    expect(at.top).toBe(212);
  });

  it("beside it when neither below nor above has room", () => {
    // A tall narrow selection on the left: right of it, top kept in the margins.
    const right = chartBoxOnPage(LETTER, sel(36, 36, 756, 150));
    expect(right).toEqual({ top: 36, left: 162, width: 360, height: 240 });
    // …on the right: left of it.
    const left = chartBoxOnPage(LETTER, sel(36, 450, 756, 576));
    expect(left).toEqual({ top: 36, left: 450 - 12 - 360, width: 360, height: 240 });
  });

  it("inside the margins when no side has room, never off the page", () => {
    const at = chartBoxOnPage(LETTER, sel(36, 36, 756, 576));
    expect(at.top + at.height).toBeLessThanOrEqual(756);
    expect(at.left).toBeGreaterThanOrEqual(36);
    expect(at.left + at.width).toBeLessThanOrEqual(576);
  });

  it("shrunk proportionally only when the margin box cannot hold 360 × 240", () => {
    const small: ChartPage = { width: 300, height: 400, margins: [20, 20, 20, 20] };
    const at = chartBoxOnPage(small, null);
    expect(at.width).toBeCloseTo(260);
    expect(at.height).toBeCloseTo(260 * (240 / 360));
    expect([at.top, at.left]).toEqual([20, 20]);
    expect(chartBoxOnPage(LETTER, null)).toEqual({ top: 36, left: 36, width: 360, height: 240 });
  });

  it("uses the page's edges when the margins leave no room", () => {
    const tight: ChartPage = { width: 400, height: 300, margins: [200, 0, 200, 0] };
    const at = chartBoxOnPage(tight, null);
    expect([at.top, at.left]).toEqual([0, 0]);
    expect(at.top + at.height).toBeLessThanOrEqual(300);
  });
});
