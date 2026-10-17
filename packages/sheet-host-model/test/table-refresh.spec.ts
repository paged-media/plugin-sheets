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
  NO_FILL_SWATCH,
  cellAlignApplies,
  cellCharacterStyleApplies,
  offsetTableRows,
  tableRefreshOps,
  textOffsetLength,
} from "../src";

function content(
  texts: string[][],
  opts: { fillAt?: [number, number]; height?: number } = {},
): LoweredContent {
  return {
    cols: texts[0].map((_, i) => ({ index: i, widthPt: 50 })),
    rows: texts.map((row, r) => ({
      index: r,
      heightPt: opts.height ?? 18,
      cells: row.map((text, c) => ({
        col: c,
        text,
        align: "left" as const,
        styleKey: opts.fillAt && opts.fillAt[0] === r && opts.fillAt[1] === c ? 1 : 0,
      })),
    })),
    rules: { h: [], v: [] },
    merges: [],
    styles: [
      { key: 0, bold: false, italic: false, borderTop: false, borderRight: false, borderBottom: false, borderLeft: false },
      { key: 1, bold: false, italic: false, fillRgb: "#FFFF00", borderTop: false, borderRight: false, borderBottom: false, borderLeft: false },
    ],
  };
}

describe("placed-table refresh replaces in place [sheet.lower.page]", () => {
  it("re-pours ONLY the changed cell (no second table)", () => {
    const prev = content([["Item", "Qty"], ["Pens", "3"]]);
    const next = content([["Item", "Qty"], ["Pens", "4"]]);
    const ops = tableRefreshOps(prev, next, "S", "T", [50, 50], [50, 50]);
    expect(ops.structure).toEqual([]);
    expect(ops.text).toEqual([
      { op: "deleteRange", args: { storyId: "S", start: 0, end: 1, cell: { tableId: "T", row: 1, col: 1 } } },
      { op: "insertText", args: { storyId: "S", offset: 0, text: "4", cell: { tableId: "T", row: 1, col: 1 } } },
    ]);
    expect(JSON.stringify(ops)).not.toContain("insertTable\"");
  });

  it("reshapes rows at the tail and pours the new rows", () => {
    const prev = content([["a"], ["b"], ["c"]]);
    const fewer = tableRefreshOps(prev, content([["a"]]), "S", "T", [50], [50]);
    expect(fewer.structure.map((m) => m.op)).toEqual(["deleteTableRow", "deleteTableRow"]);
    expect(fewer.structure.map((m) => (m.args as { at: number }).at)).toEqual([2, 1]);
    expect(fewer.text).toEqual([]);

    const more = tableRefreshOps(content([["a"]]), prev, "S", "T", [50], [50]);
    expect(more.structure.filter((m) => m.op === "insertTableRow")).toHaveLength(2);
    expect(more.text.map((m) => m.op)).toEqual(["insertText", "insertText"]);
  });

  it("resets a fill the new content no longer has; widths/heights only when moved", () => {
    const prev = content([["x"]], { fillAt: [0, 0] });
    const next = content([["x"]], { height: 24 });
    const ops = tableRefreshOps(prev, next, "S", "T", [50], [60]);
    expect(ops.decor).toContainEqual({
      op: "setElementProperty",
      args: {
        elementId: { kind: "tableCell", id: { story_id: "S", table_id: "T", row: 0, col: 0 } },
        path: "cellFillColor",
        value: { type: "colorRef", value: NO_FILL_SWATCH },
      },
    });
    expect(ops.structure.map((m) => m.op).sort()).toEqual(["setColumnWidth", "setRowHeight"]);
  });

  it("a page split that moved rows is a row insert + delete, not a re-pour of every shifted cell [sheet.lower.paginate]", () => {
    // Frame 2 of a chain: header H, then body rows a,b,c. The frame above
    // got shorter, so row z moved down into this frame and c moved out.
    const prev = content([["H"], ["a"], ["b"], ["c"]]);
    const next = content([["H"], ["z"], ["a"], ["b"]]);
    const ops = tableRefreshOps(prev, next, "S", "T", [50], [50]);
    expect(ops.structure).toEqual([
      { op: "deleteTableRow", args: { storyId: "S", tableId: "T", at: 3 } },
      { op: "insertTableRow", args: { storyId: "S", tableId: "T", at: 1 } },
      { op: "setRowHeight", args: { storyId: "S", tableId: "T", row: 1, height: 18 } },
    ]);
    expect(ops.text).toEqual([
      { op: "insertText", args: { storyId: "S", offset: 0, text: "z", cell: { tableId: "T", row: 1, col: 0 } } },
    ]);
  });

  it("measures text offsets in UTF-8 bytes (core's story unit)", () => {
    expect(textOffsetLength("Süd")).toBe(4);
  });

  it("a host header band moves every row-addressed op below it, and nothing else [sheet.lower.page]", () => {
    // The workbook's row 0 is the table's row 2 under a two-row band.
    const prev = content([["Item", "Qty"], ["Pens", "3"]], { fillAt: [1, 1] });
    const next = content([["Item", "Qty"], ["Pens", "4"], ["Ink", "9"]], { fillAt: [0, 0], height: 20 });
    const lanes = tableRefreshOps(prev, next, "S", "T", [50, 50], [50, 60]);
    const styles = [
      ...cellCharacterStyleApplies(next, "S", "T", { includeDefault: true }),
      ...cellAlignApplies(next, "S", "T", { includeDefault: true }),
    ];
    const ops = [...lanes.structure, ...lanes.text, ...lanes.decor, ...styles];
    const moved = offsetTableRows(ops, "T", 2);
    expect(moved).toHaveLength(ops.length);
    const rowOf = (m: (typeof ops)[number]): number | null => {
      const a = m.args as Record<string, unknown>;
      if (m.op === "insertTableRow" || m.op === "deleteTableRow") return a.at as number;
      if (m.op === "setRowHeight" || m.op === "setCellSpan") return a.row as number;
      const cell = a.cell as { row: number } | undefined;
      if (cell) return cell.row;
      const el = a.elementId as { kind: string; id: { row: number } } | undefined;
      return el?.kind === "tableCell" ? el.id.row : null;
    };
    let rowOps = 0;
    ops.forEach((op, i) => {
      const r = rowOf(op);
      if (r === null) {
        expect(moved[i]).toEqual(op);
      } else {
        rowOps += 1;
        expect(rowOf(moved[i])).toBe(r + 2);
        expect({ ...moved[i], args: { ...moved[i].args, at: 0, row: 0, cell: 0, elementId: 0 } })
          .toEqual({ ...op, args: { ...op.args, at: 0, row: 0, cell: 0, elementId: 0 } });
      }
    });
    // Every lane that names a row is in the fixture: inserts, heights, text,
    // fills, styles; and the column widths (no row) are left alone.
    expect(new Set(ops.filter((op) => rowOf(op) !== null).map((op) => op.op))).toEqual(
      new Set(["insertTableRow", "setRowHeight", "deleteRange", "insertText", "setElementProperty", "applyStyle"]),
    );
    expect(rowOps).toBeGreaterThan(0);
    expect(moved.filter((op) => op.op === "setColumnWidth")).toEqual(
      ops.filter((op) => op.op === "setColumnWidth"),
    );
    // Another table's ops, and a zero offset, are untouched.
    expect(offsetTableRows(ops, "Other", 2)).toEqual(ops);
    expect(offsetTableRows(ops, "T", 0)).toEqual(ops);
    // A batch is walked.
    expect(offsetTableRows([{ op: "batch", args: { ops } }], "T", 2)).toEqual([
      { op: "batch", args: { ops: moved } },
    ]);
  });
});
