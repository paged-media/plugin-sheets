// The typed engine FACADE + boot. The Rust wasm (sheet-js) does ALL the
// spreadsheet work; this is a thin TS shape over its snake_case
// wasm-bindgen methods (CLAUDE.md hard rule: no Excel-like operation in
// TS — every method here forwards to a wasm call). The facade exists so
// the rest of the bundle codes against a stable camelCase contract and so
// the not-yet-built artifact can be stubbed in tests.
//
// BOOT (S-10). The artifact is the wasm-bindgen `--target web` glue
// (`bin/sheet_js.js` + `bin/sheet_js_bg.wasm`, produced by
// scripts/build-wasm.sh — lands in Phase 2). We DON'T use the host's
// `loadBundleWasm` (it instantiates a RAW module — no wbindgen imports;
// S-10), we load the glue in the bundle realm exactly like
// @paged-media/canvas-wasm does, branching browser vs Node the way
// plugin-sdk's wasm-loader.ts does:
//   · browser: `default(new URL("./sheet_js_bg.wasm", import.meta.url))`
//   · Node:    `node:fs` readFile the bytes, `initSync({ module: bytes })`
// Until the artifact exists the dynamic import REJECTS — bootEngine
// surfaces that honestly so the panel can say "engine wasm not built".

import type {
  ChartGeometry,
  GridScene,
  LoweredContent,
} from "@paged-media/sheet-host-model";

// ------------------------------------------------------------ facade

/** One worksheet's identity + dimensions. `rows`/`cols` are the used
 *  extent (the engine computes it) so the panel can default the range
 *  input without a separate call. */
export interface SheetInfo {
  id: number;
  name: string;
  rows: number;
  cols: number;
}

/** One changed cell after an edit — the engine returns the recomputed
 *  DISPLAY string (already number-formatted in Rust, spec §9). */
export interface CellChange {
  sheet: number;
  row: number;
  col: number;
  display: string;
}

/** Options the lowering pass honours (forwarded verbatim to wasm). */
export interface LowerOptions {
  includeGridRules?: boolean;
  headerRows?: number;
}

/** Options the grid-scene windowing honours (forwarded verbatim to wasm —
 *  the engine windows in Rust, spec §8.1). */
export interface GridSceneOptions {
  includeGridlines?: boolean;
}

/** One chart in the workbook (M2 charts track, spec §8.4) — the engine's
 *  parsed-chart summary for the panel's chart list. `index` is the handle
 *  `getChartGeometry` takes. */
export interface ChartInfo {
  index: number;
  hostSheet: number;
  /** The lowercase kind tag (`"column"`, `"bar"`, `"line"`, `"area"`,
   *  `"pie"`, `"donut"`, `"scatter"`). */
  kind: string;
  title: string | null;
  seriesCount: number;
}

/** The stable engine contract the bundle codes against. Every method is
 *  a forward to the wasm surface; the facade only renames + shapes. */
export interface SheetEngine {
  /** Parse XLSX bytes into the in-memory model (preservation-first; the
   *  bytes round-trip on save — spec §10). */
  loadXlsx(bytes: Uint8Array): void;
  /** Re-emit the workbook as XLSX bytes (lazy-verbatim preservation). */
  saveXlsx(): Uint8Array;
  /** Commit one cell input (value or formula); returns every cell whose
   *  DISPLAY changed (the dirty cut, recomputed in Rust). */
  setCell(
    sheet: number,
    row: number,
    col: number,
    input: string,
  ): { changed: CellChange[] };
  /** The current formatted display of one cell. */
  getCellDisplay(sheet: number, row: number, col: number): string;
  /** Lower a range to the IR the host-model translator consumes (spec
   *  §8.2). All geometry/formatting decided in Rust. */
  getRangeLowered(
    sheet: number,
    range: string,
    opts?: LowerOptions,
  ): LoweredContent;
  /** Window a sheet into a [`GridScene`] for the sheets-mode grid surface
   *  (spec §8.1, S-02). The engine windows from the `(firstRow, firstCol)`
   *  scroll origin bounded by `(wPt, hPt)` and materializes only visible
   *  populated cells in Rust; the panel only paints the result. The wasm
   *  side (`get_grid_scene`) lands in the JOINS phase; the facade maps it
   *  now so the panel codes against a stable contract. */
  getGridScene(
    sheet: number,
    firstRow: number,
    firstCol: number,
    wPt: number,
    hPt: number,
    opts?: GridSceneOptions,
  ): GridScene;
  /** Record the sheets-mode selection rectangle in the engine model so the
   *  next [`getGridScene`] carries it (spec §8.1 — selection is engine
   *  state, the panel only requests it). `setGridSelection` forwards to
   *  wasm; the wasm side lands in JOINS. */
  setGridSelection(
    sheet: number,
    anchorRow: number,
    anchorCol: number,
    rows: number,
    cols: number,
  ): void;
  /** Enumerate the workbook's sheets (id, name, used extent). */
  listSheets(): SheetInfo[];
  /** Enumerate the workbook's charts (M2 charts track, spec §8.4). Parsed
   *  from the XLSX chart parts on load; empty for a chartless workbook. */
  listCharts(): ChartInfo[];
  /** Resolve chart `index`'s series ranges against the live model and
   *  generate its geometry IR for a `wPt × hPt` content box (spec §8.4 —
   *  live to recalc). The IR feeds BOTH the page paged.draw lowering and the
   *  grid view (one generator, two projections). */
  getChartGeometry(index: number, wPt: number, hPt: number): ChartGeometry;
  /** Release the wasm-held model. */
  dispose(): void;
}

// ---------------------------------------------------- wasm surface shape

/** The snake_case wasm-bindgen surface (sheet-js). A structural subset —
 *  only the members the bundle drives. The artifact lands in Phase 2;
 *  this shape is the contract the facade maps over. */
export interface SheetWasmEngine {
  load_xlsx(bytes: Uint8Array): void;
  save_xlsx(): Uint8Array;
  set_cell(
    sheet: number,
    row: number,
    col: number,
    input: string,
  ): { changed: CellChange[] };
  get_cell_display(sheet: number, row: number, col: number): string;
  get_range_lowered(
    sheet: number,
    range: string,
    opts?: LowerOptions,
  ): LoweredContent;
  get_grid_scene(
    sheet: number,
    first_row: number,
    first_col: number,
    w_pt: number,
    h_pt: number,
    opts?: GridSceneOptions,
  ): GridScene;
  set_grid_selection(
    sheet: number,
    anchor_row: number,
    anchor_col: number,
    rows: number,
    cols: number,
  ): void;
  list_sheets(): SheetInfo[];
  list_charts(): ChartInfo[];
  get_chart_geometry(index: number, w_pt: number, h_pt: number): ChartGeometry;
  free(): void;
}

/** The module shape the wasm-bindgen `--target web` glue exports. */
export interface SheetWasmModule {
  /** Browser init: fetch + instantiate from a URL (or accept bytes). */
  default(input?: unknown): Promise<unknown>;
  /** Node init: synchronous instantiate from raw bytes / a Module. */
  initSync(module: { module: BufferSource | WebAssembly.Module }): unknown;
  /** The engine constructor (the wasm class wraps the SheetModel). */
  SheetEngine: new () => SheetWasmEngine;
}

// ----------------------------------------------------------- the facade

/** Wrap a booted wasm engine in the camelCase facade. Split out so the
 *  mapping is unit-testable over a fake wasm object (no real wasm). */
export function wrapEngine(wasm: SheetWasmEngine): SheetEngine {
  return {
    loadXlsx: (bytes) => wasm.load_xlsx(bytes),
    saveXlsx: () => wasm.save_xlsx(),
    setCell: (sheet, row, col, input) =>
      wasm.set_cell(sheet, row, col, input),
    getCellDisplay: (sheet, row, col) =>
      wasm.get_cell_display(sheet, row, col),
    getRangeLowered: (sheet, range, opts) =>
      wasm.get_range_lowered(sheet, range, opts),
    getGridScene: (sheet, firstRow, firstCol, wPt, hPt, opts) =>
      wasm.get_grid_scene(sheet, firstRow, firstCol, wPt, hPt, opts),
    setGridSelection: (sheet, anchorRow, anchorCol, rows, cols) =>
      wasm.set_grid_selection(sheet, anchorRow, anchorCol, rows, cols),
    listSheets: () => wasm.list_sheets(),
    listCharts: () => wasm.list_charts(),
    getChartGeometry: (index, wPt, hPt) =>
      wasm.get_chart_geometry(index, wPt, hPt),
    dispose: () => wasm.free(),
  };
}

// ------------------------------------------------------------- the boot

/** Error message when the artifact is absent (the honest seam — S-10:
 *  the wasm lands in Phase 2 via scripts/build-wasm.sh). */
export const ENGINE_NOT_BUILT =
  "engine wasm not built — run scripts/build-wasm.sh";

/** Is this a Node-like (non-browser) realm? `window`/`document` absent. */
function isNode(): boolean {
  return (
    typeof process !== "undefined" &&
    process.versions != null &&
    process.versions.node != null &&
    typeof (globalThis as { window?: unknown }).window === "undefined"
  );
}

/** Load + boot the engine wasm, returning the facade. Browser path
 *  passes the `_bg.wasm` URL to `default()`; Node path reads the bytes
 *  and `initSync`s (mirroring plugin-sdk's wasm-loader.ts). Rejects with
 *  ENGINE_NOT_BUILT-flavoured detail when the artifact is missing so the
 *  panel can surface the honest "not built" state. */
export async function bootEngine(): Promise<SheetEngine> {
  let mod: SheetWasmModule;
  try {
    // @ts-ignore — the artifact (bin/sheet_js.js, the wasm-bindgen
    // --target web glue) is produced by scripts/build-wasm.sh in Phase 2
    // and is intentionally absent from the source tree; the dynamic
    // import resolves at runtime once built. Typed via SheetWasmModule.
    mod = (await import("../bin/sheet_js.js")) as SheetWasmModule;
  } catch (cause) {
    throw new Error(ENGINE_NOT_BUILT, { cause });
  }

  if (isNode()) {
    const { readFile } = await import("node:fs/promises");
    const { fileURLToPath } = await import("node:url");
    const wasmPath = fileURLToPath(
      new URL("../bin/sheet_js_bg.wasm", import.meta.url),
    );
    const bytes = await readFile(wasmPath);
    mod.initSync({
      module: new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength),
    });
  } else {
    await mod.default(new URL("./sheet_js_bg.wasm", import.meta.url));
  }

  return wrapEngine(new mod.SheetEngine());
}
