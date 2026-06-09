// sheet.plugin.lower.mutations (the bundle-side flow): the two-phase
// page lower (S-03) drives the host writes in order — phase 1 the batch
// (frame + rules + binding), phase 2 the insertText into the story the
// host's hitTest resolves. A fake host captures the mutate calls; a fake
// engine returns a small LoweredContent.

import { describe, expect, it } from "vitest";

import type {
  BundleHost,
  ElementId,
  Mutation,
  MutationOutcome,
} from "@paged-media/plugin-api";

import {
  lowerChartToFrame,
  lowerSelectionToFrame,
  type SheetEngine,
} from "../src";

// A fake engine: returns a fixed 2x1 region + one sheet.
function fakeEngine(): SheetEngine {
  return {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: () => ({ changed: [] }),
    getCellDisplay: () => "",
    getRangeLowered: () => ({
      cols: [
        { index: 0, widthPt: 50 },
        { index: 1, widthPt: 50 },
      ],
      rows: [
        {
          index: 0,
          heightPt: 18,
          cells: [
            { col: 0, text: "Item", align: "left" },
            { col: 1, text: "Qty", align: "right" },
          ],
        },
      ],
      rules: { h: [{ at: 18, from: 0, to: 100 }], v: [] },
      merges: [],
    }),
    getGridScene: () => ({
      viewport: { firstRow: 0, firstCol: 0, rows: 0, cols: 0, xOffsets: [0], yOffsets: [0] },
      cells: [],
      styles: [],
      gridlines: { h: [], v: [] },
      selection: null,
    }),
    setGridSelection() {},
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 1, cols: 2 }],
    listCharts: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
}

// A fake host capturing every mutate; createdId is a textFrame; hitTest
// returns a known storyId so phase 2 can address the story.
function fakeHost(createdId: ElementId, storyId: string | null) {
  const mutations: Mutation[] = [];
  const selections: ElementId[][] = [];
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    document: {
      async meta() {
        return { activePage: "Page/u1" } as never;
      },
      async collection() {
        return [] as never;
      },
      async mutate(m: Mutation): Promise<MutationOutcome> {
        mutations.push(m);
        // A batch that creates a frame mints the frame id; a native
        // insertTable mints the table id; everything else (the cell
        // batch, the text pour) creates nothing.
        if (m.op === "batch") {
          const ops = (m as { args: { ops: Array<{ op: string }> } }).args.ops;
          if (ops.some((o) => o.op === "insertTextFrame")) {
            return { applied: true, createdId, pageIds: ["Page/u1"] };
          }
          return { applied: true, createdId: null, pageIds: ["Page/u1"] };
        }
        if (m.op === "insertTable") {
          return {
            applied: true,
            createdId: { kind: "textFrame", id: "table1" } as ElementId,
            pageIds: ["Page/u1"],
          };
        }
        return { applied: true, createdId: null, pageIds: ["Page/u1"] };
      },
      async hitTest() {
        return storyId
          ? ({ storyId, frameId: "frame1" } as never)
          : null;
      },
    },
    text: {
      async measureString() {
        return { advance: 30, ascender: 9, descender: -2 };
      },
    },
    selection: {
      async set(ids: ElementId[]) {
        selections.push(ids);
        return ids;
      },
    },
  } as unknown as BundleHost;
  return { host, mutations, selections };
}

const CREATED: ElementId = { kind: "textFrame", id: "frame1" };

describe("sheet_plugin_lower_mutations: native-table host flow", () => {
  it("phase 1 frame batch → phase 2 insertTable → phase 3 cell text", async () => {
    const { host, mutations, selections } = fakeHost(CREATED, "Story/u9");
    const id = await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1");

    expect(id).toBe("frame1");
    expect(mutations).toHaveLength(3);
    // Phase 1 — frame + binding (NO drawn rules: the table draws borders).
    expect(mutations[0].op).toBe("batch");
    const ops = (mutations[0] as { args: { ops: Array<{ op: string }> } }).args
      .ops;
    expect(ops[0].op).toBe("insertTextFrame");
    expect(ops.some((o) => o.op === "insertLine")).toBe(false);
    expect(ops.some((o) => o.op === "setPluginMetadata")).toBe(true);
    // Phase 2 — native table in the resolved story, font-metric widths.
    expect(mutations[1].op).toBe("insertTable");
    const tbl = mutations[1] as {
      args: { storyId: string; rows: number; cols: number; columnWidths: number[] };
    };
    expect(tbl.args.storyId).toBe("Story/u9");
    expect(tbl.args.rows).toBe(1);
    expect(tbl.args.cols).toBe(2);
    expect(tbl.args.columnWidths).toHaveLength(2);
    expect(tbl.args.columnWidths[0]).toBeGreaterThan(0); // measured, not 0
    // Phase 3 — each cell's text poured into its table cell.
    expect(mutations[2].op).toBe("batch");
    const cellOps = (mutations[2] as {
      args: { ops: Array<{ op: string; args: { text?: string; cell?: unknown } }> };
    }).args.ops;
    const item = cellOps.find((o) => o.args.text === "Item");
    expect(item?.op).toBe("insertText");
    expect(item?.args.cell).toEqual({ tableId: "table1", row: 0, col: 0 });
    expect(cellOps.find((o) => o.args.text === "Qty")?.args.cell).toEqual({
      tableId: "table1",
      row: 0,
      col: 1,
    });
    // The new frame is selected.
    expect(selections).toEqual([[CREATED]]);
  });

  it("phase 1 carries the binding for this plugin's namespace", async () => {
    const { host, mutations } = fakeHost(CREATED, "Story/u9");
    await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1");
    const ops = (mutations[0] as { args: { ops: Array<{ op: string; args: unknown }> } })
      .args.ops;
    const meta = ops.find((o) => o.op === "setPluginMetadata") as {
      args: { key: string; value: string };
    };
    expect(meta.args.key).toBe("x-paged:media.paged.sheet");
    const binding = JSON.parse(meta.args.value);
    expect(binding.data.sheet).toBe("Sheet1");
    expect(binding.data.range).toBe("A1:B1");
  });

  it("skips the table (frame still placed) when the story can't be resolved", async () => {
    const { host, mutations } = fakeHost(CREATED, null);
    const id = await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1");
    expect(id).toBe("frame1"); // frame placed, honest about the gap
    expect(mutations.map((m) => m.op)).toEqual(["batch"]); // no insertTable
  });

  it("returns null when the phase-1 batch is rejected", async () => {
    const mutations: Mutation[] = [];
    const host = {
      log: { debug() {}, info() {}, warn() {}, error() {} },
      document: {
        async meta() {
          return { activePage: "Page/u1" } as never;
        },
        async collection() {
          return [] as never;
        },
        async mutate(m: Mutation): Promise<MutationOutcome> {
          mutations.push(m);
          return { applied: false, error: "nope" };
        },
        async hitTest() {
          return null;
        },
      },
      selection: { async set(ids: ElementId[]) { return ids; } },
    } as unknown as BundleHost;
    const id = await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1");
    expect(id).toBeNull();
    expect(mutations.map((m) => m.op)).toEqual(["batch"]);
  });
});

// ── chart → paged.draw vector lower (M2 charts track, spec §8.4) ────────────

/** A fake engine with one parsed chart + a fixed geometry IR (a column with
 *  one bar Rect and a title Text). */
function fakeChartEngine(): SheetEngine {
  const base = fakeEngine();
  return {
    ...base,
    listCharts: () => [
      { index: 0, hostSheet: 0, kind: "column", title: "Q1", seriesCount: 1 },
    ],
    getChartGeometry: () => ({
      widthPt: 200,
      heightPt: 150,
      prims: [
        { kind: "rect", x: 10, y: 20, w: 30, h: 80, fill: "#4E79A7", stroke: null, strokeW: 0 },
        { kind: "text", x: 100, y: 10, s: "Q1", sizePt: 10, anchor: "middle" },
      ],
    }),
  };
}

describe("sheet_chart_lower_paged_draw: bundle two-phase flow", () => {
  it("phase 1 emits the vector batch (insertPath) then pours each label", async () => {
    const { host, mutations, selections } = fakeHost(CREATED, "Story/u9");
    const ok = await lowerChartToFrame(host, fakeChartEngine(), 0);

    expect(ok).toBe(true);
    // Phase 1 — one batch with a colour swatch (the rect's #4E79A7 fill) +
    // insertPath (the rect) + its frameFillColor + insertTextFrame (the label)
    // + the binding metadata.
    expect(mutations[0].op).toBe("batch");
    const ops = (mutations[0] as {
      args: { ops: Array<{ op: string; args?: unknown }> };
    }).args.ops;
    expect(ops.some((o) => o.op === "insertPath")).toBe(true);
    expect(ops.some((o) => o.op === "insertTextFrame")).toBe(true);
    // The chart palette is now lowered: a createSwatch + a frameFillColor ref
    // (FINDING 1 — colours were previously dropped entirely).
    expect(ops.some((o) => o.op === "createSwatch")).toBe(true);
    expect(
      ops.some(
        (o) =>
          o.op === "setElementProperty" &&
          (o.args as { path: string }).path === "frameFillColor",
      ),
    ).toBe(true);
    // The binding rides the FIRST created element (so it sits after the rect's
    // style ops, not necessarily last in the batch).
    expect(ops.some((o) => o.op === "setPluginMetadata")).toBe(true);
    // Phase 2 — the label text poured into the resolved story.
    const pour = mutations.find((m) => m.op === "insertText") as {
      args: { text: string };
    };
    expect(pour.args.text).toBe("Q1");
    // The created element is selected.
    expect(selections).toEqual([[CREATED]]);
  });

  it("returns false for a chartless workbook (no engine charts)", async () => {
    const { host, mutations } = fakeHost(CREATED, "Story/u9");
    const empty: SheetEngine = {
      ...fakeEngine(),
      getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    };
    const ok = await lowerChartToFrame(host, empty, 0);
    expect(ok).toBe(false); // empty geometry => nothing lowered
    expect(mutations).toEqual([]);
  });
});
