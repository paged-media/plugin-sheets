// sheet.plugin.engine.boot: the camelCase facade maps 1:1 onto the
// snake_case wasm surface; boot is exercised on BOTH sides of the
// artifact gate (honest rejection when unbuilt, real boot when built).

import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import type {
  ChartGeometry,
  GridScene,
  LoweredContent,
} from "@paged-media/sheet-host-model";

import {
  ENGINE_NOT_BUILT,
  bootEngine,
  wrapEngine,
  type SheetWasmEngine,
} from "../src";

/** A fake wasm engine recording the snake_case calls the facade makes. */
function fakeWasm() {
  const calls: Array<{ method: string; args: unknown[] }> = [];
  const lowered: LoweredContent = {
    cols: [{ index: 0, widthPt: 40 }],
    rows: [{ index: 0, heightPt: 16, cells: [{ col: 0, text: "x", align: "left" }] }],
    rules: { h: [], v: [] },
    merges: [],
  };
  const scene: GridScene = {
    viewport: { firstRow: 0, firstCol: 0, rows: 1, cols: 1, xOffsets: [0, 40], yOffsets: [0, 16] },
    cells: [{ row: 0, col: 0, text: "x", align: "left", styleKey: 0 }],
    styles: [],
    gridlines: { h: [], v: [] },
    selection: null,
  };
  const geometry: ChartGeometry = {
    widthPt: 120,
    heightPt: 90,
    prims: [
      { kind: "rect", x: 0, y: 0, w: 10, h: 20, fill: "#4E79A7", stroke: null, strokeW: 0 },
    ],
  };
  const wasm: SheetWasmEngine = {
    load_xlsx(bytes) {
      calls.push({ method: "load_xlsx", args: [bytes] });
    },
    save_xlsx() {
      calls.push({ method: "save_xlsx", args: [] });
      return new Uint8Array([1, 2, 3]);
    },
    set_cell(sheet, row, col, inputStr) {
      calls.push({ method: "set_cell", args: [sheet, row, col, inputStr] });
      return { changed: [{ sheet, row, col, display: inputStr }] };
    },
    get_cell_display(sheet, row, col) {
      calls.push({ method: "get_cell_display", args: [sheet, row, col] });
      return "42";
    },
    get_range_lowered(sheet, range, opts) {
      calls.push({ method: "get_range_lowered", args: [sheet, range, opts] });
      return lowered;
    },
    paginate(sheet, range, frames, opts) {
      calls.push({ method: "paginate", args: [sheet, range, frames, opts] });
      return [
        { frameIndex: 0, content: lowered, continued: false, oversize: false },
      ];
    },
    get_grid_scene(sheet, firstRow, firstCol, wPt, hPt, opts) {
      calls.push({
        method: "get_grid_scene",
        args: [sheet, firstRow, firstCol, wPt, hPt, opts],
      });
      return scene;
    },
    set_grid_selection(sheet, anchorRow, anchorCol, rows, cols) {
      calls.push({
        method: "set_grid_selection",
        args: [sheet, anchorRow, anchorCol, rows, cols],
      });
    },
    list_sheets() {
      calls.push({ method: "list_sheets", args: [] });
      return [{ id: 0, name: "Sheet1", rows: 10, cols: 4 }];
    },
    list_charts() {
      calls.push({ method: "list_charts", args: [] });
      return [
        {
          index: 0,
          hostSheet: 0,
          kind: "column",
          title: "Q1",
          seriesCount: 1,
        },
      ];
    },
    get_chart_geometry(index, wPt, hPt) {
      calls.push({ method: "get_chart_geometry", args: [index, wPt, hPt] });
      return geometry;
    },
    free() {
      calls.push({ method: "free", args: [] });
    },
  };
  return { wasm, calls, lowered, scene, geometry };
}

describe("sheet_plugin_engine_boot: facade mapping", () => {
  it("forwards every camelCase method to its snake_case wasm twin", () => {
    const { wasm, calls, lowered, scene, geometry } = fakeWasm();
    const engine = wrapEngine(wasm);

    const bytes = new Uint8Array([9]);
    engine.loadXlsx(bytes);
    expect(engine.saveXlsx()).toEqual(new Uint8Array([1, 2, 3]));
    expect(engine.setCell(0, 1, 2, "=A1")).toEqual({
      changed: [{ sheet: 0, row: 1, col: 2, display: "=A1" }],
    });
    expect(engine.getCellDisplay(0, 1, 2)).toBe("42");
    expect(engine.getRangeLowered(0, "A1:B2", { includeGridRules: true })).toBe(
      lowered,
    );
    expect(
      engine.paginate(
        0,
        "A1:B9",
        [{ widthPt: 100, heightPt: 50 }],
        { continuedMarker: true },
      ),
    ).toEqual([
      { frameIndex: 0, content: lowered, continued: false, oversize: false },
    ]);
    expect(
      engine.getGridScene(0, 0, 0, 480, 320, { includeGridlines: true }),
    ).toBe(scene);
    engine.setGridSelection(0, 1, 2, 3, 4);
    expect(engine.listSheets()).toEqual([
      { id: 0, name: "Sheet1", rows: 10, cols: 4 },
    ]);
    expect(engine.listCharts()).toEqual([
      { index: 0, hostSheet: 0, kind: "column", title: "Q1", seriesCount: 1 },
    ]);
    expect(engine.getChartGeometry(0, 360, 240)).toBe(geometry);
    engine.dispose();

    expect(calls.map((c) => c.method)).toEqual([
      "load_xlsx",
      "save_xlsx",
      "set_cell",
      "get_cell_display",
      "get_range_lowered",
      "paginate",
      "get_grid_scene",
      "set_grid_selection",
      "list_sheets",
      "list_charts",
      "get_chart_geometry",
      "free",
    ]);
    // argument fidelity through the facade.
    expect(calls[0].args[0]).toBe(bytes);
    expect(calls[4].args).toEqual([0, "A1:B2", { includeGridRules: true }]);
    expect(calls[5].args).toEqual([
      0,
      "A1:B9",
      [{ widthPt: 100, heightPt: 50 }],
      { continuedMarker: true },
    ]); // paginate
    expect(calls[6].args).toEqual([0, 0, 0, 480, 320, { includeGridlines: true }]);
    expect(calls[7].args).toEqual([0, 1, 2, 3, 4]);
    expect(calls[10].args).toEqual([0, 360, 240]); // get_chart_geometry
  });

  it("dispose maps to free()", () => {
    const { wasm, calls } = fakeWasm();
    wrapEngine(wasm).dispose();
    expect(calls.map((c) => c.method)).toEqual(["free"]);
  });
});

// The two boot paths are environment twins, each gated on the artifact:
// here the honest FAILURE when bin/ is unbuilt; engine-real.spec.ts the
// real boot when scripts/build-wasm.sh has produced it.
const artifactBuilt = existsSync(
  join(dirname(fileURLToPath(import.meta.url)), "..", "bin", "sheet_js_bg.wasm"),
);

describe.skipIf(artifactBuilt)("sheet_plugin_engine_boot: boot-failure path (S-10)", () => {
  it("rejects with the 'not built' message when the artifact is absent", async () => {
    // bin/sheet_js.js is absent until scripts/build-wasm.sh runs — the
    // dynamic import fails and bootEngine surfaces it honestly.
    await expect(bootEngine()).rejects.toThrow(ENGINE_NOT_BUILT);
  });
});

describe.skipIf(!artifactBuilt)("sheet_plugin_engine_boot: built-artifact boot", () => {
  it("resolves the facade over the real wasm", async () => {
    const engine = await bootEngine();
    expect(engine.listSheets()).toEqual([{ id: 0, name: "Sheet1", rows: 0, cols: 0 }]);
    engine.dispose();
  });
});
