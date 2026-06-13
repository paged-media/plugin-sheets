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
  Page,
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
 *  the engine windows in Rust, spec §8.1). `freezeRows`/`freezeCols` OVERRIDE
 *  the workbook's stored frozen-pane split for this scene; omit them to use
 *  the workbook's own `<sheetViews><pane>` split (the engine reads it). */
export interface GridSceneOptions {
  includeGridlines?: boolean;
  freezeRows?: number;
  freezeCols?: number;
}

/** One worksheet's frozen-pane split (spec §8.1) — read-only derived state
 *  parsed from the workbook's `<sheetViews><pane>` (which still round-trips
 *  byte-identical). Only sheets WITH a frozen pane are reported. */
export interface FreezeInfo {
  sheet: number;
  rows: number;
  cols: number;
}

/** One frame's content box for pagination (the host chain link's content
 *  box; Wave 2D / S-05). Only height bounds the split; width rides along.
 *  Forwarded verbatim to wasm. */
export interface FrameBox {
  widthPt: number;
  heightPt: number;
}

/** Options the pagination pass honours (forwarded verbatim to wasm — the
 *  threading math lives in Rust, spec §8.2). */
export interface PaginateOptions {
  /** Leading rows re-emitted at the top of every continuation frame. */
  repeatedHeaderRows?: number;
  /** Append a continued-from marker to frames followed by more body rows. */
  continuedMarker?: boolean;
  /** Range-relative `[firstRow, lastRow]` blocks never split across a break. */
  keepRowsTogether?: [number, number][];
}

/** One cell rewritten by a bulk edit op (sort / replace) — BOTH
 *  re-enterable INPUT texts (the ADR-012 journal's inverse pair), straight
 *  from the engine so the session can journal the whole op as one grouped
 *  undo step. */
export interface CellEditRecord {
  sheet: number;
  row: number;
  col: number;
  prevInput: string;
  nextInput: string;
}

/** The engine's range-sort result: recomputed displays + the per-cell
 *  input rewrites (journal lane). All sort semantics (stable order, typed
 *  ranks, blanks-last, the formula-refusal boundary) are decided in Rust. */
export interface SortResult {
  changed: CellChange[];
  edits: CellEditRecord[];
}

/** Options for find/replace (forwarded verbatim to wasm; matching/collation
 *  semantics are decided in Rust — documented on the registry rows). */
export interface FindOptions {
  matchCase?: boolean;
  entireCell?: boolean;
  inFormulas?: boolean;
}

/** One find hit: the address + a truncated excerpt of the matched text
 *  (display or input per `inFormulas`). */
export interface FindMatch {
  sheet: number;
  row: number;
  col: number;
  excerpt: string;
}

/** One cell replace_all matched but did not rewrite (parse-failed
 *  replacement or engine-owned spill output) — reported, never corrupted. */
export interface SkippedCell {
  sheet: number;
  row: number;
  col: number;
  reason: string;
}

/** The engine's replace-all result: spliced-occurrence count, recomputed
 *  displays, the per-cell input rewrites (journal lane), and the skip
 *  report. */
export interface ReplaceResult {
  occurrences: number;
  changed: CellChange[];
  edits: CellEditRecord[];
  skipped: SkippedCell[];
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

/** One registered function for the formula-bar autocomplete (S-04). The
 *  list is the ENGINE's registry-generated name table (constitution §7 —
 *  the completion names are the engine's truth, NEVER a hand-kept TS list).
 *  `maxArgs` null = variadic. Only implemented functions appear. */
export interface FunctionInfo {
  name: string;
  family: string;
  minArgs: number;
  maxArgs: number | null;
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
  /** The cell's re-enterable INPUT text (`"=…"` for a formula cell; `""`
   *  for empty/OOB) — the ADR-012 undo journal's faithful inverse (the
   *  display is NOT re-enterable for formula cells). */
  getCellInput(sheet: number, row: number, col: number): string;
  /** Stable sort of a range's rows by a key column (0-based, RELATIVE to
   *  the range). VALUES-ONLY ranges sort fully; a range containing formula
   *  cells THROWS the honest boundary error ("sort over formulas not yet
   *  supported") — all semantics in Rust (sheet.edit.sort.*). */
  sortRange(
    sheet: number,
    range: string,
    keyCol: number,
    ascending: boolean,
    hasHeader: boolean,
  ): SortResult;
  /** Find every populated cell matching `needle`; `sheet` scopes to one
   *  sheet, `undefined` scans the whole workbook (sheet.edit.find.*). */
  findAll(
    sheet: number | undefined,
    needle: string,
    opts?: FindOptions,
  ): FindMatch[];
  /** Replace every occurrence over the scope, operating on cell INPUT
   *  texts via the normal set-cell lane; parse-failing replacements are
   *  SKIPPED + reported, never half-applied (sheet.edit.replace.*). */
  replaceAll(
    sheet: number | undefined,
    needle: string,
    replacement: string,
    opts?: FindOptions,
  ): ReplaceResult;
  /** Lower a range to the IR the host-model translator consumes (spec
   *  §8.2). All geometry/formatting decided in Rust. */
  getRangeLowered(
    sheet: number,
    range: string,
    opts?: LowerOptions,
  ): LoweredContent;
  /** Read a range (`"A1:D9"` or `"A1"`) as a RECTANGULAR grid of formatted
   *  DISPLAY strings — the K-6 / S-14 clipboard copy interchange. `out[r][c]`
   *  is the number-formatted display of the cell at the range's row `r`, col
   *  `c` (`""` for empty); a formula cell yields its computed display. All
   *  formatting/normalization decided in Rust (the same value the lowering
   *  and the grid show — spec §8.3). Junk endpoints / an OOB sheet THROW the
   *  boundary error. */
  getRangeValues(sheet: number, range: string): string[][];
  /** Paginate a range across the host frame chain's content boxes (Wave 2D
   *  / S-05). `frames` is the chain's content boxes (only height splits);
   *  returns one `Page` per filled frame — each a self-contained
   *  `LoweredContent` plus its chain index + continuation flags. The
   *  threading math lives in Rust (spec §8.2, the killer feature). */
  paginate(
    sheet: number,
    range: string,
    frames: FrameBox[],
    opts?: PaginateOptions,
  ): Page[];
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
  /** Enumerate the worksheets with a FROZEN PANE (spec §8.1). Read-only
   *  derived state parsed from the workbook's `<sheetViews><pane>` on load
   *  (the view round-trips byte-identical); empty when none are frozen. */
  listFreezePanes(): FreezeInfo[];
  /** Enumerate the engine's registered IMPLEMENTED functions for the
   *  formula-bar autocomplete (S-04). The names come from the engine's
   *  registry table (constitution §7) — never a TS list. Workbook-
   *  independent (the registry is build-time fixed). */
  listFunctions(): FunctionInfo[];
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
  get_cell_input(sheet: number, row: number, col: number): string;
  sort_range(
    sheet: number,
    range: string,
    key_col: number,
    ascending: boolean,
    has_header: boolean,
  ): SortResult;
  find_all(
    sheet: number | undefined,
    needle: string,
    opts?: FindOptions,
  ): FindMatch[];
  replace_all(
    sheet: number | undefined,
    needle: string,
    replacement: string,
    opts?: FindOptions,
  ): ReplaceResult;
  get_range_lowered(
    sheet: number,
    range: string,
    opts?: LowerOptions,
  ): LoweredContent;
  get_range_values(sheet: number, range: string): string[][];
  paginate(
    sheet: number,
    range: string,
    frames: FrameBox[],
    opts?: PaginateOptions,
  ): Page[];
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
  list_freeze_panes(): FreezeInfo[];
  list_functions(): FunctionInfo[];
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
    getCellInput: (sheet, row, col) => wasm.get_cell_input(sheet, row, col),
    sortRange: (sheet, range, keyCol, ascending, hasHeader) =>
      wasm.sort_range(sheet, range, keyCol, ascending, hasHeader),
    findAll: (sheet, needle, opts) => wasm.find_all(sheet, needle, opts),
    replaceAll: (sheet, needle, replacement, opts) =>
      wasm.replace_all(sheet, needle, replacement, opts),
    getRangeLowered: (sheet, range, opts) =>
      wasm.get_range_lowered(sheet, range, opts),
    getRangeValues: (sheet, range) => wasm.get_range_values(sheet, range),
    paginate: (sheet, range, frames, opts) =>
      wasm.paginate(sheet, range, frames, opts),
    getGridScene: (sheet, firstRow, firstCol, wPt, hPt, opts) =>
      wasm.get_grid_scene(sheet, firstRow, firstCol, wPt, hPt, opts),
    setGridSelection: (sheet, anchorRow, anchorCol, rows, cols) =>
      wasm.set_grid_selection(sheet, anchorRow, anchorCol, rows, cols),
    listSheets: () => wasm.list_sheets(),
    listCharts: () => wasm.list_charts(),
    listFreezePanes: () => wasm.list_freeze_panes(),
    listFunctions: () => wasm.list_functions(),
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

/** Load + instantiate the engine wasm module (the glue + the `_bg.wasm`),
 *  branching browser vs Node exactly like `bootEngine`. Split out so both
 *  the XLSX-load boot and the EMPTY-workbook boot (S-15) share one
 *  instantiate path. Rejects with ENGINE_NOT_BUILT-flavoured detail when
 *  the artifact is missing. */
async function loadModule(): Promise<SheetWasmModule> {
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
    // Browser path: resolve the artifact through the bundler's explicit
    // `?url` import (the editor's wasm-loading convention). A bare
    // relative URL resolves against the SERVED module path, and the dev
    // server answers its HTML fallback — the
    // "expected magic word 00 61 73 6d, found 3c 21 64 6f" boot failure
    // the K-1 live-validation e2e surfaced. Object form: the bare
    // argument is deprecated by wasm-bindgen.
    // @ts-ignore — `?url` is a bundler affordance, untyped.
    const wasmUrl = (await import("../bin/sheet_js_bg.wasm?url")) as {
      default: string;
    };
    await mod.default({ module_or_path: wasmUrl.default });
  }
  return mod;
}

/** Load + boot the engine wasm, returning the facade. Browser path
 *  passes the `_bg.wasm` URL to `default()`; Node path reads the bytes
 *  and `initSync`s (mirroring plugin-sdk's wasm-loader.ts). Rejects with
 *  ENGINE_NOT_BUILT-flavoured detail when the artifact is missing so the
 *  panel can surface the honest "not built" state. */
export async function bootEngine(): Promise<SheetEngine> {
  const mod = await loadModule();
  return wrapEngine(new mod.SheetEngine());
}

/** Boot a FRESH, EMPTY workbook (S-15 — source-a-sheet-from-a-dataset).
 *  `new SheetEngine()` boots an empty workbook with one sheet "Sheet1"
 *  (sheet id 0); the caller seeds cells with `setCell`. Identical boot
 *  path to `bootEngine` — the only difference is no `loadXlsx`, so the
 *  workbook starts blank rather than parsed from bytes. Same honest
 *  ENGINE_NOT_BUILT rejection when the artifact is absent. */
export async function bootEmptyEngine(): Promise<SheetEngine> {
  const mod = await loadModule();
  return wrapEngine(new mod.SheetEngine());
}
