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

import { describe, expect, it } from "vitest";

import type { LoweredContent } from "../src";
import {
  NO_CHARACTER_STYLE,
  cellCharacterStyleApplies,
  cellCharacterStyleMints,
  cellsNeedingRestyle,
} from "../src";

const base = { italic: false, borderTop: false, borderRight: false, borderBottom: false, borderLeft: false };
function content(boldHeader: boolean, body = "3"): LoweredContent {
  return {
    cols: [{ index: 0, widthPt: 50 }],
    rows: [
      { index: 0, heightPt: 18, cells: [{ col: 0, text: "Qty", align: "left", styleKey: 1 }] },
      { index: 1, heightPt: 18, cells: [{ col: 0, text: body, align: "right", styleKey: 0 }] },
    ],
    rules: { h: [], v: [] },
    merges: [],
    styles: [
      { key: 0, bold: false, ...base },
      { key: 1, bold: boldHeader, fontSizePt: boldHeader ? 14 : null, ...base },
    ],
  };
}

describe("native-table cell text formatting [sheet.lower.page]", () => {
  it("mints one character style per distinct format, once", () => {
    const mints = cellCharacterStyleMints(content(true), new Set());
    expect(mints[0].op).toBe("createCharacterStyle");
    const id = (mints[0].args as { selfId: string }).selfId;
    expect(mints.filter((m) => m.op === "setStyleProperty")).toHaveLength(2);
    expect(cellCharacterStyleMints(content(true), new Set([id]))).toEqual([]);
    // A failed read mints nothing (a duplicate create would fail the batch).
    expect(cellCharacterStyleMints(content(true), null)).toEqual([]);
  });

  it("applies it to the cell's whole text, cell-qualified", () => {
    const applies = cellCharacterStyleApplies(content(true), "S", "T");
    expect(applies).toHaveLength(1);
    expect(applies[0]).toMatchObject({
      op: "applyStyle",
      args: { storyId: "S", start: 0, end: 3, scope: "character", cell: { tableId: "T", row: 0, col: 0 } },
    });
  });

  it("a refresh restyles exactly the cells whose text or format changed, resetting lost formats", () => {
    const changed = cellsNeedingRestyle(content(true), content(false, "4"));
    expect([...changed].sort()).toEqual(["0:0", "1:0"]);
    const ops = cellCharacterStyleApplies(content(false, "4"), "S", "T", {
      cells: changed,
      includeDefault: true,
    });
    expect(ops.map((o) => (o.args as { style: string }).style)).toEqual([
      NO_CHARACTER_STYLE,
      NO_CHARACTER_STYLE,
    ]);
  });
});
