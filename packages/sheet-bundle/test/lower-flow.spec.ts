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
    // Phase 3 — ONE batch: each cell's text poured into its table cell,
    // then the decor (the engine's h-rule at the bottom boundary becomes
    // tableCell-scoped bottom-edge strokes on both columns).
    expect(mutations[2].op).toBe("batch");
    const cellOps = (mutations[2] as {
      args: {
        ops: Array<{
          op: string;
          args: { text?: string; cell?: unknown; path?: string; elementId?: unknown };
        }>;
      };
    }).args.ops;
    const item = cellOps.find((o) => o.args.text === "Item");
    expect(item?.op).toBe("insertText");
    expect(item?.args.cell).toEqual({ tableId: "table1", row: 0, col: 0 });
    expect(cellOps.find((o) => o.args.text === "Qty")?.args.cell).toEqual({
      tableId: "table1",
      row: 0,
      col: 1,
    });
    const edges = cellOps.filter((o) => o.op === "setElementProperty");
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
          return {
            applied: true,
            createdId: { kind: "textFrame", id: `tbl${tableSeq}` } as ElementId,
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

    // Each frame got its OWN page's cell text (r0 → f0, r1 → f1).
    const pours = mutations.filter((m) => m.op === "batch") as Array<{
      args: { ops: Array<{ args: { text?: string } }> };
    }>;
    expect(pours).toHaveLength(2);
    expect(pours[0].args.ops.some((o) => o.args.text === "r0")).toBe(true);
    expect(pours[1].args.ops.some((o) => o.args.text === "r1")).toBe(true);

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
