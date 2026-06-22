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

// sheet.plugin.cell-style.from-cell (session glue): "new cell style from
// selection" composes existing platform doors over the LAST-LOWERED native
// table — read the cell's properties (elementProperties), mint + populate the
// style (createCellStyle + setStyleProperty), then ATTEMPT the apply-back
// (setElementProperty{appliedCellStyle}). The honest residual: the apply-back
// is wire-shape-only (UnsupportedProperty), so a host that rejects it is the
// expected path and the session reports applied=false. The pure mint/populate
// plan is pinned in sheet-host-model/test/cell-style.spec.ts; this exercises
// the routing (grid coords → lowered tableCell), the mutation sequence, and
// the honest outcome.

import { describe, expect, it } from "vitest";

import type {
  BundleHost,
  ElementId,
  Mutation,
  MutationOutcome,
} from "@paged-media/plugin-api";

import {
  createWorkbookSession,
  type SheetEngine,
  type WorkbookSession,
} from "../src";

// A 2-col × 2-row lowered IR (model rows 0..1, model cols 0 & 2 — sparse, so
// model col 2 maps to table col 1, proving the mapping is real).
const LOWERED = {
  cols: [
    { index: 0, widthPt: 40 },
    { index: 2, widthPt: 50 },
  ],
  rows: [
    { index: 0, heightPt: 18, cells: [{ col: 0, text: "A", align: "left" as const }] },
    { index: 1, heightPt: 18, cells: [{ col: 2, text: "B", align: "left" as const }] },
  ],
  rules: { h: [], v: [] },
  merges: [],
};

function fakeEngine(): SheetEngine {
  return {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: (sheet, row, col, input) => ({ changed: [{ sheet, row, col, display: input }] }),
    getCellDisplay: () => "",
    getCellInput: () => "",
    sortRange: () => ({ changed: [], edits: [] }),
    findAll: () => [],
    replaceAll: () => ({ occurrences: 0, changed: [], edits: [], skipped: [] }),
    getRangeLowered: () => LOWERED,
    getRangeValues: () => [],
    paginate: () => [],
    getGridScene: () => ({
      viewport: { firstRow: 0, firstCol: 0, rows: 1, cols: 1, xOffsets: [0, 40], yOffsets: [0, 18] },
      cells: [],
      styles: [],
      gridlines: { h: [], v: [] },
      selection: null,
    }),
    setGridSelection() {},
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 2, cols: 3 }],
    listCharts: () => [],
    listFreezePanes: () => [],
    listDataValidations: () => [],
    listComments: () => [],
    listFunctions: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
}

/** A fake host that records mutations + answers elementProperties. The
 *  apply-back (`setElementProperty{appliedCellStyle}`) is REJECTED to model
 *  the wire-shape-only residual; everything else applies. */
function fakeHost(opts?: { rejectApply?: boolean; cellEntries?: unknown[] }) {
  const reject = opts?.rejectApply ?? true;
  const entries =
    opts?.cellEntries ??
    [
      { path: "cellFillColor", value: { type: "colorRef", value: "FFCC00" } },
      { path: "cellTopEdgeStrokeWeight", value: { type: "length", value: 0.5 } },
    ];
  const mutations: Mutation[] = [];
  let frameInserted = false;
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: () => false,
    document: {
      async meta() {
        return { activePage: "Page/u1" } as never;
      },
      async collection(name: string) {
        if (name === "stories" && frameInserted) return [{ selfId: "Story/s1" }] as never;
        return [] as never;
      },
      async elementProperties(id: ElementId) {
        return { id, kind: "tableCell", entries } as never;
      },
      async mutate(m: Mutation): Promise<MutationOutcome> {
        mutations.push(m);
        if (m.op === "batch") {
          const ops = (m as { args: { ops: Array<{ op: string }> } }).args.ops;
          if (ops.some((o) => o.op === "insertTextFrame")) {
            frameInserted = true;
            return {
              applied: true,
              createdId: { kind: "textFrame", id: "frame1" } as ElementId,
              pageIds: ["Page/u1"],
            };
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
        if (m.op === "setElementProperty") {
          const path = (m as { args: { path: string } }).args.path;
          if (path === "appliedCellStyle" && reject) {
            return { applied: false, error: "UnsupportedProperty" };
          }
        }
        return { applied: true, createdId: null, pageIds: ["Page/u1"] };
      },
      async hitTest() {
        return { storyId: "Story/s1", frameId: "frame1" } as never;
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
  return { host, mutations };
}

async function loweredSession(
  host: BundleHost,
): Promise<WorkbookSession> {
  const engine = fakeEngine();
  const session = createWorkbookSession(host);
  const st = session.state();
  st.engine = engine;
  st.activeSheet = 0;
  st.selectedRange = "A1:C2";
  // Lower the range so the session records lastLoweredTable. Re-use the same
  // engine the session holds (lowerSelectionToFrame reads getRangeLowered).
  await session.lowerSelection();
  return session;
}

describe("sheet_plugin_cell_style_from_cell: session glue", () => {
  it("requires a lowered table — refuses honestly before any lowering", async () => {
    const { host } = fakeHost();
    const session = createWorkbookSession(host);
    const st = session.state();
    st.engine = fakeEngine();
    st.activeSheet = 0;
    session.setGridSelection(0, 0, 1, 1);
    const res = await session.newCellStyleFromSelection("X");
    expect(res.ok).toBe(false);
    if (!res.ok) expect(res.message).toMatch(/lower a range/);
  });

  it("requires a selection — refuses when no cell is selected", async () => {
    const { host } = fakeHost();
    const session = await loweredSession(host);
    const res = await session.newCellStyleFromSelection("X");
    expect(res.ok).toBe(false);
    if (!res.ok) expect(res.message).toMatch(/select a cell/);
  });

  it("mints + populates from the selected cell, reports the honest residual", async () => {
    const { host, mutations } = fakeHost({ rejectApply: true });
    const session = await loweredSession(host);
    // Select model cell (row 1, col 2) → table position (row 1, col 1).
    session.setGridSelection(1, 2, 1, 1);
    mutations.length = 0; // ignore the lowering mutations; focus on the style ops

    const res = await session.newCellStyleFromSelection("Highlight");
    expect(res.ok).toBe(true);
    if (!res.ok) return;

    // Mint, then populate the two cell-appearance props, then attempt apply.
    const create = mutations.find((m) => m.op === "createCellStyle") as {
      args: { selfId: string; name: string };
    };
    expect(create.args.name).toBe("Highlight");
    expect(create.args.selfId).toBe(res.styleId);

    const props = mutations.filter((m) => m.op === "setStyleProperty") as Array<{
      args: { collection: string; styleId: string; path: string };
    }>;
    expect(props).toHaveLength(2);
    expect(props.every((p) => p.args.collection === "cell")).toBe(true);
    expect(props.every((p) => p.args.styleId === res.styleId)).toBe(true);
    expect(props.map((p) => p.args.path).sort()).toEqual([
      "cellFillColor",
      "cellTopEdgeStrokeWeight",
    ]);
    expect(res.capturedCount).toBe(2);

    // The apply-back targets the MAPPED table cell (model col 2 → table col 1).
    const apply = mutations.find(
      (m) =>
        m.op === "setElementProperty" &&
        (m as { args: { path: string } }).args.path === "appliedCellStyle",
    ) as { args: { elementId: ElementId } };
    expect(apply.args.elementId).toEqual({
      kind: "tableCell",
      id: { story_id: "Story/s1", table_id: "table1", row: 1, col: 1 },
    });

    // The honest residual: applied=false (the host rejected appliedCellStyle).
    expect(res.applied).toBe(false);
    expect(res.applyMessage).toMatch(/appliedCellStyle/);
  });

  it("reports applied=true when the host accepts the apply-back", async () => {
    const { host } = fakeHost({ rejectApply: false });
    const session = await loweredSession(host);
    session.setGridSelection(0, 0, 1, 1);
    const res = await session.newCellStyleFromSelection("OK");
    expect(res.ok).toBe(true);
    if (res.ok) expect(res.applied).toBe(true);
  });

  it("falls back to the table's first cell for an out-of-range selection", async () => {
    const { host, mutations } = fakeHost({ rejectApply: true });
    const session = await loweredSession(host);
    // Select model (row 99, col 99) — outside the lowered range.
    session.setGridSelection(99, 99, 1, 1);
    mutations.length = 0;
    const res = await session.newCellStyleFromSelection("Edge");
    expect(res.ok).toBe(true);
    if (!res.ok) return;
    const apply = mutations.find(
      (m) =>
        m.op === "setElementProperty" &&
        (m as { args: { path: string } }).args.path === "appliedCellStyle",
    ) as { args: { elementId: ElementId } };
    // First cell of the table (row 0, col 0).
    expect(apply.args.elementId).toEqual({
      kind: "tableCell",
      id: { story_id: "Story/s1", table_id: "table1", row: 0, col: 0 },
    });
    expect(res.applyMessage).toMatch(/first cell|appliedCellStyle/);
  });
});
