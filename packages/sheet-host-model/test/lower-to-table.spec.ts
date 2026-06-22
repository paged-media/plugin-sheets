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

// sheet.lower.native-table / sheet.plugin.lower.mutations (native tables,
// S-03 RESOLVED): the pure emitters turn the lowered IR into the
// insertTable op + the per-cell insertText pour + the decor batch
// (setCellSpan per merge, tableCell-scoped cellFillColor /
// cell*EdgeStrokeWeight for style fills/borders and grid rules). No host,
// no spreadsheet semantics — just the wire vocabulary over
// already-decided geometry/text.

import { describe, expect, it } from "vitest";

import type { Mutation } from "@paged-media/plugin-api";

import type { LoweredContent, Page } from "../src";
import {
  CELL_EDGE_STROKE_PT,
  columnOrder,
  pageTableMutations,
  tableCellOps,
  tableContentBatch,
  tableDecorOps,
  tableInsertOp,
} from "../src";

const CONTENT: LoweredContent = {
  cols: [
    { index: 0, widthPt: 50 },
    { index: 2, widthPt: 60 }, // sparse: model col 2, table col 1
  ],
  rows: [
    {
      index: 0,
      heightPt: 18,
      cells: [
        { col: 0, text: "Item", align: "left" },
        { col: 2, text: "Qty", align: "right" },
      ],
    },
    {
      index: 1,
      heightPt: 20,
      cells: [
        { col: 0, text: "Apple", align: "left" },
        // model col 2 empty in this row
      ],
    },
  ],
  rules: { h: [], v: [] },
  merges: [],
};

describe("native table emitters", () => {
  it("columnOrder maps sparse model columns to table positions", () => {
    expect(columnOrder(CONTENT)).toEqual([0, 2]);
  });

  it("tableInsertOp sizes the table from the IR + caller widths", () => {
    const op = tableInsertOp(CONTENT, "Story/u1", [44, 55]);
    expect(op.op).toBe("insertTable");
    const a = (op as { args: Record<string, unknown> }).args;
    expect(a.storyId).toBe("Story/u1");
    expect(a.rows).toBe(2);
    expect(a.cols).toBe(2);
    expect(a.columnWidths).toEqual([44, 55]);
    expect(a.rowHeights).toEqual([18, 20]);
    expect(a.headerRows).toBe(0);
  });

  it("tableCellOps pours non-empty cells at their table row/col", () => {
    const batch = tableCellOps(CONTENT, "Story/u1", "tbl9");
    expect(batch.op).toBe("batch");
    const ops = (batch as { args: { ops: Array<{ op: string; args: { text: string; cell: unknown } }> } })
      .args.ops;
    // 3 non-empty cells (Item, Qty, Apple); the empty col-2/row-1 is skipped.
    expect(ops).toHaveLength(3);
    expect(ops.every((o) => o.op === "insertText")).toBe(true);
    const item = ops.find((o) => o.args.text === "Item")!;
    expect(item.args.cell).toEqual({ tableId: "tbl9", row: 0, col: 0 });
    const qty = ops.find((o) => o.args.text === "Qty")!;
    expect(qty.args.cell).toEqual({ tableId: "tbl9", row: 0, col: 1 }); // sparse col 2 → table col 1
    const apple = ops.find((o) => o.args.text === "Apple")!;
    expect(apple.args.cell).toEqual({ tableId: "tbl9", row: 1, col: 0 });
  });
});

// ── per-page table translation (Wave 2D, S-05) ──────────────────────────────

/** Two paginated pages over CONTENT-shaped slices: page 0 the header row,
 *  page 1 the body row (re-based to 0 within its frame). */
const PAGE0: Page = {
  frameIndex: 0,
  content: {
    cols: [
      { index: 0, widthPt: 50 },
      { index: 2, widthPt: 60 },
    ],
    rows: [
      {
        index: 0,
        heightPt: 18,
        cells: [
          { col: 0, text: "Item", align: "left" },
          { col: 2, text: "Qty", align: "right" },
        ],
      },
    ],
    rules: { h: [], v: [] },
    merges: [],
  },
  continued: true,
  oversize: false,
};

const PAGE1: Page = {
  frameIndex: 1,
  content: {
    cols: [
      { index: 0, widthPt: 50 },
      { index: 2, widthPt: 60 },
    ],
    rows: [
      {
        index: 0,
        heightPt: 20,
        cells: [{ col: 0, text: "Apple", align: "left" }],
      },
    ],
    rules: { h: [], v: [] },
    merges: [],
  },
  continued: false,
  oversize: false,
};

describe("sheet_lower_paginate per-page table translation", () => {
  it("pageTableMutations emits insert + cells over THIS page's content", () => {
    const ops = pageTableMutations(PAGE0, "Story/p0", [44, 55]);

    // The insert is sized from page 0's own content (1 row, 2 cols).
    expect(ops.insert.op).toBe("insertTable");
    const a = (ops.insert as { args: Record<string, unknown> }).args;
    expect(a.storyId).toBe("Story/p0");
    expect(a.rows).toBe(1);
    expect(a.cols).toBe(2);
    expect(a.columnWidths).toEqual([44, 55]);
    expect(a.rowHeights).toEqual([18]);

    // The cells batch is built lazily once the tableId is known.
    const cells = ops.cells("tblP0");
    expect(cells.op).toBe("batch");
    const cellOps = (cells as {
      args: { ops: Array<{ args: { text: string; cell: unknown } }> };
    }).args.ops;
    expect(cellOps).toHaveLength(2); // Item, Qty
    expect(cellOps.find((o) => o.args.text === "Item")!.args.cell).toEqual({
      tableId: "tblP0",
      row: 0,
      col: 0,
    });
    expect(cellOps.find((o) => o.args.text === "Qty")!.args.cell).toEqual({
      tableId: "tblP0",
      row: 0,
      col: 1, // sparse model col 2 → table col 1
    });
  });

  it("each page translates to its OWN frame's table (re-based rows)", () => {
    const p0 = pageTableMutations(PAGE0, "Story/p0", [50, 60]);
    const p1 = pageTableMutations(PAGE1, "Story/p1", [50, 60]);

    // Page 1's table is in its own story, sized from its own (1-row) content;
    // the body row is re-based to row 0 within the frame (not the global row).
    const a1 = (p1.insert as { args: Record<string, unknown> }).args;
    expect(a1.storyId).toBe("Story/p1");
    expect(a1.rows).toBe(1);
    expect(a1.rowHeights).toEqual([20]);

    const apple = (p1.cells("tblP1") as {
      args: { ops: Array<{ args: { text: string; cell: unknown } }> };
    }).args.ops.find((o) => o.args.text === "Apple")!;
    expect(apple.args.cell).toEqual({ tableId: "tblP1", row: 0, col: 0 });

    // Page 0's table is a distinct op in a distinct story (no cross-frame leak).
    expect(
      (p0.insert as { args: { storyId: string } }).args.storyId,
    ).toBe("Story/p0");
  });
});

// ── decor: merges, fills, edge strokes (sheet.lower.native-table) ───────────

/** Unwrap a batch's ops. */
function opsOf(batch: Mutation): Array<{ op: string; args: any }> {
  expect(batch.op).toBe("batch");
  return (batch as { args: { ops: Array<{ op: string; args: any }> } }).args
    .ops;
}

/** Loosen a Mutation[] for shape assertions across the op union. */
function loose(ops: Mutation[]): Array<{ op: string; args: any }> {
  return ops as unknown as Array<{ op: string; args: any }>;
}

describe("sheet_lower_native_table: merges → setCellSpan", () => {
  it("emits one setCellSpan per merged span at its table position", () => {
    const content: LoweredContent = {
      ...CONTENT,
      merges: [{ row: 0, col: 0, rowSpan: 2, colSpan: 1 }],
    };
    const { ops } = tableDecorOps(content, "Story/u1", "tbl9");
    expect(ops).toEqual([
      {
        op: "setCellSpan",
        args: {
          storyId: "Story/u1",
          tableId: "tbl9",
          row: 0,
          col: 0,
          rowSpan: 2,
          columnSpan: 1,
        },
      },
    ]);
  });

  it("maps a sparse-column span to LOWERED column counts", () => {
    // Model cols 0..2 merged, but only model cols 0 and 2 are lowered —
    // the table span covers 2 table columns, not 3 model columns.
    const content: LoweredContent = {
      ...CONTENT,
      merges: [{ row: 0, col: 0, rowSpan: 1, colSpan: 3 }],
    };
    const { ops } = tableDecorOps(content, "Story/u1", "tbl9");
    expect(ops).toHaveLength(1);
    expect(ops[0].op).toBe("setCellSpan");
    const args = (ops[0] as { args: any }).args;
    expect(args.row).toBe(0);
    expect(args.col).toBe(0);
    expect(args.rowSpan).toBe(1);
    expect(args.columnSpan).toBe(2); // sparse: model cols {0,2} only
  });

  it("skips a span anchored outside the lowered grid (defensive)", () => {
    const content: LoweredContent = {
      ...CONTENT,
      merges: [{ row: 9, col: 9, rowSpan: 2, colSpan: 2 }],
    };
    expect(tableDecorOps(content, "Story/u1", "tbl9").ops).toEqual([]);
  });
});

describe("sheet_lower_native_table: style fills + borders → tableCell props", () => {
  const STYLED: LoweredContent = {
    ...CONTENT,
    rows: [
      {
        index: 0,
        heightPt: 18,
        cells: [
          { col: 0, text: "Item", align: "left", styleKey: 1 },
          { col: 2, text: "Qty", align: "right", styleKey: 0 },
        ],
      },
      { index: 1, heightPt: 20, cells: [{ col: 0, text: "Apple", align: "left" }] },
    ],
    styles: [
      {
        key: 0,
        bold: false,
        italic: false,
        borderTop: false,
        borderRight: false,
        borderBottom: false,
        borderLeft: false,
      },
      {
        key: 1,
        bold: true,
        italic: false,
        fillRgb: "#FFEE00",
        borderTop: true,
        borderRight: false,
        borderBottom: true,
        borderLeft: false,
      },
    ],
  };

  it("a styled cell gets cellFillColor + per-edge stroke weights", () => {
    const decor = tableDecorOps(STYLED, "Story/u1", "tbl9");
    expect(decor.unmappedRules).toBe(0);
    const ops = loose(decor.ops);

    const fill = ops.find(
      (o) => o.op === "setElementProperty" && o.args.path === "cellFillColor",
    )!;
    expect(fill.args.elementId).toEqual({
      kind: "tableCell",
      id: { story_id: "Story/u1", table_id: "tbl9", row: 0, col: 0 },
    });
    expect(fill.args.value).toEqual({ type: "colorRef", value: "#FFEE00" });

    const edgePaths = ops
      .filter((o) => o.op === "setElementProperty" && o.args.path !== "cellFillColor")
      .map((o) => o.args.path)
      .sort();
    expect(edgePaths).toEqual([
      "cellBottomEdgeStrokeWeight",
      "cellTopEdgeStrokeWeight",
    ]);
    for (const o of ops) {
      if (o.args.path === "cellFillColor") continue;
      expect(o.args.value).toEqual({ type: "length", value: CELL_EDGE_STROKE_PT });
      expect(o.args.elementId.id).toMatchObject({ row: 0, col: 0 });
    }
  });

  it("default-styled cells emit no decor", () => {
    expect(tableDecorOps(CONTENT, "Story/u1", "tbl9").ops).toEqual([]);
  });
});

describe("sheet_lower_native_table: grid rules → cell edge strokes", () => {
  // 2 rows (18, 20) × 2 lowered cols (50, 60): row boundaries 0/18/38,
  // col boundaries 0/50/110.
  const RULED: LoweredContent = {
    ...CONTENT,
    rules: {
      h: [
        { at: 0, from: 0, to: 110 },
        { at: 18, from: 0, to: 110 },
        { at: 38, from: 0, to: 110 },
      ],
      v: [
        { at: 0, from: 0, to: 38 },
        { at: 50, from: 0, to: 38 },
        { at: 110, from: 0, to: 38 },
      ],
    },
  };

  it("maps boundary rules to top/bottom/left/right edges of boundary cells", () => {
    const decor = tableDecorOps(RULED, "Story/u1", "tbl9");
    expect(decor.unmappedRules).toBe(0);
    const ops = loose(decor.ops);

    const byPath = new Map<string, Array<{ row: number; col: number }>>();
    for (const o of ops) {
      expect(o.op).toBe("setElementProperty");
      expect(o.args.value).toEqual({ type: "length", value: CELL_EDGE_STROKE_PT });
      const cells = byPath.get(o.args.path) ?? [];
      cells.push({ row: o.args.elementId.id.row, col: o.args.elementId.id.col });
      byPath.set(o.args.path, cells);
    }

    // at 0 → top edge of row 0, both columns.
    expect(byPath.get("cellTopEdgeStrokeWeight")).toEqual([
      { row: 0, col: 0 },
      { row: 0, col: 1 },
    ]);
    // at 18 → bottom of row 0; at 38 → bottom of row 1.
    expect(byPath.get("cellBottomEdgeStrokeWeight")).toEqual([
      { row: 0, col: 0 },
      { row: 0, col: 1 },
      { row: 1, col: 0 },
      { row: 1, col: 1 },
    ]);
    // v at 0 → left of col 0 (both rows).
    expect(byPath.get("cellLeftEdgeStrokeWeight")).toEqual([
      { row: 0, col: 0 },
      { row: 1, col: 0 },
    ]);
    // v at 50 → right of col 0; v at 110 → right of col 1.
    expect(byPath.get("cellRightEdgeStrokeWeight")).toEqual([
      { row: 0, col: 0 },
      { row: 1, col: 0 },
      { row: 0, col: 1 },
      { row: 1, col: 1 },
    ]);
  });

  it("a rule aligning to NO boundary is counted, never guessed", () => {
    const content: LoweredContent = {
      ...CONTENT,
      rules: { h: [{ at: 25, from: 0, to: 110 }], v: [] },
    };
    const { ops, unmappedRules } = tableDecorOps(content, "Story/u1", "tbl9");
    expect(ops).toEqual([]);
    expect(unmappedRules).toBe(1);
  });

  it("a partial-extent rule only strokes the covered columns", () => {
    const content: LoweredContent = {
      ...CONTENT,
      rules: { h: [{ at: 18, from: 0, to: 50 }], v: [] },
    };
    const ops = loose(tableDecorOps(content, "Story/u1", "tbl9").ops);
    expect(ops).toHaveLength(1); // only table col 0 (extent 0..50)
    expect(ops[0].args.elementId.id).toMatchObject({ row: 0, col: 0 });
    expect(ops[0].args.path).toBe("cellBottomEdgeStrokeWeight");
  });
});

describe("sheet_lower_native_table: tableContentBatch (pour + decor, one step)", () => {
  it("pours text FIRST, then spans, then cell properties", () => {
    const content: LoweredContent = {
      ...CONTENT,
      merges: [{ row: 0, col: 0, rowSpan: 2, colSpan: 1 }],
      rules: { h: [{ at: 38, from: 0, to: 110 }], v: [] },
    };
    const { batch, unmappedRules } = tableContentBatch(
      content,
      "Story/u1",
      "tbl9",
    );
    expect(unmappedRules).toBe(0);
    const ops = opsOf(batch);
    const kinds = ops.map((o) => o.op);
    // 3 pours, 1 span, 2 edge ops — in that phase order.
    expect(kinds).toEqual([
      "insertText",
      "insertText",
      "insertText",
      "setCellSpan",
      "setElementProperty",
      "setElementProperty",
    ]);
  });

  it("pageTableMutations.cells carries the page's decor too", () => {
    const page: Page = {
      ...PAGE0,
      content: {
        ...PAGE0.content,
        merges: [{ row: 0, col: 0, rowSpan: 1, colSpan: 3 }],
      },
    };
    const ops = opsOf(pageTableMutations(page, "Story/p0", [50, 60]).cells("tblP0"));
    expect(ops.some((o) => o.op === "setCellSpan")).toBe(true);
  });
});
