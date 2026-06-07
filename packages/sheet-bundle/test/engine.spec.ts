// sheet.plugin.engine.boot (the facade half — boot itself stays planned
// until the real wasm lands in Phase 2): the camelCase facade maps 1:1
// onto the snake_case wasm surface, and bootEngine rejects honestly when
// the artifact is absent.

import { describe, expect, it } from "vitest";

import type { LoweredContent } from "@paged-media/sheet-host-model";

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
    list_sheets() {
      calls.push({ method: "list_sheets", args: [] });
      return [{ id: 0, name: "Sheet1", rows: 10, cols: 4 }];
    },
    free() {
      calls.push({ method: "free", args: [] });
    },
  };
  return { wasm, calls, lowered };
}

describe("sheet_plugin_engine_boot: facade mapping", () => {
  it("forwards every camelCase method to its snake_case wasm twin", () => {
    const { wasm, calls, lowered } = fakeWasm();
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
    expect(engine.listSheets()).toEqual([
      { id: 0, name: "Sheet1", rows: 10, cols: 4 },
    ]);
    engine.dispose();

    expect(calls.map((c) => c.method)).toEqual([
      "load_xlsx",
      "save_xlsx",
      "set_cell",
      "get_cell_display",
      "get_range_lowered",
      "list_sheets",
      "free",
    ]);
    // argument fidelity through the facade.
    expect(calls[0].args[0]).toBe(bytes);
    expect(calls[4].args).toEqual([0, "A1:B2", { includeGridRules: true }]);
  });

  it("dispose maps to free()", () => {
    const { wasm, calls } = fakeWasm();
    wrapEngine(wasm).dispose();
    expect(calls.map((c) => c.method)).toEqual(["free"]);
  });
});

describe("sheet_plugin_engine_boot: boot-failure path (S-10)", () => {
  it("rejects with the 'not built' message when the artifact is absent", async () => {
    // The artifact (bin/sheet_js.js) is intentionally not in the tree
    // (Phase 2 builds it) — the dynamic import fails, surfacing honestly.
    await expect(bootEngine()).rejects.toThrow(ENGINE_NOT_BUILT);
  });
});
