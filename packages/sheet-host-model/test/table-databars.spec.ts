// Wave 9 — conditional-format data bars on the NATIVE placed table
// (sheet.xlsx.condfmt): IR bars mapped onto the table's measured columns,
// drawn as paths under the frame.

import { describe, expect, it } from "vitest";

import type { Mutation } from "@paged-media/plugin-api";

import { tableDataBarOps, tableDataBars, type LoweredContent } from "../src";

const content: LoweredContent = {
  cols: [
    { index: 0, widthPt: 50 },
    { index: 1, widthPt: 40 },
  ],
  rows: [
    { index: 0, heightPt: 15, cells: [{ col: 1, text: "10", align: "right" }] },
    { index: 2, heightPt: 20, cells: [{ col: 1, text: "5", align: "right" }] },
  ],
  rules: { h: [], v: [] },
  merges: [],
  databars: [
    { row: 0, col: 1, x: 51, y: 1, w: 38, h: 13, fillFraction: 1, fill: "#638EC6" },
    { row: 2, col: 1, x: 51, y: 16, w: 19, h: 18, fillFraction: 0.5, fill: "#638EC6" },
    { row: 1, col: 1, x: 51, y: 99, w: 10, h: 5, fillFraction: 0.2, fill: "#638EC6" }, // hidden row
  ],
};

type Loose = { op: string; args: Record<string, unknown> };
const loose = (ops: Mutation[]) => ops as unknown as Loose[];

describe("data bars on the native table [sheet.xlsx.condfmt]", () => {
  it("maps each bar onto its table cell, scaled by the measured width", () => {
    const bars = tableDataBars(content, [60, 80]); // column 1 doubled
    expect(bars).toHaveLength(2); // the hidden row's bar has no table row
    expect(bars[0]).toMatchObject({ x: 60 + 1 * 2, w: 76, y: 1, h: 13 });
    expect(bars[1]).toMatchObject({ x: 62, w: 38, y: 16 });
  });

  it("draws, fills and sends each bar under the frame; mints its colour once", () => {
    const ops = loose(tableDataBarOps(content, [60, 80], "Page/u1", [100, 20], new Set()));
    expect(ops.map((o) => o.op)).toEqual([
      "createSwatch",
      "insertPath",
      "bindCreated",
      "setElementProperty",
      "reorderElement",
      "insertPath",
      "bindCreated",
      "setElementProperty",
      "reorderElement",
    ]);
    const first = ops[1].args.anchors as { anchor: [number, number] }[];
    expect(first[0].anchor).toEqual([20 + 62, 100 + 1]);
    expect(ops[4].args).toEqual({ elementId: { kind: "polygon", id: "$h:bar0" }, to: "backward" });
    // A known colour is referenced, not re-minted; no bars, no ops.
    const known = new Set([String((ops[0].args.spec as { selfId: string }).selfId)]);
    expect(loose(tableDataBarOps(content, [60, 80], "Page/u1", [0, 0], known))[0].op).toBe("insertPath");
    expect(tableDataBarOps({ ...content, databars: [] }, [60, 80], "Page/u1", [0, 0])).toEqual([]);
  });
});
