// sheet.plugin.sort.command + sheet.plugin.find-replace.panel: the session
// glue over the engine's bulk edit ops. THIN-GLUE contract (§3): the session
// only routes the active sheet/range in, journals the engine's per-cell
// prev/next input rewrites as ONE grouped ADR-012 step (a sort or
// replace-all undoes with a single Cmd-Z), and surfaces the engine's honest
// boundary messages ("sort over formulas not yet supported"). All sort /
// find / replace SEMANTICS are pinned engine-side in
// sheet-conformance/tests/edit_ops.rs — none are re-tested (or re-implemented)
// here. No DOM, no wasm: a stateful fake engine applies the edits it reports
// so the journal inverses are real.

import { describe, expect, it } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";

import {
  createWorkbookSession,
  type CellEditRecord,
  type FindOptions,
  type SheetEngine,
} from "../src";

/** A stateful fake: cells hold their INPUT text. `sortRange`/`replaceAll`
 *  return PRESCRIBED edit records and apply them to the cell store (the
 *  same contract the wasm engine honours), so undo/redo inverses are real.
 *  Call args are captured for the routing assertions. */
function statefulEngine() {
  const cells = new Map<string, string>();
  const key = (s: number, r: number, c: number) => `${s}:${r}:${c}`;
  const calls: Array<{ method: string; args: unknown[] }> = [];

  let nextSortEdits: CellEditRecord[] = [];
  let sortError: string | null = null;
  let nextReplace = {
    occurrences: 0,
    edits: [] as CellEditRecord[],
    skipped: [] as { sheet: number; row: number; col: number; reason: string }[],
  };
  let nextHits: { sheet: number; row: number; col: number; excerpt: string }[] =
    [];
  let findThrows = false;

  const apply = (edits: CellEditRecord[]) => {
    for (const e of edits) cells.set(key(e.sheet, e.row, e.col), e.nextInput);
  };

  const engine: SheetEngine = {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: (sheet, row, col, input) => {
      cells.set(key(sheet, row, col), input);
      return { changed: [{ sheet, row, col, display: input }] };
    },
    getCellDisplay: (sheet, row, col) => cells.get(key(sheet, row, col)) ?? "",
    getCellInput: (sheet, row, col) => cells.get(key(sheet, row, col)) ?? "",
    sortRange: (sheet, range, keyCol, ascending, hasHeader) => {
      calls.push({
        method: "sortRange",
        args: [sheet, range, keyCol, ascending, hasHeader],
      });
      if (sortError) throw new Error(sortError);
      apply(nextSortEdits);
      return { changed: [], edits: nextSortEdits };
    },
    findAll: (sheet, needle, opts?: FindOptions) => {
      calls.push({ method: "findAll", args: [sheet, needle, opts] });
      if (findThrows) throw new Error("boom");
      return nextHits;
    },
    replaceAll: (sheet, needle, replacement, opts?: FindOptions) => {
      calls.push({
        method: "replaceAll",
        args: [sheet, needle, replacement, opts],
      });
      apply(nextReplace.edits);
      return { ...nextReplace, changed: [] };
    },
    getRangeLowered: () => ({
      cols: [],
      rows: [],
      rules: { h: [], v: [] },
      merges: [],
    }),
    getRangeValues: () => [],
    paginate: () => [],
    getGridScene: () => ({
      viewport: {
        firstRow: 0,
        firstCol: 0,
        rows: 1,
        cols: 1,
        xOffsets: [0, 40],
        yOffsets: [0, 20],
      },
      cells: [],
      styles: [],
      gridlines: { h: [], v: [] },
      selection: null,
    }),
    setGridSelection() {},
    listSheets: () => [
      { id: 0, name: "Sheet1", rows: 9, cols: 9 },
      { id: 1, name: "Sheet2", rows: 4, cols: 4 },
    ],
    listCharts: () => [],
    listFreezePanes: () => [],
    listDataValidations: () => [],
    listFunctions: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
  return {
    engine,
    cells,
    key,
    calls,
    setSortEdits: (e: CellEditRecord[]) => (nextSortEdits = e),
    setSortError: (m: string | null) => (sortError = m),
    setReplace: (r: typeof nextReplace) => (nextReplace = r),
    setHits: (h: typeof nextHits) => (nextHits = h),
    setFindThrows: (v: boolean) => (findThrows = v),
  };
}

const silentHost = {
  log: { debug() {}, info() {}, warn() {}, error() {} },
  supports: () => false,
} as unknown as BundleHost;

function booted() {
  const fake = statefulEngine();
  const session = createWorkbookSession(silentHost);
  const st = session.state();
  st.engine = fake.engine;
  st.activeSheet = 0;
  st.fileName = "demo.xlsx";
  st.selectedRange = "A1:B3";
  return { session, ...fake };
}

describe("sheet_plugin_sort_command: session glue + grouped journal", () => {
  it("routes the active sheet/range + controls to engine.sortRange", () => {
    const { session, calls } = booted();
    const res = session.sortRange(1, false, true);
    expect(res).toEqual({ ok: true });
    expect(calls).toEqual([
      { method: "sortRange", args: [0, "A1:B3", 1, false, true] },
    ]);
  });

  it("journals the engine's edits as ONE grouped undo/redo step", () => {
    const { session, cells, key, setSortEdits } = booted();
    // The pre-sort state the engine's prev inputs refer to.
    cells.set(key(0, 0, 0), "30");
    cells.set(key(0, 1, 0), "10");
    setSortEdits([
      { sheet: 0, row: 0, col: 0, prevInput: "30", nextInput: "10" },
      { sheet: 0, row: 1, col: 0, prevInput: "10", nextInput: "30" },
    ]);

    expect(session.sortRange(0, true, false)).toEqual({ ok: true });
    expect(cells.get(key(0, 0, 0))).toBe("10");
    expect(cells.get(key(0, 1, 0))).toBe("30");

    // ONE undo unwinds the whole sort (ADR-012 grouped step)…
    expect(session.undoCellEdit()).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("30");
    expect(cells.get(key(0, 1, 0))).toBe("10");
    expect(session.canUndoCellEdit()).toBe(false);

    // …and ONE redo re-applies it.
    expect(session.redoCellEdit()).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("10");
    expect(cells.get(key(0, 1, 0))).toBe("30");
    expect(session.canRedoCellEdit()).toBe(false);
  });

  it("a no-op sort journals nothing", () => {
    const { session, setSortEdits } = booted();
    setSortEdits([]);
    expect(session.sortRange(0, true, false)).toEqual({ ok: true });
    expect(session.canUndoCellEdit()).toBe(false);
  });

  it("surfaces the engine's refusal message honestly (nothing journaled)", () => {
    const { session, setSortError } = booted();
    setSortError("sort over formulas not yet supported (formula at A2)");
    const res = session.sortRange(0, true, false);
    expect(res).toEqual({
      ok: false,
      message: "sort over formulas not yet supported (formula at A2)",
    });
    expect(session.canUndoCellEdit()).toBe(false);
  });

  it("plain cell edits before/after a batch keep their SINGLE-step grain", () => {
    const { session, cells, key, setSortEdits } = booted();
    session.editCell(0, 5, 0, "before");
    setSortEdits([
      { sheet: 0, row: 0, col: 0, prevInput: "", nextInput: "x" },
      { sheet: 0, row: 1, col: 0, prevInput: "", nextInput: "y" },
    ]);
    session.sortRange(0, true, false);
    session.editCell(0, 6, 0, "after");

    // Undo 1: the single "after" edit.
    expect(session.undoCellEdit()).toBe(true);
    expect(cells.get(key(0, 6, 0))).toBe("");
    expect(cells.get(key(0, 0, 0))).toBe("x"); // batch intact
    // Undo 2: the WHOLE batch.
    expect(session.undoCellEdit()).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("");
    expect(cells.get(key(0, 1, 0))).toBe("");
    expect(cells.get(key(0, 5, 0))).toBe("before"); // single edit intact
    // Undo 3: the single "before" edit.
    expect(session.undoCellEdit()).toBe(true);
    expect(session.undoCellEdit()).toBe(false);
  });

  it("guards honestly without a workbook / range", () => {
    const session = createWorkbookSession(silentHost);
    const res = session.sortRange(0, true, false);
    expect(res.ok).toBe(false);
  });
});

describe("sheet_plugin_find_replace_panel: session glue", () => {
  it("findAll scopes 'sheet' to the active sheet and 'workbook' to all", () => {
    const { session, calls, setHits } = booted();
    setHits([{ sheet: 0, row: 1, col: 2, excerpt: "total 30" }]);

    const opts = { matchCase: true, entireCell: false, inFormulas: true };
    const hits = session.findAll("30", opts, "sheet");
    expect(hits).toEqual([{ sheet: 0, row: 1, col: 2, excerpt: "total 30" }]);
    session.findAll("30", opts, "workbook");

    expect(calls.map((c) => c.args[0])).toEqual([0, undefined]);
    expect(calls[0].args.slice(1)).toEqual(["30", opts]);
  });

  it("findAll never throws — an engine failure is an empty hit list", () => {
    const { session, setFindThrows } = booted();
    setFindThrows(true);
    expect(session.findAll("x", {}, "sheet")).toEqual([]);
  });

  it("replaceAll reports counts and journals ONE grouped step", () => {
    const { session, cells, key, setReplace } = booted();
    cells.set(key(0, 0, 0), "draft");
    cells.set(key(0, 1, 0), "draft copy");
    setReplace({
      occurrences: 2,
      edits: [
        { sheet: 0, row: 0, col: 0, prevInput: "draft", nextInput: "final" },
        {
          sheet: 0,
          row: 1,
          col: 0,
          prevInput: "draft copy",
          nextInput: "final copy",
        },
      ],
      skipped: [{ sheet: 0, row: 2, col: 0, reason: "does not parse" }],
    });

    const res = session.replaceAll("draft", "final", {}, "sheet");
    expect(res).toEqual({ occurrences: 2, replacedCells: 2, skipped: 1 });
    expect(cells.get(key(0, 0, 0))).toBe("final");

    // One grouped undo step restores both cells.
    expect(session.undoCellEdit()).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("draft");
    expect(cells.get(key(0, 1, 0))).toBe("draft copy");
    expect(session.canUndoCellEdit()).toBe(false);
  });

  it("replaceAll guards honestly without a workbook", () => {
    const session = createWorkbookSession(silentHost);
    expect(session.replaceAll("a", "b", {}, "sheet")).toEqual({
      error: "no workbook",
    });
  });

  it("goToCell activates the hit's sheet and selects its cell", () => {
    const { session } = booted();
    session.goToCell(1, 3, 2);
    const st = session.state();
    expect(st.activeSheet).toBe(1);
    expect(st.gridSelection).toEqual({
      anchorRow: 3,
      anchorCol: 2,
      rows: 1,
      cols: 1,
    });
    // The range defaulted to the new sheet's used extent.
    expect(st.selectedRange).toBe("A1:D4");
  });
});
