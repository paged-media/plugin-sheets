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

// sheet.chart.kind-set + sheet.chart.series-orientation — the SESSION half
// of the Illustrator §16.4 graphs row. Two contracts, no wasm:
//
//  1. The chart-kind vocabulary is READ from the engine. The workbook panel
//     renders `session.chartKinds()`, so if this ever fell back to a literal
//     list the panel could offer a kind the engine refuses (and the Rust-owns-
//     the-semantics rule would be broken in the one place users can see).
//  2. The transpose control reaches the engine, and an OMITTED orientation
//     means "columns" — the pre-transpose meaning of the same call.

import { describe, expect, it } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";

import { createWorkbookSession, type SheetEngine } from "../src";

/** Records every addChart call and serves a fixed kind list. `chartKinds`
 *  can be made to throw, to prove the session degrades rather than taking
 *  the panel down with it. */
function recordingEngine(opts: { kindsThrow?: boolean } = {}) {
  const authored: unknown[][] = [];
  const engine = {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: () => ({ changed: [] }),
    getCellDisplay: () => "",
    getCellInput: () => "",
    sortRange: () => ({ changed: [], edits: [] }),
    findAll: () => [],
    replaceAll: () => ({ occurrences: 0, changed: [], edits: [], skipped: [] }),
    getRangeLowered: () => ({
      cols: [],
      rows: [],
      rules: { h: [], v: [] },
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
      viewport: {
        firstRow: 0,
        firstCol: 0,
        rows: 0,
        cols: 0,
        xOffsets: [0],
        yOffsets: [0],
      },
      cells: [],
      styles: [],
      gridlines: { h: [], v: [] },
      selection: null,
    }),
    setGridSelection() {},
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 4, cols: 3 }],
    listCharts: () => [],
    chartKinds: () => {
      if (opts.kindsThrow) throw new Error("engine gone");
      return ["column", "stackedColumn", "bar", "stackedBar", "radar"];
    },
    addChart: (...args: unknown[]) => {
      authored.push(args);
      return authored.length - 1;
    },
    listFreezePanes: () => [],
    listDataValidations: () => [],
    listComments: () => [],
    listFunctions: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  } as unknown as SheetEngine;
  return { engine, authored };
}

const silentHost = {
  log: { debug() {}, info() {}, warn() {}, error() {} },
  supports: () => false,
} as unknown as BundleHost;

function booted(opts: { kindsThrow?: boolean } = {}) {
  const { engine, authored } = recordingEngine(opts);
  const session = createWorkbookSession(silentHost);
  const st = session.state();
  st.engine = engine;
  st.activeSheet = 0;
  return { session, authored };
}

describe("sheet_chart_kind_set: the panel reads the engine's vocabulary", () => {
  it("serves the engine's kind list verbatim", () => {
    const { session } = booted();
    expect(session.chartKinds()).toEqual([
      "column",
      "stackedColumn",
      "bar",
      "stackedBar",
      "radar",
    ]);
  });

  it("is empty with no workbook, and degrades when the engine throws", () => {
    // No engine at all: an empty <select>, not a crash on panel mount.
    const fresh = createWorkbookSession(silentHost);
    expect(fresh.chartKinds()).toEqual([]);
    // Engine present but refusing: same degradation, warn-only.
    const { session } = booted({ kindsThrow: true });
    expect(session.chartKinds()).toEqual([]);
  });
});

describe("sheet_chart_series_orientation: transpose reaches the engine", () => {
  it("forwards the chosen orientation and defaults to columns", () => {
    const { session, authored } = booted();
    expect(session.authorChart("B2:C4", "A2:A4", "radar", "T", "rows")).toEqual({
      ok: true,
      index: 0,
    });
    // The transpose control is the LAST argument; the active sheet the first.
    expect(authored[0]).toEqual([0, "B2:C4", "A2:A4", "radar", "T", "rows"]);

    // Omitted => "columns" (the pre-transpose meaning of the same call).
    session.authorChart("B2:C4", "", "column", "");
    expect(authored[1]).toEqual([0, "B2:C4", "", "column", "", "columns"]);
  });

  it("reports the engine's refusal instead of guessing", () => {
    const { session } = booted();
    const st = session.state();
    st.engine = {
      ...(st.engine as SheetEngine),
      addChart: () => {
        throw new Error('unknown series orientation "diagonal" (columns|rows)');
      },
    } as SheetEngine;
    const res = session.authorChart("B2:C4", "", "column", "", "diagonal");
    expect(res.ok).toBe(false);
    expect(res.ok === false && res.message).toMatch(/orientation/);
  });
});
