// sheet.plugin.lower.mutations (native tables, S-03 RESOLVED): the pure
// emitters turn the lowered IR into the insertTable op + the per-cell
// insertText batch. No host, no spreadsheet semantics — just the wire
// vocabulary over already-decided geometry/text.

import { describe, expect, it } from "vitest";

import type { LoweredContent, Page } from "../src";
import {
  columnOrder,
  pageTableMutations,
  tableCellOps,
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
