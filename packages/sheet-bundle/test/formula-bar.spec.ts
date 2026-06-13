// sheet.plugin.formula-bar (session glue): the bar prefills with the cell's
// re-enterable INPUT (engine.getCellInput, formula-safe), sources its
// autocomplete names from the ENGINE registry (engine.listFunctions, cached),
// and commits through the journaled editCell lane (one undoable step). THIN
// GLUE (§3): no completion/matching logic here — that is pinned pure in
// sheet-host-model/test/completions.spec.ts. No DOM, no wasm.

import { describe, expect, it } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";

import { createWorkbookSession, type SheetEngine } from "../src";

function fakeEngine() {
  const cells = new Map<string, string>();
  const key = (s: number, r: number, c: number) => `${s}:${r}:${c}`;
  let listFnCalls = 0;
  const engine: SheetEngine = {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: (sheet, row, col, input) => {
      cells.set(key(sheet, row, col), input);
      return { changed: [{ sheet, row, col, display: input }] };
    },
    getCellDisplay: (s, r, c) => cells.get(key(s, r, c)) ?? "",
    getCellInput: (s, r, c) => cells.get(key(s, r, c)) ?? "",
    sortRange: () => ({ changed: [], edits: [] }),
    findAll: () => [],
    replaceAll: () => ({ occurrences: 0, changed: [], edits: [], skipped: [] }),
    getRangeLowered: () => ({ cols: [], rows: [], rules: { h: [], v: [] }, merges: [] }),
    getRangeValues: () => [],
    paginate: () => [],
    getGridScene: () => ({
      viewport: { firstRow: 0, firstCol: 0, rows: 1, cols: 1, xOffsets: [0, 40], yOffsets: [0, 20] },
      cells: [],
      styles: [],
      gridlines: { h: [], v: [] },
      selection: null,
    }),
    setGridSelection() {},
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 9, cols: 9 }],
    listCharts: () => [],
    listFreezePanes: () => [],
    listDataValidations: () => [],
    listComments: () => [],
    listFunctions: () => {
      listFnCalls++;
      return [
        { name: "SUM", family: "math", minArgs: 1, maxArgs: null },
        { name: "VLOOKUP", family: "lookup", minArgs: 3, maxArgs: 4 },
      ];
    },
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
  return { engine, cells, key, listFnCalls: () => listFnCalls };
}

const silentHost = {
  log: { debug() {}, info() {}, warn() {}, error() {} },
  supports: () => false,
} as unknown as BundleHost;

function booted() {
  const fake = fakeEngine();
  const session = createWorkbookSession(silentHost);
  const st = session.state();
  st.engine = fake.engine;
  st.activeSheet = 0;
  st.selectedRange = "A1:B3";
  return { session, ...fake };
}

describe("sheet_plugin_formula_bar: session glue", () => {
  it("cellInputAt reads the re-enterable INPUT (formula, not display)", () => {
    const { session, cells, key } = booted();
    cells.set(key(0, 2, 0), "=SUM(A1:A2)");
    expect(session.cellInputAt(2, 0)).toBe("=SUM(A1:A2)");
    expect(session.cellInputAt(9, 9)).toBe(""); // empty cell
  });

  it("cellInputAt returns '' without an engine / sheet (never throws)", () => {
    const session = createWorkbookSession(silentHost);
    expect(session.cellInputAt(0, 0)).toBe("");
  });

  it("functionList sources names from the engine and CACHES (one wasm call)", () => {
    const { session, listFnCalls } = booted();
    expect(session.functionList().map((f) => f.name)).toEqual(["SUM", "VLOOKUP"]);
    // A second read is served from the cache (the registry is build-fixed).
    session.functionList();
    expect(listFnCalls()).toBe(1);
  });

  it("functionList is empty (never throws) without an engine", () => {
    const session = createWorkbookSession(silentHost);
    expect(session.functionList()).toEqual([]);
  });

  it("a commit rides the journaled editCell lane (one undoable step)", () => {
    const { session, cells, key } = booted();
    cells.set(key(0, 0, 0), "1");
    // The formula bar commits via editCell.
    expect(session.editCell(0, 0, 0, "=SUM(A2:A3)")).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("=SUM(A2:A3)");
    // One undo restores the prior input (the journal's faithful inverse).
    expect(session.undoCellEdit()).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("1");
  });
});
