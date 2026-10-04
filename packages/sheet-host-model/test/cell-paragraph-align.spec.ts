// Wave 9 — horizontal cell alignment as paragraph styles on a native table
// (sheet.lower.page): the engine resolves each cell's alignment; these turn
// it into a paragraph-style mint + a cell-qualified paragraph applyStyle.

import { describe, expect, it } from "vitest";

import type { Mutation } from "@paged-media/plugin-api";

import {
  ALIGN_PARAGRAPH_STYLES,
  NO_PARAGRAPH_STYLE,
  cellAlignApplies,
  cellAlignStyleMints,
  cellsNeedingRealign,
  needsAlignStyles,
  type LoweredContent,
} from "../src";

const content = (cells: [string, "general" | "left" | "center" | "right"][]): LoweredContent => ({
  cols: cells.map((_, i) => ({ index: i, widthPt: 40 })),
  rows: [{ index: 0, heightPt: 15, cells: cells.map(([text, align], col) => ({ col, text, align })) }],
  rules: { h: [], v: [] },
  merges: [],
});

type Loose = { op: string; args: Record<string, unknown> };
const loose = (ops: Mutation[]) => ops as unknown as Loose[];
const RIGHT = ALIGN_PARAGRAPH_STYLES.right!.id;
const CENTER = ALIGN_PARAGRAPH_STYLES.center!.id;

describe("cell alignment → paragraph styles [sheet.lower.page]", () => {
  it("left and general cells need nothing", () => {
    const c = content([["a", "left"], ["b", "general"]]);
    expect(needsAlignStyles(c)).toBe(false);
    expect(cellAlignStyleMints(c, new Set())).toEqual([]);
    expect(cellAlignApplies(c, "S", "t")).toEqual([]);
  });

  it("mints only the used, absent styles; nothing on a failed read", () => {
    const c = content([["1", "right"], ["x", "center"], ["2", "right"]]);
    const ops = loose(cellAlignStyleMints(c, new Set([CENTER])));
    expect(ops.map((o) => o.op)).toEqual(["createParagraphStyle", "setStyleProperty"]);
    expect(ops[0].args.selfId).toBe(RIGHT);
    expect(ops[1].args).toMatchObject({
      collection: "paragraph",
      path: "paragraphJustification",
      value: { type: "text", value: "RightAlign" },
    });
    expect(cellAlignStyleMints(c, null)).toEqual([]);
  });

  it("applies a paragraph-scope style to each aligned cell, skipping unavailable styles", () => {
    const c = content([["1", "right"], ["x", "left"], ["yes", "center"]]);
    const ops = loose(cellAlignApplies(c, "S", "t"));
    expect(ops.map((o) => [o.args.style, o.args.scope, o.args.cell, o.args.end])).toEqual([
      [RIGHT, "paragraph", { tableId: "t", row: 0, col: 0 }, 1],
      [CENTER, "paragraph", { tableId: "t", row: 0, col: 2 }, 3],
    ]);
    expect(cellAlignApplies(c, "S", "t", { available: new Set() })).toEqual([]);
    const all = loose(cellAlignApplies(c, "S", "t", { includeDefault: true }));
    expect(all.map((o) => o.args.style)).toEqual([RIGHT, NO_PARAGRAPH_STYLE, CENTER]);
  });

  it("a refresh re-aligns the cells whose alignment changed", () => {
    const prev = content([["x", "left"], ["1", "right"], ["2", "right"]]);
    const next = content([["7", "right"], ["y", "left"], ["2", "right"]]);
    expect([...cellsNeedingRealign(prev, next)].sort()).toEqual(["0:0", "0:1"]);
  });
});
