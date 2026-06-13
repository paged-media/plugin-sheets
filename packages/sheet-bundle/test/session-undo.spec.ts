// sheet.grid.session-undo-journal (ADR-012 Tier 1): the session keeps a
// journal of COMMITTED cell edits — prev/next as re-enterable INPUT texts
// (engine.getCellInput) — behind undoCellEdit/redoCellEdit; an OPEN edit
// buffer unwinds first; a fresh commit truncates the redo tail; the
// journal dies at the session boundary (clear + workbook load). No DOM,
// no wasm: a stateful fake engine stores inputs so the inverses are real.

import { describe, expect, it } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";

import { createWorkbookSession, type SheetEngine } from "../src";

/** A stateful fake: cells hold their INPUT text; getCellInput/Display
 *  read it back (display == input is fine for the journal contract —
 *  the formula-vs-display split is pinned engine-side in
 *  sheet-conformance js_surface.rs). */
function statefulEngine() {
  const cells = new Map<string, string>();
  const key = (s: number, r: number, c: number) => `${s}:${r}:${c}`;
  const engine: SheetEngine = {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: (sheet, row, col, input) => {
      cells.set(key(sheet, row, col), input);
      return { changed: [{ sheet, row, col, display: input }] };
    },
    getCellDisplay: (sheet, row, col) => cells.get(key(sheet, row, col)) ?? "",
    getCellInput: (sheet, row, col) => cells.get(key(sheet, row, col)) ?? "",
    sortRange: () => ({ changed: [], edits: [] }),
    findAll: () => [],
    replaceAll: () => ({ occurrences: 0, changed: [], edits: [], skipped: [] }),
    getRangeLowered: () => ({
      cols: [],
      rows: [],
      rules: { h: [], v: [] },
      merges: [],
    }),
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
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 9, cols: 9 }],
    listCharts: () => [],
    listFunctions: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
  return { engine, cells, key };
}

const silentHost = {
  log: { debug() {}, info() {}, warn() {}, error() {} },
  supports: () => false,
} as unknown as BundleHost;

function booted() {
  const { engine, cells, key } = statefulEngine();
  const session = createWorkbookSession(silentHost);
  const st = session.state();
  st.engine = engine;
  st.activeSheet = 0;
  st.fileName = "demo.xlsx";
  return { session, cells, key };
}

describe("sheet_grid_session_undo_journal: ADR-012 Tier 1", () => {
  it("undo re-enters the previous input; redo re-applies; exhaustion is false", () => {
    const { session, cells, key } = booted();
    cells.set(key(0, 0, 0), "=SUM(A2:A3)"); // pre-existing formula

    expect(session.canUndoCellEdit()).toBe(false);
    expect(session.editCell(0, 0, 0, "42")).toBe(true);
    expect(session.editCell(0, 1, 0, "7")).toBe(true);
    expect(session.canUndoCellEdit()).toBe(true);

    // Undo A2 (7 → empty), then A1 (42 → the formula, NOT a display).
    expect(session.undoCellEdit()).toBe(true);
    expect(cells.get(key(0, 1, 0))).toBe("");
    expect(session.undoCellEdit()).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("=SUM(A2:A3)");
    expect(session.undoCellEdit()).toBe(false); // exhausted — no fall-through
    expect(session.canUndoCellEdit()).toBe(false);

    // Redo walks forward again.
    expect(session.canRedoCellEdit()).toBe(true);
    expect(session.redoCellEdit()).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("42");
    expect(session.redoCellEdit()).toBe(true);
    expect(cells.get(key(0, 1, 0))).toBe("7");
    expect(session.redoCellEdit()).toBe(false);
  });

  it("a fresh commit truncates the redo tail (linear history)", () => {
    const { session, cells, key } = booted();
    session.editCell(0, 0, 0, "1");
    session.editCell(0, 0, 0, "2");
    session.undoCellEdit(); // back to "1"
    expect(session.canRedoCellEdit()).toBe(true);
    session.editCell(0, 0, 0, "3"); // diverge — drops the "2" redo
    expect(session.canRedoCellEdit()).toBe(false);
    session.undoCellEdit();
    expect(cells.get(key(0, 0, 0))).toBe("1");
  });

  it("an OPEN edit buffer unwinds first (Cmd-Z mid-typing = cancel, no journal step)", () => {
    const { session, cells, key } = booted();
    session.editCell(0, 0, 0, "1");
    session.setGridSelection(0, 0, 1, 1);
    expect(session.typeCellChar("9")).toBe(true);
    expect(session.isCellEditing()).toBe(true);
    // First undo only closes the buffer; the committed "1" survives.
    expect(session.undoCellEdit()).toBe(true);
    expect(session.isCellEditing()).toBe(false);
    expect(cells.get(key(0, 0, 0))).toBe("1");
    // Second undo pops the journal.
    expect(session.undoCellEdit()).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("");
  });

  it("the in-frame commit path journals too, and clear drops everything", () => {
    const { session, cells, key } = booted();
    session.setGridSelection(0, 0, 1, 1);
    session.typeCellChar("5");
    expect(session.commitCellEdit()).toBe(true);
    expect(cells.get(key(0, 0, 0))).toBe("5");
    expect(session.canUndoCellEdit()).toBe(true);
    session.clearCellEditJournal(); // the modal exit boundary
    expect(session.canUndoCellEdit()).toBe(false);
    expect(session.undoCellEdit()).toBe(false);
  });
});
