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

// sheet.plugin.lower.mutations / sheet.lower.native-table (the
// bundle-side flow): the NATIVE page lower drives the host writes in
// order — phase 1 the frame+binding batch, phase 2 the insertTable into
// the story the host's hitTest resolves, phase 3 the cell pour + decor
// (spans, fills, edge strokes) — with the tab-text lane retained as the
// explicit/runtime fallback. A fake host captures the mutate calls; a
// fake engine returns a small LoweredContent.

import { describe, expect, it } from "vitest";

import type {
  BundleHost,
  DocumentChangeEvent,
  ElementGeometryItem,
  ElementId,
  FrameChainLink,
  Mutation,
  MutationOutcome,
} from "@paged-media/plugin-api";
import type { Page } from "@paged-media/sheet-host-model";

import {
  DEFAULT_CELL_POINT_SIZE,
  lowerChartToFrame,
  lowerPaginatedToChain,
  lowerSelectionToFrame,
  subscribeChainReflow,
  type SheetEngine,
} from "../src";

// A fake engine: returns a fixed 2x1 region + one sheet.
function fakeEngine(): SheetEngine {
  return {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: () => ({ changed: [] }),
    getCellDisplay: () => "",
    getCellInput: () => "",
    sortRange: () => ({ changed: [], edits: [] }),
    findAll: () => [],
    replaceAll: () => ({ occurrences: 0, changed: [], edits: [], skipped: [] }),
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
    getRangeStyled: () => ({
      cols: [],
      rows: [],
      rules: { h: [], v: [] },
      merges: [],
    }),
    getRangeValues: () => [],
    paginate: () => [],
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
    chartKinds: () => ["column"],
    addChart: () => 0,
    listFreezePanes: () => [],
    listDataValidations: () => [],
    listComments: () => [],
    listFunctions: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
}

// A fake host capturing every mutate; createdId is a textFrame; the
// stories collection grows by one entry once the frame batch applies —
// the diff the lower flow resolves the new frame's story from (the
// hitTest door cannot see an EMPTY frame's story; verified live).
function fakeHost(createdId: ElementId, storyId: string | null) {
  const mutations: Mutation[] = [];
  const selections: ElementId[][] = [];
  let frameInserted = false;
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    document: {
      async meta() {
        return { activePage: "Page/u1" } as never;
      },
      async collection(name: string) {
        if (name === "stories" && frameInserted && storyId) {
          return [{ selfId: storyId }] as never;
        }
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
            frameInserted = true;
            return { applied: true, createdId, pageIds: ["Page/u1"] };
          }
          return { applied: true, createdId: null, pageIds: ["Page/u1"] };
        }
        if (m.op === "insertTable") {
          // The REAL engine mints a structured Table id
          // ({kind:"table", id:{story_id, table_id}}) — NOT a bare string.
          // (The old bare-string mock hid the cell-addressing bug.)
          return {
            applied: true,
            createdId: {
              kind: "table",
              id: { story_id: storyId ?? "Story/u9", table_id: "table1" },
            } as ElementId,
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
    // phase1 batch + insertTable + 2 cell-text insertText + 1 decor batch.
    expect(mutations).toHaveLength(5);
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
    // Phase 3 — TWO lanes (NOT one batch): each cell's text via its own
    // insertText (the text lane can't ride an Operation::Batch), then the
    // decor (the engine's h-rule at the bottom boundary → tableCell-scoped
    // bottom-edge strokes on both columns) as ONE batch — the LAST mutation.
    const tail = mutations.slice(2) as Array<{
      op: string;
      args: { text?: string; cell?: unknown; ops?: Array<{ op: string; args: { path?: string; elementId?: unknown } }> };
    }>;
    const textPours = tail.filter((o) => o.op === "insertText");
    expect(textPours).toHaveLength(2);
    const item = textPours.find((o) => o.args.text === "Item");
    expect(item?.args.cell).toEqual({ tableId: "table1", row: 0, col: 0 });
    expect(textPours.find((o) => o.args.text === "Qty")?.args.cell).toEqual({
      tableId: "table1",
      row: 0,
      col: 1,
    });
    const decor = mutations[mutations.length - 1] as {
      op: string;
      args: { ops: Array<{ op: string; args: { path?: string; elementId?: unknown } }> };
    };
    expect(decor.op).toBe("batch");
    const edges = decor.args.ops.filter((o) => o.op === "setElementProperty");
    expect(edges).toHaveLength(2); // one per column under the h-rule at 18
    expect(edges.every((o) => o.args.path === "cellBottomEdgeStrokeWeight")).toBe(
      true,
    );
    expect(edges[0].args.elementId).toEqual({
      kind: "tableCell",
      id: { story_id: "Story/u9", table_id: "table1", row: 0, col: 0 },
    });
    // The new frame is selected.
    expect(selections).toEqual([[CREATED]]);
  });

  it("lane: 'tab-text' drives the retained degradation (rules + text pour)", async () => {
    const { host, mutations, selections } = fakeHost(CREATED, "Story/u9");
    const id = await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1", {
      lane: "tab-text",
    });

    expect(id).toBe("frame1");
    expect(mutations.map((m) => m.op)).toEqual(["batch", "insertText"]);
    // Phase 1 — frame + DRAWN rule + binding (the spec §2.2 degradation).
    const ops = (mutations[0] as { args: { ops: Array<{ op: string }> } }).args
      .ops;
    expect(ops[0].op).toBe("insertTextFrame");
    expect(ops.some((o) => o.op === "insertLine")).toBe(true);
    expect(ops.some((o) => o.op === "setPluginMetadata")).toBe(true);
    expect(ops.some((o) => o.op === "insertTable")).toBe(false);
    // Phase 2 — the tab/newline join poured at offset 0 (no cell qualifier).
    const pour = mutations[1] as {
      args: { storyId: string; offset: number; text: string; cell?: unknown };
    };
    expect(pour.args.storyId).toBe("Story/u9");
    expect(pour.args.offset).toBe(0);
    expect(pour.args.text).toBe("Item\tQty");
    expect(pour.args.cell).toBeUndefined();
    expect(selections).toEqual([[CREATED]]);
  });

  it("falls back to the tab-text pour when the host rejects insertTable", async () => {
    const { host, mutations } = fakeHost(CREATED, "Story/u9");
    // Wrap mutate: reject insertTable (an older wire), apply the rest.
    const realMutate = host.document.mutate.bind(host.document);
    host.document.mutate = async (m: Mutation) => {
      if (m.op === "insertTable") {
        mutations.push(m);
        return { applied: false, error: "unknown op" } as MutationOutcome;
      }
      return realMutate(m);
    };

    const id = await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1");
    expect(id).toBe("frame1"); // frame stands
    expect(mutations.map((m) => m.op)).toEqual([
      "batch", // phase 1 — frame + binding
      "insertTable", // rejected
      "insertText", // the runtime tab-text fallback pour
    ]);
    const pour = mutations[2] as { args: { text: string; cell?: unknown } };
    expect(pour.args.text).toBe("Item\tQty");
    expect(pour.args.cell).toBeUndefined();
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

// ── cell fills reach the PAGE as real swatches (sheet.lower.native-table) ────
//
// A `{type:"colorRef"}` value is a swatch id. Rendered through the real
// engine, a `cellFillColor` naming a raw hex paints NOTHING (0 non-white
// pixels) while the same document with the swatch minted paints the cell.
// So the driver must mint before it refs — and must NOT re-mint an id the
// document already carries, because core refuses a duplicate `createSwatch`
// and the refusal fails the WHOLE batch (verified: a batch led by a
// duplicate mint dropped its sibling fills).

/** The 2x1 fixture engine, but with one FILLED cell (style key 1). */
function filledEngine(): SheetEngine {
  const e = fakeEngine();
  return {
    ...e,
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
            { col: 0, text: "Item", align: "left", styleKey: 1 },
            { col: 1, text: "Qty", align: "right", styleKey: 0 },
          ],
        },
      ],
      rules: { h: [], v: [] },
      merges: [],
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
          bold: false,
          italic: false,
          fillRgb: "#FFFF00",
          borderTop: false,
          borderRight: false,
          borderBottom: false,
          borderLeft: false,
        },
      ],
    }),
  };
}

/** `fakeHost` plus a swatch-collection answer: `rows` when given, a THROW
 *  when `rows` is null (the read-failed case the driver must not guess at). */
function fakeHostWithSwatches(
  createdId: ElementId,
  storyId: string,
  rows: string[] | null,
) {
  const built = fakeHost(createdId, storyId);
  const inner = built.host.document.collection.bind(built.host.document);
  (built.host.document as unknown as {
    collection: (name: string) => Promise<unknown>;
  }).collection = async (name: string) => {
    if (name === "swatches") {
      if (rows === null) throw new Error("swatch read failed");
      return rows.map((selfId) => ({ selfId }));
    }
    return inner(name as never);
  };
  return built;
}

const FILL_SWATCH = "Color/uPagedSheetCellFillFFFF00";

describe("sheet_lower_native_table: the driver mints the fills it references", () => {
  const decorOf = (mutations: Mutation[]) =>
    (mutations[mutations.length - 1] as {
      args: { ops: Array<{ op: string; args: Record<string, any> }> };
    }).args.ops;

  it("the decor batch LEADS with the createSwatch its cellFillColor names", async () => {
    const { host, mutations } = fakeHostWithSwatches(CREATED, "Story/u9", []);
    await lowerSelectionToFrame(host, filledEngine(), 0, "A1:B1");

    const ops = decorOf(mutations);
    expect(ops[0].op).toBe("createSwatch");
    expect(ops[0].args.spec).toMatchObject({
      selfId: FILL_SWATCH,
      space: "RGB",
      value: [255, 255, 0],
    });
    const fill = ops.find((o) => o.args.path === "cellFillColor")!;
    expect(fill.args.value).toEqual({ type: "colorRef", value: FILL_SWATCH });
  });

  it("does NOT re-mint a swatch the document already carries", async () => {
    const { host, mutations } = fakeHostWithSwatches(CREATED, "Story/u9", [
      FILL_SWATCH,
    ]);
    await lowerSelectionToFrame(host, filledEngine(), 0, "A1:B1");

    const ops = decorOf(mutations);
    expect(ops.some((o) => o.op === "createSwatch")).toBe(false);
    // The fill still rides — it names the swatch that is already there.
    expect(
      ops.find((o) => o.args.path === "cellFillColor")!.args.value,
    ).toEqual({ type: "colorRef", value: FILL_SWATCH });
  });

  it("a FAILED swatch read mints nothing rather than risk the whole batch", async () => {
    const { host, mutations } = fakeHostWithSwatches(CREATED, "Story/u9", null);
    await lowerSelectionToFrame(host, filledEngine(), 0, "A1:B1");

    const ops = decorOf(mutations);
    expect(ops.some((o) => o.op === "createSwatch")).toBe(false);
    // The decor still applies; the fill degrades to unpainted (the
    // pre-fix behaviour) instead of taking the edge strokes down with it.
    expect(ops.some((o) => o.args.path === "cellFillColor")).toBe(true);
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

// ── the mint guard on the chart lane (sheet.lower.swatch-mint-dedupe) ───────
//
// The chart's swatch mints ride INSIDE its phase-1 batch, at deterministic
// content-addressed ids. Core refuses a duplicate `createSwatch` and the
// refusal fails the WHOLE batch, so before this guard a second
// `lowerChartToFrame` landed NOTHING — not just a lost colour, the whole
// chart. Measured against core's real engine (paged-run + the CPU
// rasterizer) over the exact ops this driver emits:
//
//   chart 0, then chart 0 AGAIN   → 2nd batch applied:false, scene tree
//                                   unchanged at 18 nodes
//   chart 0, then a DIFFERENT     → 2nd batch applied:false (both charts
//     chart                         carry the axis grey #888888)
//   the same batch, mints removed → applied:true, 18 nodes, art present
//                                   but UNPAINTED
//
// The last row is why a FAILED read degrades the colour, not the batch.

const CHART_SWATCH = "Color/uPagedSheetChart4E79A7";

describe("sheet_lower_swatch_mint_dedupe: the chart lane reads before it mints", () => {
  const batchOps = (mutations: Mutation[]) =>
    (mutations[0] as { args: { ops: Array<{ op: string; args: any }> } }).args
      .ops;

  it("a virgin document gets the mint the frameFillColor names", async () => {
    const { host, mutations } = fakeHostWithSwatches(CREATED, "Story/u9", []);
    expect(await lowerChartToFrame(host, fakeChartEngine(), 0)).toBe(true);

    const ops = batchOps(mutations);
    expect(ops[0].op).toBe("createSwatch");
    expect(ops[0].args.spec.selfId).toBe(CHART_SWATCH);
  });

  it("a RE-LOWER does NOT re-mint — so the whole chart still lands", async () => {
    const { host, mutations } = fakeHostWithSwatches(CREATED, "Story/u9", [
      CHART_SWATCH,
    ]);
    expect(await lowerChartToFrame(host, fakeChartEngine(), 0)).toBe(true);

    const ops = batchOps(mutations);
    expect(ops.some((o) => o.op === "createSwatch")).toBe(false);
    // The geometry is intact and the fill still names the swatch that is
    // already in the document.
    expect(ops.some((o) => o.op === "insertPath")).toBe(true);
    expect(ops.some((o) => o.op === "insertTextFrame")).toBe(true);
    expect(ops.some((o) => o.op === "setPluginMetadata")).toBe(true);
    expect(
      ops.find((o) => o.args?.path === "frameFillColor")!.args.value,
    ).toEqual({ type: "colorRef", value: CHART_SWATCH });
  });

  it("a FAILED swatch read mints nothing rather than risk the whole chart", async () => {
    const { host, mutations } = fakeHostWithSwatches(CREATED, "Story/u9", null);
    expect(await lowerChartToFrame(host, fakeChartEngine(), 0)).toBe(true);

    const ops = batchOps(mutations);
    expect(ops.some((o) => o.op === "createSwatch")).toBe(false);
    // The art still lands (an unresolvable colorRef is a paint miss, not
    // an op error — verified by render); only the colour degrades.
    expect(ops.some((o) => o.op === "insertPath")).toBe(true);
    expect(ops.some((o) => o.args?.path === "frameFillColor")).toBe(true);
  });
});

// ── the mint guard on the tab-text lane's data bars ─────────────────────────
//
// LATENT, stated rather than implied: `getRangeLowered` routes through
// Rust's `lower_range`, NOT `lower_range_condfmt`, so `databars` never
// crosses the wasm boundary today and this lane mints nothing in
// production. The guard is here because the op sequence it WOULD emit is
// fatal on a second apply exactly as the chart lane's was (measured: the
// same data-bar batch applied twice → 2nd applied:false), and because a
// duplicate here costs the FRAME, the rules and the binding — not a bar.

/** `filledEngine` plus one conditional-format data bar in the lowered
 *  region (what `lower_range_condfmt` produces; see the note above). */
function dataBarEngine(): SheetEngine {
  const e = filledEngine();
  return {
    ...e,
    getRangeLowered: (sheet, range, opts) => ({
      ...e.getRangeLowered(sheet, range, opts),
      databars: [
        {
          row: 0,
          col: 0,
          x: 0,
          y: 0,
          w: 20,
          h: 10,
          fillFraction: 0.8,
          fill: "#638EC6",
        },
      ],
    }),
  };
}

const BAR_SWATCH = "Color/uPagedSheetDataBar638EC6";

describe("sheet_lower_swatch_mint_dedupe: the tab-text lane reads before it mints", () => {
  const batchOps = (mutations: Mutation[]) =>
    (mutations[0] as { args: { ops: Array<{ op: string; args: any }> } }).args
      .ops;

  it("a virgin document gets the bar mint the frameFillColor names", async () => {
    const { host, mutations } = fakeHostWithSwatches(CREATED, "Story/u9", []);
    await lowerSelectionToFrame(host, dataBarEngine(), 0, "A1:B1", {
      lane: "tab-text",
    });

    const ops = batchOps(mutations);
    const mint = ops.find((o) => o.op === "createSwatch")!;
    expect(mint.args.spec.selfId).toBe(BAR_SWATCH);
  });

  it("a RE-LOWER mints nothing — the frame, rules and binding still land", async () => {
    const { host, mutations } = fakeHostWithSwatches(CREATED, "Story/u9", [
      BAR_SWATCH,
    ]);
    await lowerSelectionToFrame(host, dataBarEngine(), 0, "A1:B1", {
      lane: "tab-text",
    });

    const ops = batchOps(mutations);
    expect(ops.some((o) => o.op === "createSwatch")).toBe(false);
    expect(ops[0].op).toBe("insertTextFrame");
    expect(ops.some((o) => o.op === "insertPath")).toBe(true);
    expect(ops.some((o) => o.op === "setPluginMetadata")).toBe(true);
    expect(
      ops.find((o) => o.args?.path === "frameFillColor")!.args.value,
    ).toEqual({ type: "colorRef", value: BAR_SWATCH });
  });

  it("a FAILED swatch read mints nothing rather than risk the frame", async () => {
    const { host, mutations } = fakeHostWithSwatches(CREATED, "Story/u9", null);
    await lowerSelectionToFrame(host, dataBarEngine(), 0, "A1:B1", {
      lane: "tab-text",
    });

    const ops = batchOps(mutations);
    expect(ops.some((o) => o.op === "createSwatch")).toBe(false);
    expect(ops[0].op).toBe("insertTextFrame");
    expect(ops.some((o) => o.op === "setPluginMetadata")).toBe(true);
  });

  it("a region with NO data bars costs no swatch read at all", async () => {
    // The guard must not add a host round-trip to the ordinary path —
    // today's `getRangeLowered` never emits a bar.
    const reads: string[] = [];
    const { host } = fakeHostWithSwatches(CREATED, "Story/u9", []);
    const inner = host.document.collection.bind(host.document);
    (host.document as unknown as {
      collection: (name: string) => Promise<unknown>;
    }).collection = async (name: string) => {
      reads.push(name);
      return inner(name as never);
    };
    await lowerSelectionToFrame(host, filledEngine(), 0, "A1:B1", {
      lane: "tab-text",
    });
    expect(reads).not.toContain("swatches");
  });
});

// ── live multi-frame pagination across the host chain (Wave 2D, S-05) ────────

/** Drain the microtask queue until `pred` holds or `ticks` is exhausted —
 *  the async chain-lower flow hops several awaits (meta → frameChain →
 *  elementGeometry → paginate → per-frame hitTest/mutate), so a fixed
 *  `Promise.resolve()` count is brittle. */
async function until(pred: () => boolean, ticks = 50): Promise<void> {
  for (let i = 0; i < ticks && !pred(); i++) {
    await Promise.resolve();
  }
}

/** One page over a fixed 1-row content (the slice text differs per page). */
function pageFor(frameIndex: number, text: string, continued: boolean): Page {
  return {
    frameIndex,
    content: {
      cols: [{ index: 0, widthPt: 50 }],
      rows: [
        { index: 0, heightPt: 18, cells: [{ col: 0, text, align: "left" }] },
      ],
      rules: { h: [], v: [] },
      merges: [],
    },
    continued,
    oversize: false,
  };
}

/** A fake engine whose `paginate` returns two pages (one per chain frame),
 *  recording the boxes it was handed. */
function fakeChainEngine() {
  const paginateCalls: Array<{
    sheet: number;
    range: string;
    frames: Array<{ widthPt: number; heightPt: number }>;
  }> = [];
  const engine: SheetEngine = {
    ...fakeEngine(),
    paginate(sheet, range, frames) {
      paginateCalls.push({ sheet, range, frames });
      return [pageFor(0, "r0", true), pageFor(1, "r1", false)];
    },
  };
  return { engine, paginateCalls };
}

/** A fake host with a real frame chain (2 links), per-frame geometry boxes,
 *  per-frame story resolution (hitTest), table minting (mutate), and an
 *  onDidChange channel the test fires reflow / non-reflow events on. */
function fakeChainHost(links: FrameChainLink[]) {
  const mutations: Mutation[] = [];
  const listeners: Array<(e: DocumentChangeEvent) => void> = [];
  let tableSeq = 0;

  // Each frame: a content box (from elementGeometry) + a story (from hitTest).
  const boxes: Record<string, [number, number, number, number]> = {
    f0: [0, 0, 54, 50], // 54pt tall (3 × 18) , 50 wide
    f1: [0, 0, 54, 50],
  };
  const storyByFrame: Record<string, string> = {
    f0: "Story/f0",
    f1: "Story/f1",
  };

  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    document: {
      async meta() {
        return { activePage: "Page/u1" } as never;
      },
      async collection() {
        return [] as never;
      },
      async frameChain(_storyId: string): Promise<FrameChainLink[]> {
        return links;
      },
      async elementGeometry(ids: ElementId[]): Promise<ElementGeometryItem[]> {
        return ids
          .map((id) => {
            const fid = (id as { id: string }).id;
            const bounds = boxes[fid];
            if (!bounds) return null;
            return {
              id,
              pageId: "Page/u1",
              bounds,
            } as ElementGeometryItem;
          })
          .filter((x): x is ElementGeometryItem => x !== null);
      },
      async hitTest(_pageId: string, _pt: [number, number]) {
        // The flow hit-tests each frame's center; we resolve the story from
        // which frame's box contains the point. Both frames are placed at the
        // origin here, so route by call order via a per-frame map keyed on the
        // y-extent — simpler: return the NEXT unresolved frame's story.
        const fid = pendingFrames.shift();
        return fid
          ? ({ storyId: storyByFrame[fid], frameId: fid } as never)
          : null;
      },
      async mutate(m: Mutation): Promise<MutationOutcome> {
        mutations.push(m);
        if (m.op === "insertTable") {
          tableSeq += 1;
          // Realistic structured Table id (as the real engine mints).
          return {
            applied: true,
            createdId: {
              kind: "table",
              id: { story_id: `Story/f${tableSeq - 1}`, table_id: `tbl${tableSeq}` },
            } as ElementId,
            pageIds: ["Page/u1"],
          };
        }
        return { applied: true, createdId: null, pageIds: ["Page/u1"] };
      },
      onDidChange(listener: (e: DocumentChangeEvent) => void) {
        listeners.push(listener);
        return { dispose() {} };
      },
    },
    text: {
      async measureString() {
        return { advance: 30, ascender: 9, descender: -2 };
      },
    },
    selection: {
      async set(ids: ElementId[]) {
        return ids;
      },
    },
  } as unknown as BundleHost;

  // The hitTest order: the flow lowers page 0 (frame f0) then page 1 (f1).
  const pendingFrames = ["f0", "f1"];
  // Reset before each pass: lowerPaginatedToChain hit-tests in chain order.
  const resetHitOrder = () => {
    pendingFrames.length = 0;
    pendingFrames.push("f0", "f1");
  };

  const fire = (e: DocumentChangeEvent) => {
    resetHitOrder();
    for (const l of listeners) l(e);
  };

  return { host, mutations, listeners, fire, resetHitOrder };
}

const CHAIN: FrameChainLink[] = [
  { frameId: "f0", next: "f1", overflow: false },
  { frameId: "f1", next: null, overflow: false },
];

describe("sheet_plugin_lower_chain: live multi-frame pagination", () => {
  it("reads the chain, paginates into its boxes, lowers each page to its frame", async () => {
    const { host, mutations } = fakeChainHost(CHAIN);
    const { engine, paginateCalls } = fakeChainEngine();

    const result = await lowerPaginatedToChain(
      host,
      engine,
      0,
      "A1:A6",
      "Story/f0",
      { continuedMarker: true },
    );

    expect(result).not.toBeNull();
    // The engine was handed the chain's TWO content boxes (height 54 each).
    expect(paginateCalls).toHaveLength(1);
    expect(paginateCalls[0].frames).toEqual([
      { widthPt: 50, heightPt: 54 },
      { widthPt: 50, heightPt: 54 },
    ]);

    // Two pages → two tables, one per frame's resolved story.
    const inserts = mutations.filter((m) => m.op === "insertTable") as Array<{
      args: { storyId: string };
    }>;
    expect(inserts).toHaveLength(2);
    expect(inserts[0].args.storyId).toBe("Story/f0");
    expect(inserts[1].args.storyId).toBe("Story/f1");

    // Each frame got its OWN page's cell text via individual insertText (the
    // text lane — NOT a batch), r0 → tbl1, r1 → tbl2.
    const pours = mutations.filter((m) => m.op === "insertText") as Array<{
      args: { text?: string; cell?: { tableId?: string } };
    }>;
    expect(pours.find((o) => o.args.text === "r0")?.args.cell?.tableId).toBe("tbl1");
    expect(pours.find((o) => o.args.text === "r1")?.args.cell?.tableId).toBe("tbl2");

    expect(result!.tableIds).toEqual(["tbl1", "tbl2"]);
  });

  it("re-paginates on a reflow event for a chain frame, ignores non-reflow", async () => {
    const { host, mutations, fire } = fakeChainHost(CHAIN);
    const { engine, paginateCalls } = fakeChainEngine();

    const sub = subscribeChainReflow(host, engine, 0, "A1:A6", "Story/f0", {
      continuedMarker: true,
    });
    // Let the async chain-prime in subscribeChainReflow settle.
    await until(() => false, 5);

    const insertsBefore = mutations.filter((m) => m.op === "insertTable").length;
    expect(paginateCalls).toHaveLength(0);

    // (a) A change with NO reflow is the §8.5 transform case — IGNORED.
    fire({ kind: "mutationApplied", pageIds: ["Page/u1"] });
    await until(() => false, 10); // give a re-pagination a chance to (not) run
    expect(paginateCalls).toHaveLength(0);
    expect(mutations.filter((m) => m.op === "insertTable").length).toBe(
      insertsBefore,
    );

    // (b) A reflow for a frame IN the chain re-paginates the whole chain.
    fire({
      kind: "mutationApplied",
      pageIds: ["Page/u1"],
      reflow: { frameId: "f0", contentBox: [0, 0, 36, 50] },
    });
    await until(() => paginateCalls.length === 1);

    expect(paginateCalls).toHaveLength(1); // re-paginated exactly once
    await until(
      () => mutations.filter((m) => m.op === "insertTable").length ===
        insertsBefore + 2,
    );
    expect(mutations.filter((m) => m.op === "insertTable").length).toBe(
      insertsBefore + 2, // two pages re-lowered
    );

    // (c) A reflow for a frame NOT in the chain is ignored.
    fire({
      kind: "mutationApplied",
      pageIds: ["Page/u1"],
      reflow: { frameId: "fOTHER", contentBox: [0, 0, 10, 10] },
    });
    await until(() => false, 10);
    expect(paginateCalls).toHaveLength(1); // unchanged

    sub.dispose();
  });

  it("returns null when the story threads no frames", async () => {
    const { host } = fakeChainHost([]);
    const { engine, paginateCalls } = fakeChainEngine();
    const result = await lowerPaginatedToChain(
      host,
      engine,
      0,
      "A1:A6",
      "Story/empty",
    );
    expect(result).toBeNull();
    expect(paginateCalls).toHaveLength(0); // never paginates an empty chain
  });
});

// ── S-13 · the width is measured for the text that will RENDER ────────
//
// `measureColumnWidths` asks the host shaper how wide a column's widest
// string is and sizes the column to it. That is only true if it asks
// about the SAME text the pour produces: the pour writes
// `characterFontSize` only when the workbook's style carries one, so an
// un-styled cell renders at the engine's default (12 pt) and asking at
// any other size measures a document nobody rendered.
//
// The fallback was 11 (Excel's Calibri default). Every un-styled column
// came out 9% narrow; the flat 4 pt inset hid it until a header was wide
// enough to eat the slack, and then the column's OWN HEADER wrapped —
// "Revenue" in the showcase's workbook, measured 44.91 pt at 11 pt,
// rendered 49.02 pt at 12 pt, in a 48.91 pt column.
//
// No mock could catch it before this: all three `measureString` fakes in
// this repo ignore their arguments and return a constant. This one
// RECORDS them.
describe("sheet_plugin_lower_measure: the measured size is the rendered size", () => {
  /** A lowered IR with one un-styled column and one carrying an explicit
   *  8 pt style — the two cases the fallback has to tell apart. */
  function styledEngine(): SheetEngine {
    const base = fakeEngine();
    return {
      ...base,
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
              { col: 0, text: "Region", align: "left" as const },
              { col: 1, text: "Revenue", align: "left" as const, styleKey: 7 },
            ],
          },
        ],
        rules: { h: [], v: [] },
        merges: [],
        styles: [
          {
            key: 7,
            bold: false,
            italic: false,
            fontSizePt: 8,
            fontName: "Courier",
            borderTop: false,
            borderRight: false,
            borderBottom: false,
            borderLeft: false,
          },
        ],
      }),
    } as SheetEngine;
  }

  /** `fakeHost` with a measureString that records what it was asked. */
  function recordingHost(createdId: ElementId, storyId: string) {
    const asked: Array<{ family: string; text: string; sizePt: number }> = [];
    const { host, mutations } = fakeHost(createdId, storyId);
    (host as unknown as { text: unknown }).text = {
      async measureString(
        family: string,
        _style: string | null,
        text: string,
        sizePt: number,
      ) {
        asked.push({ family, text, sizePt });
        return { advance: 30, ascender: 9, descender: -2 };
      },
    };
    return { host, asked, mutations };
  }

  it("measures an un-styled column at the size the engine pours it at", async () => {
    const { host, asked } = recordingHost(CREATED, "Story/u9");
    await lowerSelectionToFrame(host, styledEngine(), 0, "A1:B1");

    const region = asked.find((a) => a.text === "Region");
    expect(region, "the un-styled column was measured").toBeDefined();
    // 12 pt is `PipelineOptions::default_point_size` — what a bare run
    // renders at. Asking at 11 (Excel's default) is what wrapped
    // "Revenue" in the showcase.
    expect(region?.sizePt).toBe(DEFAULT_CELL_POINT_SIZE);
    expect(DEFAULT_CELL_POINT_SIZE).toBe(12);
    // …and at the face a bare run resolves to, not a guessed name.
    expect(region?.family).toBe("");
  });

  it("measures a styled column at ITS size, which the pour then writes", async () => {
    const { host, asked } = recordingHost(CREATED, "Story/u9");
    await lowerSelectionToFrame(host, styledEngine(), 0, "A1:B1");

    const revenue = asked.find((a) => a.text === "Revenue");
    expect(revenue?.sizePt).toBe(8);
    expect(revenue?.family).toBe("Courier");
  });
});
