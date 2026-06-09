// sheet.plugin.lower.mutations (native tables, S-03 RESOLVED): the pure
// emitters turn the lowered IR into the insertTable op + the per-cell
// insertText batch. No host, no spreadsheet semantics — just the wire
// vocabulary over already-decided geometry/text.

import { describe, expect, it } from "vitest";

import type { LoweredContent } from "../src";
import { columnOrder, tableCellOps, tableInsertOp } from "../src";

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
