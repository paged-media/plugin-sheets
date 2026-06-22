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

// sheet.grid.clipboard.* (K-6 / S-14): the grid's range copy/paste over
// host.clipboard. No DOM, no wasm — a fake engine returns range values +
// records setCell calls, and a fake host carries an in-memory clipboard so
// we can assert the round-trip + the journaled paste (one grouped undo).
//
// All spreadsheet semantics are the engine's (getRangeValues formats in
// Rust; setCell re-types each string); the session is thin glue: read the
// range → host.clipboard.write, and host.clipboard.read → journaled cells.

import { describe, expect, it } from "vitest";

import type { BundleHost, ClipboardPayload } from "@paged-media/plugin-api";

import { createWorkbookSession, type SheetEngine } from "../src";

/** A fake engine: getRangeValues returns a fixed 2×2 grid of display
 *  strings; setCell + getCellInput are recorded so we can assert the paste
 *  lands cells through the journaled lane. */
function fakeEngine() {
  const setCellCalls: Array<[number, number, number, string]> = [];
  const inputs = new Map<string, string>(); // "row:col" -> input text
  const engine: SheetEngine = {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: (sheet, row, col, input) => {
      setCellCalls.push([sheet, row, col, input]);
      inputs.set(`${row}:${col}`, input);
      return { changed: [{ sheet, row, col, display: input }] };
    },
    getCellDisplay: () => "",
    getCellInput: (_sheet, row, col) => inputs.get(`${row}:${col}`) ?? "",
    sortRange: () => ({ changed: [], edits: [] }),
    findAll: () => [],
    replaceAll: () => ({ occurrences: 0, changed: [], edits: [], skipped: [] }),
    getRangeLowered: () => ({ cols: [], rows: [], rules: { h: [], v: [] }, merges: [] }),
    // The COPY source: a 2×2 grid of formatted display strings.
    getRangeValues: () => [
      ["Name", "Qty"],
      ["Apples", "12"],
    ],
    paginate: () => [],
    getGridScene: () => ({
      viewport: { firstRow: 0, firstCol: 0, rows: 0, cols: 0, xOffsets: [0], yOffsets: [0] },
      cells: [],
      styles: [],
      gridlines: { h: [], v: [] },
      selection: null,
    }),
    setGridSelection() {},
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 4, cols: 4 }],
    listCharts: () => [],
    listFreezePanes: () => [],
    listDataValidations: () => [],
    listComments: () => [],
    listFunctions: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
  return { engine, setCellCalls };
}

/** A host with an in-memory clipboard slot (supports("clipboard@1") true). */
function fakeHost() {
  let slot: ClipboardPayload | null = null;
  const writes: ClipboardPayload[] = [];
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) => f === "clipboard@1",
    clipboard: {
      async read() {
        return slot;
      },
      async write(payload: ClipboardPayload) {
        writes.push(payload);
        slot = payload;
      },
    },
  } as unknown as BundleHost;
  return {
    host,
    writes,
    setClipboard: (p: ClipboardPayload | null) => {
      slot = p;
    },
  };
}

function bootedSession(host: BundleHost, engine: SheetEngine) {
  const session = createWorkbookSession(host);
  const st = session.state();
  st.engine = engine;
  st.activeSheet = 0;
  st.fileName = "demo.xlsx";
  st.selectedRange = "A1:B2";
  return session;
}

describe("sheet_grid_clipboard_copy_range: K-6 copy", () => {
  it("copies the selected range as BOTH a tabular grid and a TSV text fallback", async () => {
    const { engine } = fakeEngine();
    const { host, writes } = fakeHost();
    const session = bootedSession(host, engine);

    // Select a 2×2 range anchored at A1.
    session.setGridSelection(0, 0, 2, 2);
    const r = await session.copySelection();

    expect(r).toEqual({ ok: true, rows: 2, cols: 2 });
    expect(writes).toHaveLength(1);
    expect(writes[0].tabular?.rows).toEqual([
      ["Name", "Qty"],
      ["Apples", "12"],
    ]);
    // The TSV fallback: tabs within a row, newlines between rows.
    expect(writes[0].text).toBe("Name\tQty\nApples\t12");
  });

  it("refuses honestly with no selection", async () => {
    const { engine } = fakeEngine();
    const { host } = fakeHost();
    const session = bootedSession(host, engine);
    const r = await session.copySelection();
    expect(r.ok).toBe(false);
  });
});

describe("sheet_grid_clipboard_paste_range: K-6 paste", () => {
  it("pastes a tabular payload at the anchor through the journaled editCell lane", async () => {
    const { engine, setCellCalls } = fakeEngine();
    const { host, setClipboard } = fakeHost();
    const session = bootedSession(host, engine);

    // A 2×2 grid on the clipboard, anchor at B2 (row1, col1).
    setClipboard({
      tabular: {
        rows: [
          ["a", "b"],
          ["c", "d"],
        ],
      },
    });
    session.setGridSelection(1, 1, 1, 1);
    const r = await session.pasteAtSelection();

    expect(r).toEqual({ ok: true, rows: 2, cols: 2 });
    // Landed at the anchor (1,1) and one cell down/right.
    expect(setCellCalls).toContainEqual([0, 1, 1, "a"]);
    expect(setCellCalls).toContainEqual([0, 1, 2, "b"]);
    expect(setCellCalls).toContainEqual([0, 2, 1, "c"]);
    expect(setCellCalls).toContainEqual([0, 2, 2, "d"]);

    // The whole paste is ONE grouped undo step (ADR-012): undo reverts every
    // pasted cell to its prior input (all "" here) in a single call.
    const before = setCellCalls.length;
    expect(session.undoCellEdit()).toBe(true);
    // 4 cells reverted to "" (their captured prev input).
    expect(setCellCalls.length).toBe(before + 4);
    expect(setCellCalls.slice(before).every(([, , , v]) => v === "")).toBe(true);
    // A second undo finds the journal exhausted (the paste was one step).
    expect(session.undoCellEdit()).toBe(false);
  });

  it("falls back to parsing the TSV text half when there is no tabular grid", async () => {
    const { engine, setCellCalls } = fakeEngine();
    const { host, setClipboard } = fakeHost();
    const session = bootedSession(host, engine);

    // Text-only clipboard (e.g. copied from a plain editor) — parsed as TSV.
    setClipboard({ text: "x\ty\nz\tw" });
    session.setGridSelection(0, 0, 1, 1);
    const r = await session.pasteAtSelection();

    expect(r).toEqual({ ok: true, rows: 2, cols: 2 });
    expect(setCellCalls).toContainEqual([0, 0, 0, "x"]);
    expect(setCellCalls).toContainEqual([0, 1, 1, "w"]);
  });

  it("refuses honestly when the clipboard is empty", async () => {
    const { engine } = fakeEngine();
    const { host } = fakeHost();
    const session = bootedSession(host, engine);
    session.setGridSelection(0, 0, 1, 1);
    const r = await session.pasteAtSelection();
    expect(r.ok).toBe(false);
  });

  it("refuses honestly with no selection (nowhere to anchor)", async () => {
    const { engine } = fakeEngine();
    const { host, setClipboard } = fakeHost();
    const session = bootedSession(host, engine);
    setClipboard({ tabular: { rows: [["a"]] } });
    const r = await session.pasteAtSelection();
    expect(r.ok).toBe(false);
  });
});
