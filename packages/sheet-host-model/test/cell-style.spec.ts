// sheet.plugin.cell-style.from-cell (host-model half): the PURE plan that
// turns a cell's read properties into createCellStyle + setStyleProperty
// mutations. ZERO spreadsheet semantics — it only re-shapes already-read
// document properties into style-collection writes, and OFFERS the apply-back
// op the caller attempts separately (wire-shape-only residual).

import { describe, expect, it } from "vitest";

import type { ElementId, PropertyPath, Value } from "@paged-media/plugin-api";

import {
  isCellStylePath,
  planCellStyleFromEntries,
  type ReadEntry,
} from "../src";

const FILL: Value = { type: "colorRef", value: "FF8800" };
const TOP_W: Value = { type: "length", value: 0.5 };

const ENTRIES: ReadEntry[] = [
  { path: "cellFillColor", value: FILL },
  { path: "cellTopEdgeStrokeWeight", value: TOP_W },
  // A non-cell path — must be ignored (not a cell-style property).
  { path: "frameFillColor", value: { type: "colorRef", value: "000000" } },
  // An indeterminate read (value null) — must be skipped, never baked.
  { path: "cellBottomEdgeStrokeWeight", value: null },
];

describe("sheet_plugin_cell_style_from_cell: path filter", () => {
  it("recognises cell-appearance paths, rejects frame/text paths", () => {
    expect(isCellStylePath("cellFillColor")).toBe(true);
    expect(isCellStylePath("cellRightEdgeStrokeColor")).toBe(true);
    expect(isCellStylePath("frameFillColor" as PropertyPath)).toBe(false);
    expect(isCellStylePath("characterFontSize" as PropertyPath)).toBe(false);
  });
});

describe("sheet_plugin_cell_style_from_cell: mint + populate plan", () => {
  it("mints with our own selfId + the given name (null-createdId precedent)", () => {
    const plan = planCellStyleFromEntries("style.1", "Highlight", ENTRIES);
    expect(plan.createOp).toEqual({
      op: "createCellStyle",
      args: { selfId: "style.1", name: "Highlight" },
    });
    expect(plan.styleId).toBe("style.1");
  });

  it("populates ONLY the cell-appearance entries with concrete values", () => {
    const plan = planCellStyleFromEntries("style.1", "Highlight", ENTRIES);
    expect(plan.capturedPaths).toEqual([
      "cellFillColor",
      "cellTopEdgeStrokeWeight",
    ]);
    expect(plan.propertyOps).toEqual([
      {
        op: "setStyleProperty",
        args: { collection: "cell", styleId: "style.1", path: "cellFillColor", value: FILL },
      },
      {
        op: "setStyleProperty",
        args: {
          collection: "cell",
          styleId: "style.1",
          path: "cellTopEdgeStrokeWeight",
          value: TOP_W,
        },
      },
    ]);
  });

  it("an all-indeterminate / all-foreign read mints an empty-but-valid style", () => {
    const plan = planCellStyleFromEntries("style.2", "Blank", [
      { path: "frameFillColor", value: { type: "colorRef", value: "111" } },
      { path: "cellFillColor", value: null },
    ]);
    expect(plan.propertyOps).toEqual([]);
    expect(plan.capturedPaths).toEqual([]);
    // The mint still happens (a named empty style is valid).
    expect(plan.createOp.op).toBe("createCellStyle");
  });

  it("offers the apply-back op (appliedCellStyle) addressed at a tableCell", () => {
    const plan = planCellStyleFromEntries("style.1", "Highlight", ENTRIES);
    const cellId: ElementId = {
      kind: "tableCell",
      id: { story_id: "s1", table_id: "t1", row: 2, col: 3 },
    };
    expect(plan.applyOp(cellId)).toEqual({
      op: "setElementProperty",
      args: {
        elementId: cellId,
        path: "appliedCellStyle",
        value: { type: "text", value: "style.1" },
      },
    });
  });
});
