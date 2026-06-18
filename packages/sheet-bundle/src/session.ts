// The workbook session — the bundle's IN-MEMORY workbook handle (S-08:
// no persistence; workbook bytes NEVER touch host.storage — the panel
// says so). It holds the booted engine + the active sheet/range/file
// name, exposes import + lower + dispose, and emits a change signal the
// panel subscribes to. All spreadsheet work is the engine's; this is
// session bookkeeping + the host write path.

import type {
  BundleHost,
  DataProviderInfo,
  ElementId,
  ProviderRecordSet,
  SceneLayerSurface,
  TabularClipboard,
} from "@paged-media/plugin-api";
import {
  gridSceneToSceneLayer,
  hitCell,
  type FunctionEntry,
  type GridCell,
  type GridScene,
  type GridSelection,
} from "@paged-media/sheet-host-model";

import {
  bootEmptyEngine,
  bootEngine,
  ENGINE_NOT_BUILT,
  type CellEditRecord,
  type ChartInfo,
  type FindMatch,
  type FindOptions,
  type SheetEngine,
} from "./engine";
import { lowerSelectionToFrame, type LoweredTableInfo } from "./lower";
import { lowerChartToFrame } from "./lower-chart";
import { readWorkbookPart, writeWorkbookPart } from "./workbook-part";
import {
  planCellStyleFromEntries,
  tableCellPositionOf,
  type ReadEntry,
} from "@paged-media/sheet-host-model";

/** S-08 persistence keys: the workbook bytes live in `host.blob` (binary),
 *  its display name in the KV `host.storage`. Per-plugin — the last
 *  imported workbook is the one restored on reload. */
const BLOB_KEY = "workbook";
const BLOB_NAME_KEY = "workbook.name";

/** A tiny synchronous event emitter (one channel: "did the session
 *  state change"). Avoids dragging a dependency for a single signal. */
class Emitter {
  private listeners = new Set<() => void>();
  on(listener: () => void): { dispose(): void } {
    this.listeners.add(listener);
    return {
      dispose: () => {
        this.listeners.delete(listener);
      },
    };
  }
  emit(): void {
    for (const l of [...this.listeners]) l();
  }
  clear(): void {
    this.listeners.clear();
  }
}

/** The session's reactive state — a plain snapshot the panel renders. */
export interface SessionState {
  /** The booted engine, or null before the first successful import. */
  engine: SheetEngine | null;
  /** The imported workbook's file name (display only). */
  fileName: string | null;
  /** The active sheet's wasm id, or null. */
  activeSheet: number | null;
  /** The A1 range the panel will lower. */
  selectedRange: string | null;
  /** Set when boot failed (e.g. the artifact isn't built — S-10). */
  bootError: string | null;
  /** The sheets-mode grid selection rectangle (spec §8.1), or null. The
   *  grid panel sets it on click; [`gridScene`] overlays it on the scene so
   *  the SVG draws the selection chrome (the engine also gets told via
   *  `setGridSelection` so the JOINS-phase wasm can carry it natively). */
  gridSelection: GridSelection | null;
  /** S-15 — when the active workbook was sourced from a governed dataset
   *  (`sourceFromDataset`), the linked provider id + the revision the cells
   *  were seeded from, and whether the provider has since announced a newer
   *  revision (`stale`). Null when the workbook was hand-entered / imported
   *  from XLSX (the snapshot is committed content either way — §1.1 honesty:
   *  no auto-refetch; a refresh is an explicit re-source). */
  dataSource: { providerId: string; revision: string; stale: boolean } | null;
}

export interface WorkbookSession {
  /** Read the current state snapshot. */
  state(): SessionState;
  /** Subscribe to state changes (the panel's render trigger). */
  onDidChange(listener: () => void): { dispose(): void };
  /** Import XLSX bytes under a display name: boots the engine on first
   *  use, loads the workbook, defaults the active sheet + range, and (when
   *  `host.blob` is wired) PERSISTS the bytes so they survive a reload
   *  (S-08). */
  import(bytes: Uint8Array, name: string): Promise<void>;
  /** Restore the last persisted workbook from `host.blob` (S-08), if any.
   *  A cheap no-op (one blob read) when nothing was persisted or no blob
   *  store is wired — the engine boots ONLY when there are bytes to load.
   *  Returns whether a workbook was restored. */
  restore(): Promise<boolean>;
  /** Set which sheet is active (and default its range to the used
   *  extent). */
  setActiveSheet(id: number): void;
  /** Set the A1 range the next lower will project. */
  setRange(range: string): void;
  /** Lower the active sheet's selected range to a new page frame
   *  (the two-phase flow in lower.ts). Returns the created frame id. */
  lowerSelection(): Promise<string | null>;
  /** C-1 / S-02 — render the active sheet's grid INSIDE a frame as a live
   *  vector layer (`host.contribute.sceneLayer()`): gridlines + cell fills
   *  + cell values, clipped to the frame's content box by core. `frameId`
   *  targets a specific frame (e.g. the one a sheet edit-context entered
   *  on); omitted ⇒ the last-lowered frame. Returns false when there is no
   *  target frame, no scene channel (`supports("rendering.sceneLayer@1")`),
   *  or no engine. The layer is EPHEMERAL (re-submitted; not doc content). */
  showGridInFrame(frameId?: string): Promise<boolean>;
  /** K-1 — select the cell under a FRAME-CONTENT-space point (the editor
   *  inverted the frame transform before delivering it) and re-render the
   *  in-frame grid with the selection chrome. Pure `hitCell` against the
   *  last rendered grid — no engine round-trip for the hit. Returns false
   *  when no grid is shown or the point falls outside the windowed cells. */
  selectCellInFrame(contentX: number, contentY: number): boolean;
  /** K-1 — is an in-frame cell edit in progress? (Drives the edit context's
   *  `isDirty` so the shell routes Enter/Esc to the cell, not the context.) */
  isCellEditing(): boolean;
  /** K-1 — a printable key in-frame: begin a fresh replace-mode edit on the
   *  selected cell, or append to the open one. Re-renders with the buffer.
   *  Returns false when there's no selected cell / not a single char. */
  typeCellChar(ch: string): boolean;
  /** K-1 — Backspace in-frame: open from the cell's current value if not
   *  editing, then drop the last char. Returns false when no cell selected. */
  backspaceCellEdit(): boolean;
  /** K-1 — commit the in-frame cell edit (Enter): write the buffer via the
   *  engine + re-render. Returns whether an edit was committed. */
  commitCellEdit(): boolean;
  /** K-1 — abandon the in-frame cell edit (Esc): drop the buffer + re-render
   *  the committed value. */
  cancelCellEdit(): void;
  /** Clear the in-frame grid layer (the frame returns to its native
   *  lowered content). */
  hideGridInFrame(): void;
  /** Enumerate the workbook's parsed charts (M2 charts track, spec §8.4).
   *  Empty when there is no engine / no charts. */
  listCharts(): ChartInfo[];
  /** Lower a parsed chart to a paged.draw vector frame (spec §8.4 — the
   *  two-phase flow in lower-chart.ts). `chartIndex` indexes [`listCharts`].
   *  Returns false when there is no engine or the lower fails. */
  lowerChart(chartIndex: number): Promise<boolean>;
  /** Window the active sheet into a [`GridScene`] for the grid panel (spec
   *  §8.1). Delegates the windowing to `engine.getGridScene` (Rust) and
   *  overlays the session's current [`gridSelection`] onto the scene.
   *  Returns null when there is no engine / active sheet. */
  gridScene(
    firstRow: number,
    firstCol: number,
    wPt: number,
    hPt: number,
  ): GridScene | null;
  /** Record the grid selection rectangle (spec §8.1): forwards to the
   *  engine (`setGridSelection`) AND holds it in session state so the next
   *  `gridScene` paints it. Emits a change so the panel re-renders. */
  setGridSelection(
    anchorRow: number,
    anchorCol: number,
    rows: number,
    cols: number,
  ): void;
  /** Commit one cell edit (spec §8.1 panel edit contract): `engine.setCell`
   *  then refresh (emit). All spreadsheet semantics are the engine's; this
   *  only drives the write + signal. Returns false when there is no engine
   *  / active sheet or the write throws (never throws). */
  editCell(sheet: number, row: number, col: number, input: string): boolean;
  /** S-04 formula bar — the re-enterable INPUT text of `(row, col)` on the
   *  active sheet (`engine.getCellInput`: `"=…"` for a formula, the literal
   *  for a value, `""` for empty/OOB). The formula bar prefills with this so
   *  editing a formula cell shows its formula, not the computed display.
   *  Returns "" when there is no engine / active sheet (never throws). */
  cellInputAt(row: number, col: number): string;
  /** K-6 / S-14 — COPY the current grid selection to the system clipboard
   *  (`host.clipboard.write`). Reads the selected range's FORMATTED display
   *  strings from the engine (`getRangeValues` — all formatting in Rust) and
   *  writes BOTH a `tabular` grid AND a TSV `text` fallback. Returns the
   *  outcome: `ok:true` with the copied row/col counts, or `ok:false` with an
   *  honest reason (no selection / no engine / the clipboard door denied).
   *  Never throws. */
  copySelection(): Promise<
    | { ok: true; rows: number; cols: number }
    | { ok: false; message: string }
  >;
  /** K-6 / S-14 — PASTE the system clipboard into the grid at the selection
   *  ANCHOR (`host.clipboard.read`). Prefers the rich `tabular` grid; falls
   *  back to parsing the `text` half as TSV. Each cell re-types through the
   *  journaled `editCell` lane as ONE grouped ADR-012 undo step (one Cmd-Z
   *  undoes the whole paste). Returns the outcome: `ok:true` with the pasted
   *  row/col counts, or `ok:false` with an honest reason (no selection / no
   *  engine / nothing on the clipboard). Never throws. */
  pasteAtSelection(): Promise<
    | { ok: true; rows: number; cols: number }
    | { ok: false; message: string }
  >;
  /** S-04 formula bar — the engine's registry-generated function name table
   *  for the autocomplete (constitution §7: the completion names are the
   *  ENGINE's, never a TS list). Cached after the first call (the registry
   *  is build-time fixed). Empty when there is no engine (never throws). */
  functionList(): readonly FunctionEntry[];
  /** ADR-012 Tier 1 — undo one step of the in-session journal. An OPEN
   *  cell-edit buffer unwinds first (= cancel, no Operation); then each
   *  call re-enters the previous INPUT of the latest committed cell edit.
   *  Returns false when exhausted (the shell does NOT fall through to the
   *  document stack mid-session). */
  undoCellEdit(): boolean;
  /** ADR-012 Tier 1 — re-apply the next journal entry (false when none). */
  redoCellEdit(): boolean;
  canUndoCellEdit(): boolean;
  canRedoCellEdit(): boolean;
  /** Drop the journal — the modal session boundary (call on exit; Tier 2's
   *  re-lowered batch owns the document grain from there). */
  clearCellEditJournal(): void;
  /** Sort the SELECTED RANGE's rows on the active sheet by `keyCol`
   *  (0-based, relative to the range) — thin glue over `engine.sortRange`
   *  (all sort semantics in Rust, sheet.edit.sort.*). The engine's per-cell
   *  input rewrites journal as ONE grouped ADR-012 step (one Cmd-Z undoes
   *  the whole sort). Returns the honest outcome: `ok: false` carries the
   *  engine's boundary message (e.g. "sort over formulas not yet
   *  supported") for the panel to show. Never throws. */
  sortRange(
    keyCol: number,
    ascending: boolean,
    hasHeader: boolean,
  ): { ok: true } | { ok: false; message: string };
  /** Find every cell matching `needle` — thin glue over `engine.findAll`
   *  (matching/collation decided in Rust, sheet.edit.find.*). `scope`
   *  "sheet" searches the active sheet; "workbook" all sheets. Returns []
   *  when there is no engine or the call fails (never throws). */
  findAll(
    needle: string,
    opts: FindOptions,
    scope: "sheet" | "workbook",
  ): FindMatch[];
  /** Replace every occurrence over the scope — thin glue over
   *  `engine.replaceAll` (input-text splice + re-entry decided in Rust,
   *  sheet.edit.replace.*). The per-cell rewrites journal as ONE grouped
   *  step. Returns the counts (skipped = parse-failed/spill cells the
   *  engine reported, untouched) or the honest error. Never throws. */
  replaceAll(
    needle: string,
    replacement: string,
    opts: FindOptions,
    scope: "sheet" | "workbook",
  ):
    | { occurrences: number; replacedCells: number; skipped: number }
    | { error: string };
  /** Jump to a cell (a find hit): activate its sheet if needed and select
   *  it in the grid (the panel + any in-frame grid re-render). */
  goToCell(sheet: number, row: number, col: number): void;
  /** S-04 — mint a NEW cell style named `name` from the selected cell's
   *  current appearance, over the last-lowered native table. Composes from
   *  existing platform doors (the RFI verdict): read the cell's properties
   *  (B-19 `elementProperties`), `createCellStyle` (selfId-minted — the
   *  null-createdId precedent), `setStyleProperty` to populate it, then
   *  ATTEMPT `setElementProperty{appliedCellStyle}` to apply it back.
   *
   *  HONEST RESIDUAL (verified against the plugin-api contract): applying a
   *  cell style via `appliedCellStyle` is wire-shape-only today
   *  ("UnsupportedProperty until the Table NodeId surface lands",
   *  wire.d.ts CellStyleSummary). So the style is minted + populated (these
   *  land), and the apply is reported as `applied` true/false honestly.
   *
   *  Returns the outcome: the minted style id, the count of captured
   *  properties, and whether the apply-back took. `ok:false` carries the
   *  reason (no lowered table / no selection / mint rejected). Never throws. */
  newCellStyleFromSelection(
    name: string,
  ): Promise<
    | {
        ok: true;
        styleId: string;
        capturedCount: number;
        applied: boolean;
        applyMessage: string | null;
      }
    | { ok: false; message: string }
  >;
  /** Re-emit the loaded workbook as XLSX bytes for the exporter
   *  contribution (S-06). Preservation-first (`engine.saveXlsx` — the
   *  lazy-verbatim re-emit, §10.2). Returns the bytes + a suggested file
   *  name, or null when there is no workbook (nothing to export). */
  saveWorkbook(): { bytes: Uint8Array; fileName: string } | null;
  /** S-15 — enumerate the governed datasets the platform offers in the
   *  `"dataset"` category (`host.dataProviders.discover`), schema + revision
   *  only, NO rows. The datasets panel lists these so the author can source
   *  a sheet from one. Returns [] when the `dataProviders` surface is absent
   *  or no shared registry is wired (`supports("dataProviders@1")` false) —
   *  the §2.1 graceful-absence posture (paged.data not installed ⇒ no
   *  sources). Never throws. */
  discoverDatasets(): readonly DataProviderInfo[];
  /** S-15 — source the active workbook from a governed dataset: pull the
   *  provider's resolved snapshot (`host.dataProviders.get`), boot a FRESH
   *  EMPTY workbook, and seed sheet 0 (row 0 = the schema field names; rows
   *  1.. = the column-major records). Sets `state.dataSource` to the linked
   *  `(providerId, revision)` and subscribes to `onDidChange` so a later
   *  revision marks the sheet stale (logged "re-source to refresh"; NO
   *  auto-refetch — §1.1 / the RFC). A no-op (logged) when the provider is
   *  gone, the surface is absent, or the engine cannot boot. */
  sourceFromDataset(providerId: string): Promise<void>;
  /** Tear down: free the engine, drop listeners. */
  dispose(): void;
}

/** S-15 — coerce one provider cell value to the string the engine's
 *  `setCell` ingests (it parses the input back to a typed value in Rust —
 *  §3: no spreadsheet semantics in TS, this is pure transport).
 *
 *  CONTRACT NOTE (forward-compatible): the data-provider RFC's
 *  `ProviderRecordSet.columns[c][r]` cell MAY arrive as a PLAIN JS value
 *  (`string | number | boolean | null`) OR as the data engine's TAGGED form
 *  `{ t: "text"|"number"|"bool"|"date"|"datetime"|"null"|…, v }`. The
 *  contract (`plugin-api`) does not yet standardize which — a follow-up
 *  should pin one encoding. We handle BOTH defensively: an object carrying
 *  a `t`/`v` tag uses its `v`; anything else is used directly. `null` /
 *  `undefined` (and a null tag's value) lower to "" (a blank cell). */
export function cellToString(value: unknown): string {
  // Tagged form `{ t, v }` from the data engine — unwrap to the value.
  if (
    typeof value === "object" &&
    value !== null &&
    "t" in value &&
    "v" in value
  ) {
    return cellToString((value as { v: unknown }).v);
  }
  if (value === null || value === undefined) return "";
  if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
  return String(value);
}

/** Default the range to the whole used extent of a sheet (A1 to the
 *  bottom-right used cell). Pure A1 formatting — NOT spreadsheet
 *  semantics (the engine decided the extent; this only names it). */
export function usedRangeA1(rows: number, cols: number): string {
  if (rows <= 0 || cols <= 0) return "A1";
  return `A1:${columnLabel(cols - 1)}${rows}`;
}

/** 0-based column index → A1 column letters (0→A, 25→Z, 26→AA). A
 *  display helper, not a parser; the engine validates the real range. */
export function columnLabel(index: number): string {
  let n = index;
  let label = "";
  do {
    label = String.fromCharCode(65 + (n % 26)) + label;
    n = Math.floor(n / 26) - 1;
  } while (n >= 0);
  return label;
}

/** K-6 — the A1 range string for a grid selection rectangle (anchor +
 *  span). Pure A1 formatting (NOT spreadsheet semantics — the engine reads
 *  + validates the range). A 1×1 selection yields a single-cell range. */
export function selectionRangeA1(
  anchorRow: number,
  anchorCol: number,
  rows: number,
  cols: number,
): string {
  const start = `${columnLabel(anchorCol)}${anchorRow + 1}`;
  if (rows <= 1 && cols <= 1) return start;
  const end = `${columnLabel(anchorCol + cols - 1)}${anchorRow + rows}`;
  return `${start}:${end}`;
}

/** K-6 — parse a TSV `text` clipboard half into a rectangular grid (the
 *  fallback when the platform offered no rich `tabular`). Tabs split cells,
 *  newlines split rows; CRLF tolerated; a single trailing newline dropped
 *  (so a copied range doesn't gain a blank row). Pure transport — never
 *  spreadsheet semantics. */
export function tsvToRows(text: string): string[][] {
  const normalized = text.replace(/\r\n/g, "\n").replace(/\r/g, "\n");
  const body = normalized.endsWith("\n") ? normalized.slice(0, -1) : normalized;
  if (body === "") return [];
  return body.split("\n").map((line) => line.split("\t"));
}

export function createWorkbookSession(host: BundleHost): WorkbookSession {
  const emitter = new Emitter();
  const state: SessionState = {
    engine: null,
    fileName: null,
    activeSheet: null,
    selectedRange: null,
    bootError: null,
    gridSelection: null,
    dataSource: null,
  };

  // S-15 — the live `onDidChange` subscription for the currently-linked
  // dataset (disposed + replaced on each `sourceFromDataset`, and on
  // `dispose`). Null when the workbook is not dataset-sourced.
  let dataSourceSub: { dispose(): void } | null = null;

  // C-1 / S-02 — the last frame this session lowered into (the target for
  // the in-frame grid) + the lazily-obtained scene-layer surface.
  let lastFrameId: string | null = null;

  // S-04 — the last NATIVE TABLE this session lowered (frame/story/table ids
  // + the sheet/range it projects). "New style from cell" addresses this
  // table's cells; null until a range is lowered to a native table.
  let lastLoweredTable: LoweredTableInfo | null = null;
  // S-04 — a per-session counter so minted cell-style ids are unique within
  // one session (paired with a timestamp so they are unique across sessions).
  let nextCellStyleSeq = 1;
  let sceneSurface: SceneLayerSurface | null = null;
  const sceneChannel = (): SceneLayerSurface | null => {
    if (!host.supports("rendering.sceneLayer@1")) return null;
    if (!sceneSurface) sceneSurface = host.contribute.sceneLayer();
    return sceneSurface;
  };

  // K-1 — the LAST in-frame grid this session rendered: the window it was
  // computed with (so a re-render keeps the same viewport) + the resolved
  // scene (so a content-space pointer can `hitCell` against it without
  // re-querying the engine). Both null until `showGridInFrame` runs.
  let lastGridWindow:
    | { firstRow: number; firstCol: number; wPt: number; hPt: number }
    | null = null;
  let lastGridScene: GridScene | null = null;

  // K-1 — the in-frame cell EDITOR buffer (a keystroke edit, no DOM
  // overlay): the cell being typed into + its in-progress text. The grid
  // re-renders with this text overlaid until commit (→ engine.setCell) or
  // cancel. `null` ⇒ not editing (the context is not "dirty").
  let cellEdit: { row: number; col: number; text: string } | null = null;

  // S-04 formula bar — the engine's function name table, cached after the
  // first read (the registry is build-time fixed; one wasm call suffices for
  // the whole session). Null until first requested.
  let functionCache: readonly FunctionEntry[] | null = null;

  // ADR-012 Tier 1 — the in-session undo JOURNAL: one entry per COMMITTED
  // cell edit (Tier 0 already coalesced keystrokes into the commit).
  // `prev`/`next` are re-enterable INPUT texts (engine.getCellInput —
  // formula-safe; the display is NOT an inverse). Entries [0, cursor) are
  // undoable, [cursor, length) redoable; a fresh commit truncates the redo
  // tail (the linear-history rule). Cleared on workbook load/source (the
  // session boundary) and on modal exit (Tier 2 owns the document grain).
  // `batch` (additive): entries sharing a batch id were one BULK op (sort /
  // replace-all) — undo/redo unwind the whole batch as ONE step. Plain cell
  // edits carry no batch and unwind singly (unchanged behavior).
  let editJournal: {
    sheet: number;
    row: number;
    col: number;
    prev: string;
    next: string;
    batch?: number;
  }[] = [];
  let journalCursor = 0;
  let nextJournalBatch = 1;

  /** Write one cell through the engine AND journal it (the shared Tier-1
   *  capture for `editCell` + the in-frame commit). Returns false when
   *  there is no engine or the write throws — nothing is journaled then. */
  function journaledSetCell(
    sheet: number,
    row: number,
    col: number,
    input: string,
  ): boolean {
    if (!state.engine) return false;
    let prev: string;
    try {
      prev = state.engine.getCellInput(sheet, row, col);
      state.engine.setCell(sheet, row, col, input);
    } catch (err) {
      host.log.error("setCell failed", err);
      return false;
    }
    editJournal.length = journalCursor; // drop any redo tail
    editJournal.push({ sheet, row, col, prev, next: input });
    journalCursor = editJournal.length;
    return true;
  }

  /** Journal a BULK op's per-cell rewrites (the engine's `edits` lane —
   *  prev/next already faithful inputs) as ONE grouped batch: a single
   *  undo/redo step for the whole sort / replace-all. No-op when the op
   *  changed nothing. */
  function journalBatch(edits: readonly CellEditRecord[]): void {
    if (edits.length === 0) return;
    editJournal.length = journalCursor; // drop any redo tail
    const batch = nextJournalBatch++;
    for (const e of edits) {
      editJournal.push({
        sheet: e.sheet,
        row: e.row,
        col: e.col,
        prev: e.prevInput,
        next: e.nextInput,
        batch,
      });
    }
    journalCursor = editJournal.length;
  }

  /** Record the grid selection (engine + session) so the next windowing
   *  paints it. Shared by the panel's `setGridSelection` door and K-1's
   *  in-frame click-to-select. Pure state + signal — never throws. */
  function applyGridSelection(
    anchorRow: number,
    anchorCol: number,
    rows: number,
    cols: number,
  ): void {
    state.gridSelection = { anchorRow, anchorCol, rows, cols };
    if (state.engine && state.activeSheet !== null) {
      try {
        state.engine.setGridSelection(
          state.activeSheet,
          anchorRow,
          anchorCol,
          rows,
          cols,
        );
      } catch (err) {
        // The wasm side lands in JOINS — tolerate its absence; the
        // session-held selection still drives the overlay.
        host.log.debug("setGridSelection: engine not ready", err);
      }
    }
    emitter.emit();
  }

  /** Re-window + submit the in-frame grid for `lastGridWindow` (carrying
   *  the current selection). Caches `lastGridScene` for the next hit-test.
   *  Returns false when there is no target frame / scene channel / window
   *  / engine. Never throws. */
  async function submitInFrameGrid(): Promise<boolean> {
    if (!lastFrameId || !lastGridWindow) return false;
    const surface = sceneChannel();
    if (!surface) return false;
    const scene = computeGridScene(
      lastGridWindow.firstRow,
      lastGridWindow.firstCol,
      lastGridWindow.wPt,
      lastGridWindow.hPt,
    );
    if (!scene) return false;
    // K-1 — overlay the in-progress cell-edit text on its cell (the engine
    // scene still shows the COMMITTED value; the buffer is uncommitted).
    if (cellEdit) overlayCellText(scene, cellEdit.row, cellEdit.col, cellEdit.text);
    lastGridScene = scene;
    try {
      await surface.submit(lastFrameId, gridSceneToSceneLayer(scene));
    } catch (err) {
      host.log.error("showGridInFrame: submit failed", err);
      return false;
    }
    return true;
  }

  /** Override the rendered text of `(row, col)` in `scene` (mutates it) —
   *  the uncommitted cell-edit buffer. Replaces the cell if present in the
   *  window, else appends a left-aligned one so an edit on an empty cell
   *  still shows. */
  function overlayCellText(
    scene: GridScene,
    row: number,
    col: number,
    text: string,
  ): void {
    const existing = scene.cells.find((c) => c.row === row && c.col === col);
    if (existing) {
      existing.text = text;
      return;
    }
    const cell: GridCell = { row, col, text, align: "left", styleKey: 0 };
    scene.cells.push(cell);
  }

  /** The display value of `(row, col)` on the active sheet, or "" — the
   *  seed when an edit OPENS on a populated cell (F2-style). Never throws. */
  function cellDisplay(row: number, col: number): string {
    if (!state.engine || state.activeSheet === null) return "";
    try {
      return state.engine.getCellDisplay(state.activeSheet, row, col) ?? "";
    } catch {
      return "";
    }
  }

  function defaultRangeForActive(): void {
    if (!state.engine || state.activeSheet === null) return;
    const sheet = state.engine
      .listSheets()
      .find((s) => s.id === state.activeSheet);
    if (sheet) state.selectedRange = usedRangeA1(sheet.rows, sheet.cols);
  }

  /** S-15 — seed sheet 0 of a fresh engine from a provider RecordSet: row 0
   *  = the schema field names (the header); rows 1.. = the column-major
   *  records (`columns[c][r-1]` → the cell at row r, col c). Every value
   *  goes in as a STRING via `cellToString` + `engine.setCell` — the engine
   *  re-types it in Rust (§3: no spreadsheet semantics in TS). Pure transport
   *  over the engine; tolerant of a per-cell write throwing (logs + skips).
   */
  function seedSheetFromRecords(
    engine: SheetEngine,
    records: ProviderRecordSet,
  ): void {
    const fields = records.schema.fields;
    // Header row (row 0) — the schema field names.
    for (let c = 0; c < fields.length; c++) {
      writeCell(engine, 0, c, fields[c].name);
    }
    // Body rows (rows 1..rowCount) — column-major: columns[c][r] is the
    // cell value for data-row r, which lands on sheet row r + 1.
    for (let c = 0; c < records.columns.length; c++) {
      const col = records.columns[c];
      for (let r = 0; r < records.rowCount; r++) {
        writeCell(engine, r + 1, c, cellToString(col[r]));
      }
    }
  }

  /** Write one cell through the engine, tolerating a throw (an out-of-range
   *  or malformed input never aborts the whole seed — it logs + skips). */
  function writeCell(
    engine: SheetEngine,
    row: number,
    col: number,
    value: string,
  ): void {
    try {
      engine.setCell(0, row, col, value);
    } catch (err) {
      host.log.warn(`sourceFromDataset: setCell(0,${row},${col}) failed`, err);
    }
  }

  /** Window the active sheet into a GridScene + overlay the session
   *  selection. Shared by the `gridScene` door and `showGridInFrame`. */
  function computeGridScene(
    firstRow: number,
    firstCol: number,
    wPt: number,
    hPt: number,
  ): GridScene | null {
    if (!state.engine || state.activeSheet === null) return null;
    let scene: GridScene;
    try {
      scene = state.engine.getGridScene(
        state.activeSheet,
        firstRow,
        firstCol,
        wPt,
        hPt,
        { includeGridlines: true },
      );
    } catch (err) {
      host.log.warn("gridScene: engine windowing failed", err);
      return null;
    }
    if (state.gridSelection) scene.selection = state.gridSelection;
    return scene;
  }

  /** S-08: persist the imported bytes + name (best-effort — never let a
   *  persist failure break an import). Per-plugin keyed: the LAST imported
   *  workbook is the one restored on reload.
   *
   *  Two homes, by design: the `.paged` container PART (the portable one —
   *  it travels WITH the document, the read PREFERENCE on restore) and the
   *  per-browser `host.blob` (a fast local cache + backward-compat for hosts
   *  with no container writer). */
  async function persistWorkbook(bytes: Uint8Array, name: string): Promise<void> {
    try {
      await writeWorkbookPart(host, bytes, name);
    } catch (err) {
      host.log.warn("workbook container-part persist failed", err);
    }
    if (!host.supports("storage.blob@1")) return;
    try {
      await host.blob.write(BLOB_KEY, bytes);
      host.storage.set(BLOB_NAME_KEY, name);
    } catch (err) {
      host.log.warn("workbook persist failed (kept in memory)", err);
    }
  }

  /** Boot (if needed) + load bytes into the engine + default sheet/range.
   *  Shared by import (persist) and restore (no re-persist). Returns true
   *  on a successful load. */
  async function loadWorkbook(
    bytes: Uint8Array,
    name: string,
    persist: boolean,
  ): Promise<boolean> {
    try {
      if (!state.engine) state.engine = await bootEngine();
      state.bootError = null;
    } catch (err) {
      // Boot failure (the artifact isn't built — S-10). Surface it; the
      // panel renders the honest "not built" state.
      state.engine = null;
      state.bootError = err instanceof Error ? err.message : ENGINE_NOT_BUILT;
      host.log.warn("sheet engine boot failed", err);
      emitter.emit();
      return false;
    }
    try {
      state.engine.loadXlsx(bytes);
      state.fileName = name;
      state.gridSelection = null;
      lastLoweredTable = null; // the prior table belonged to the old workbook
      editJournal = [];
      journalCursor = 0;
      const sheets = state.engine.listSheets();
      state.activeSheet = sheets.length > 0 ? sheets[0].id : null;
      defaultRangeForActive();
    } catch (err) {
      host.log.error("workbook load failed", err);
      state.fileName = null;
      state.activeSheet = null;
      state.selectedRange = null;
      emitter.emit();
      return false;
    }
    if (persist) await persistWorkbook(bytes, name);
    emitter.emit();
    return true;
  }

  return {
    state: () => state,
    onDidChange: (l) => emitter.on(l),

    async import(bytes, name) {
      await loadWorkbook(bytes, name, true);
    },

    async restore() {
      // Prefer the portable `.paged` container part — it travels WITH the
      // document, so a fresh browser profile / another machine restores it
      // even though the per-browser blob is empty there.
      let fromPart: { bytes: Uint8Array; name: string } | null = null;
      try {
        fromPart = await readWorkbookPart(host);
      } catch (err) {
        host.log.warn("workbook container-part restore failed", err);
      }
      if (fromPart) return loadWorkbook(fromPart.bytes, fromPart.name, false);

      // Fall back to the per-browser blob (S-08 — pre-migration documents, or
      // a host with no container writer).
      if (!host.supports("storage.blob@1")) return false;
      let bytes: Uint8Array | null;
      try {
        bytes = await host.blob.read(BLOB_KEY);
      } catch (err) {
        host.log.warn("workbook restore read failed", err);
        return false;
      }
      if (!bytes) return false; // nothing persisted — no engine boot
      const name = host.storage.get<string>(BLOB_NAME_KEY) ?? "workbook.xlsx";
      const ok = await loadWorkbook(bytes, name, false);
      // One-time migration: lift the per-browser blob into the container so the
      // workbook now travels with the document on the next save.
      if (ok) {
        try {
          await writeWorkbookPart(host, bytes, name);
        } catch (err) {
          host.log.warn("workbook container-part migration failed", err);
        }
      }
      return ok;
    },

    setActiveSheet(id) {
      state.activeSheet = id;
      // Selection is sheet-relative — clear it on a sheet switch.
      state.gridSelection = null;
      defaultRangeForActive();
      emitter.emit();
    },

    setRange(range) {
      state.selectedRange = range;
      emitter.emit();
    },

    async lowerSelection() {
      if (!state.engine || state.activeSheet === null || !state.selectedRange) {
        host.log.warn("lowerSelection: no workbook / sheet / range");
        return null;
      }
      const id = await lowerSelectionToFrame(
        host,
        state.engine,
        state.activeSheet,
        state.selectedRange,
        {
          // S-04 — record the resolved native table so "new style from cell"
          // can address its cells.
          onLowered: (info) => {
            lastLoweredTable = info;
          },
        },
      );
      if (id) lastFrameId = id; // remember the in-frame grid target (S-02)
      return id;
    },

    async showGridInFrame(frameId?: string) {
      const target = frameId ?? lastFrameId;
      if (!target) {
        host.log.warn("showGridInFrame: no target frame — lower a range first");
        return false;
      }
      lastFrameId = target; // the in-frame grid + hide now track this frame
      const surface = sceneChannel();
      if (!surface) {
        host.log.warn(
          "showGridInFrame: no scene channel (supports('rendering.sceneLayer@1') is false)",
        );
        return false;
      }
      if (!state.engine || state.activeSheet === null) {
        host.log.warn("showGridInFrame: no workbook / sheet");
        return false;
      }
      // Size the grid window to the frame's content box (core clips to it).
      let wPt = 480;
      let hPt = 640;
      try {
        const geom = await host.document.elementGeometry([
          { kind: "textFrame", id: lastFrameId } as never,
        ]);
        const bounds = geom[0]?.bounds;
        if (bounds) {
          const [top, left, bottom, right] = bounds;
          wPt = Math.max(right - left, 0);
          hPt = Math.max(bottom - top, 0);
        }
      } catch (err) {
        host.log.debug("showGridInFrame: frame geometry read failed", err);
      }
      lastGridWindow = { firstRow: 0, firstCol: 0, wPt, hPt };
      const ok = await submitInFrameGrid();
      if (!ok) host.log.warn("showGridInFrame: grid windowing failed");
      return ok;
    },

    selectCellInFrame(contentX: number, contentY: number) {
      // K-1 — the editor delivers a pointer in FRAME-CONTENT coordinates
      // (it inverted the frame's ItemTransform + content offset, §8.5).
      // Hit-test it against the last rendered grid, select that cell, and
      // re-render in-frame so the selection chrome shows. No engine round-
      // trip for the hit (pure geometry off `lastGridScene`). A click ALSO
      // cancels any in-progress edit on another cell (Excel behavior).
      if (!lastGridScene) return false;
      const hit = hitCell(lastGridScene, contentX, contentY);
      if (!hit) return false;
      cellEdit = null;
      applyGridSelection(hit.row, hit.col, 1, 1);
      void submitInFrameGrid();
      return true;
    },

    isCellEditing() {
      return cellEdit !== null;
    },

    typeCellChar(ch: string) {
      // K-1 — a printable key in-frame: begin a fresh (replace-mode) edit on
      // the selected cell, or append to the open one. Returns false when
      // there's nothing to edit (no selected cell / not a single char).
      if (ch.length !== 1) return false;
      if (!cellEdit) {
        const sel = state.gridSelection;
        if (!sel) return false;
        cellEdit = { row: sel.anchorRow, col: sel.anchorCol, text: ch };
      } else {
        cellEdit = { ...cellEdit, text: cellEdit.text + ch };
      }
      void submitInFrameGrid();
      return true;
    },

    backspaceCellEdit() {
      // Begin from the cell's current value (F2-like) if not already open,
      // then drop the last char.
      if (!cellEdit) {
        const sel = state.gridSelection;
        if (!sel) return false;
        cellEdit = {
          row: sel.anchorRow,
          col: sel.anchorCol,
          text: cellDisplay(sel.anchorRow, sel.anchorCol),
        };
      }
      cellEdit = { ...cellEdit, text: cellEdit.text.slice(0, -1) };
      void submitInFrameGrid();
      return true;
    },

    commitCellEdit() {
      // Write the buffer through the engine (it recomputes the dirty cut),
      // clear the edit, re-render. Returns whether an edit was committed.
      // Mirrors `editCell` rather than calling it (no reliance on `this`).
      if (!cellEdit) return false;
      const { row, col, text } = cellEdit;
      cellEdit = null;
      if (state.engine && state.activeSheet !== null) {
        journaledSetCell(state.activeSheet, row, col, text);
      }
      emitter.emit();
      void submitInFrameGrid();
      return true;
    },

    cancelCellEdit() {
      if (!cellEdit) return;
      cellEdit = null;
      void submitInFrameGrid();
    },

    hideGridInFrame() {
      cellEdit = null;
      if (lastFrameId) void sceneSurface?.clear(lastFrameId);
    },

    listCharts() {
      if (!state.engine) return [];
      try {
        return state.engine.listCharts();
      } catch (err) {
        host.log.warn("listCharts: engine call failed", err);
        return [];
      }
    },

    async lowerChart(chartIndex) {
      if (!state.engine) {
        host.log.warn("lowerChart: no workbook");
        return false;
      }
      return lowerChartToFrame(host, state.engine, chartIndex);
    },

    gridScene(firstRow, firstCol, wPt, hPt) {
      return computeGridScene(firstRow, firstCol, wPt, hPt);
    },

    setGridSelection(anchorRow, anchorCol, rows, cols) {
      applyGridSelection(anchorRow, anchorCol, rows, cols);
    },

    editCell(sheet, row, col, input) {
      if (!state.engine || state.activeSheet === null) {
        host.log.warn("editCell: no workbook / sheet");
        return false;
      }
      if (!journaledSetCell(sheet, row, col, input)) return false;
      // The dirty cut recomputed in Rust; refresh the panel (it re-requests
      // the windowed scene on the next render).
      emitter.emit();
      return true;
    },

    cellInputAt(row, col) {
      // S-04 formula bar — re-enterable input (engine.getCellInput), so the
      // bar shows a cell's FORMULA, not its computed display. Never throws.
      if (!state.engine || state.activeSheet === null) return "";
      try {
        return state.engine.getCellInput(state.activeSheet, row, col) ?? "";
      } catch (err) {
        host.log.warn("cellInputAt: engine read failed", err);
        return "";
      }
    },

    async copySelection() {
      // K-6 / S-14 — read the selected range's FORMATTED display strings from
      // the engine (all formatting in Rust) and write a tabular payload (+ a
      // TSV text fallback) to the system clipboard. Thin glue (§3): the engine
      // owns the values, the host owns the clipboard.
      if (!state.engine || state.activeSheet === null) {
        return { ok: false as const, message: "no workbook / sheet" };
      }
      const sel = state.gridSelection;
      if (!sel) {
        return { ok: false as const, message: "select a range to copy" };
      }
      const range = selectionRangeA1(
        sel.anchorRow,
        sel.anchorCol,
        sel.rows,
        sel.cols,
      );
      let grid: string[][];
      try {
        grid = state.engine.getRangeValues(state.activeSheet, range);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        host.log.warn("copySelection: range read failed", err);
        return { ok: false as const, message };
      }
      if (grid.length === 0) {
        return { ok: false as const, message: "the selection is empty" };
      }
      const tabular: TabularClipboard = { rows: grid };
      const text = grid.map((r) => r.join("\t")).join("\n");
      try {
        await host.clipboard.write({ text, tabular });
      } catch (err) {
        // The SDK door already swallows a platform refusal; a throw here would
        // be a gate/contract error — report it honestly, never crash the grid.
        const message = err instanceof Error ? err.message : String(err);
        host.log.warn("copySelection: clipboard write failed", err);
        return { ok: false as const, message };
      }
      return {
        ok: true as const,
        rows: grid.length,
        cols: grid[0]?.length ?? 0,
      };
    },

    async pasteAtSelection() {
      // K-6 / S-14 — read the clipboard, land its grid at the selection anchor
      // through the JOURNALED editCell lane as ONE grouped undo step. Prefers
      // the rich tabular half; falls back to TSV. Thin glue (§3): the engine
      // re-types each cell's string in Rust; this only routes + journals.
      if (!state.engine || state.activeSheet === null) {
        return { ok: false as const, message: "no workbook / sheet" };
      }
      const sel = state.gridSelection;
      if (!sel) {
        return { ok: false as const, message: "select a cell to paste at" };
      }
      let payload;
      try {
        payload = await host.clipboard.read();
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        host.log.warn("pasteAtSelection: clipboard read failed", err);
        return { ok: false as const, message };
      }
      if (!payload) {
        return { ok: false as const, message: "the clipboard is empty" };
      }
      // Prefer the rich grid; fall back to parsing the TSV text half.
      const grid =
        payload.tabular?.rows ??
        (payload.text !== undefined ? tsvToRows(payload.text) : []);
      if (grid.length === 0) {
        return {
          ok: false as const,
          message: "nothing tabular on the clipboard",
        };
      }
      // Write each cell through the engine, capturing prev/next inputs so the
      // whole paste journals as ONE grouped ADR-012 undo step. A per-cell write
      // failure is tolerated (logged + skipped) — never half a crash.
      const sheet = state.activeSheet;
      const edits: CellEditRecord[] = [];
      let written = 0;
      for (let r = 0; r < grid.length; r++) {
        const row = grid[r];
        for (let c = 0; c < row.length; c++) {
          const targetRow = sel.anchorRow + r;
          const targetCol = sel.anchorCol + c;
          const next = row[c];
          let prev: string;
          try {
            prev = state.engine.getCellInput(sheet, targetRow, targetCol);
            state.engine.setCell(sheet, targetRow, targetCol, next);
          } catch (err) {
            host.log.warn(
              `pasteAtSelection: setCell(${sheet},${targetRow},${targetCol}) failed`,
              err,
            );
            continue;
          }
          edits.push({ sheet, row: targetRow, col: targetCol, prevInput: prev, nextInput: next });
          written += 1;
        }
      }
      if (written === 0) {
        return { ok: false as const, message: "the paste wrote no cells" };
      }
      journalBatch(edits); // one grouped Cmd-Z undoes the whole paste
      emitter.emit();
      void submitInFrameGrid();
      return {
        ok: true as const,
        rows: grid.length,
        cols: Math.max(...grid.map((r) => r.length)),
      };
    },

    functionList() {
      // S-04 formula bar — the engine's registry function table (constitution
      // §7), cached for the session. Never throws (empty on failure / no
      // engine).
      if (functionCache) return functionCache;
      if (!state.engine) return [];
      try {
        functionCache = state.engine.listFunctions();
        return functionCache;
      } catch (err) {
        host.log.warn("functionList: engine read failed", err);
        return [];
      }
    },

    undoCellEdit() {
      // An open buffer unwinds first — Cmd-Z mid-typing = cancel the
      // in-flight edit (no Operation was committed for it).
      if (cellEdit !== null) {
        cellEdit = null;
        emitter.emit();
        void submitInFrameGrid();
        return true;
      }
      if (journalCursor === 0 || !state.engine) return false;
      // A batched group (sort / replace-all) unwinds WHOLE — one undo step;
      // plain entries (no batch) unwind singly. Cells in a batch are
      // disjoint, so reverse-order re-entry is order-independent.
      const group = editJournal[journalCursor - 1].batch;
      do {
        const entry = editJournal[journalCursor - 1];
        try {
          state.engine.setCell(entry.sheet, entry.row, entry.col, entry.prev);
        } catch (err) {
          host.log.error("undoCellEdit: engine setCell failed", err);
          return false;
        }
        journalCursor -= 1;
      } while (
        group !== undefined &&
        journalCursor > 0 &&
        editJournal[journalCursor - 1].batch === group
      );
      emitter.emit();
      void submitInFrameGrid();
      return true;
    },

    redoCellEdit() {
      if (cellEdit !== null || journalCursor >= editJournal.length) {
        return false;
      }
      if (!state.engine) return false;
      // Mirror of undo: a batched group re-applies whole.
      const group = editJournal[journalCursor].batch;
      do {
        const entry = editJournal[journalCursor];
        try {
          state.engine.setCell(entry.sheet, entry.row, entry.col, entry.next);
        } catch (err) {
          host.log.error("redoCellEdit: engine setCell failed", err);
          return false;
        }
        journalCursor += 1;
      } while (
        group !== undefined &&
        journalCursor < editJournal.length &&
        editJournal[journalCursor].batch === group
      );
      emitter.emit();
      void submitInFrameGrid();
      return true;
    },

    canUndoCellEdit() {
      return cellEdit !== null || journalCursor > 0;
    },

    canRedoCellEdit() {
      return cellEdit === null && journalCursor < editJournal.length;
    },

    clearCellEditJournal() {
      editJournal = [];
      journalCursor = 0;
    },

    sortRange(keyCol, ascending, hasHeader) {
      // Thin glue (§3): the engine owns ALL sort semantics — stable order,
      // typed ranks, blanks-last, the formula-refusal boundary. This only
      // routes the selected range in and journals the result.
      if (!state.engine || state.activeSheet === null || !state.selectedRange) {
        return { ok: false, message: "no workbook / sheet / range" };
      }
      try {
        const res = state.engine.sortRange(
          state.activeSheet,
          state.selectedRange,
          keyCol,
          ascending,
          hasHeader,
        );
        journalBatch(res.edits); // one grouped ADR-012 undo step
        emitter.emit();
        void submitInFrameGrid();
        return { ok: true };
      } catch (err) {
        // The honest boundary (e.g. "sort over formulas not yet
        // supported") — surfaced verbatim for the panel.
        const message = err instanceof Error ? err.message : String(err);
        host.log.warn("sortRange refused", err);
        return { ok: false, message };
      }
    },

    findAll(needle, opts, scope) {
      if (!state.engine) return [];
      const sheet =
        scope === "workbook" ? undefined : (state.activeSheet ?? undefined);
      if (scope === "sheet" && sheet === undefined) return [];
      try {
        return state.engine.findAll(sheet, needle, opts);
      } catch (err) {
        host.log.warn("findAll failed", err);
        return [];
      }
    },

    replaceAll(needle, replacement, opts, scope) {
      if (!state.engine) return { error: "no workbook" };
      const sheet =
        scope === "workbook" ? undefined : (state.activeSheet ?? undefined);
      if (scope === "sheet" && sheet === undefined) {
        return { error: "no active sheet" };
      }
      try {
        const res = state.engine.replaceAll(sheet, needle, replacement, opts);
        journalBatch(res.edits); // one grouped ADR-012 undo step
        emitter.emit();
        void submitInFrameGrid();
        return {
          occurrences: res.occurrences,
          replacedCells: res.edits.length,
          skipped: res.skipped.length,
        };
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        host.log.warn("replaceAll failed", err);
        return { error: message };
      }
    },

    goToCell(sheet, row, col) {
      // A find hit: land on its sheet (range defaults like setActiveSheet)
      // and select the cell; applyGridSelection emits + re-renders.
      if (state.activeSheet !== sheet) {
        state.activeSheet = sheet;
        defaultRangeForActive();
      }
      applyGridSelection(row, col, 1, 1);
    },

    async newCellStyleFromSelection(name: string) {
      // S-04 — thin glue (§3): the engine owns the lowering/appearance, the
      // platform owns style minting + the cell-property read. This routes
      // the selected cell's model coords → the lowered table cell, reads its
      // properties, and mints/populates/attempts-to-apply a cell style.
      if (!lastLoweredTable) {
        return {
          ok: false as const,
          message:
            "no lowered table — lower a range to a frame first, then pick a cell",
        };
      }
      const sel = state.gridSelection;
      if (!sel || state.engine === null || state.activeSheet === null) {
        return { ok: false as const, message: "select a cell first" };
      }
      // The lowered table belongs to ONE (sheet, range). A style-from-cell
      // only makes sense over that table's own sheet.
      if (state.activeSheet !== lastLoweredTable.sheet) {
        return {
          ok: false as const,
          message:
            "the selected cell is on a different sheet than the lowered table",
        };
      }

      // Map the selection anchor (MODEL coords) → the lowered table cell
      // (row/col POSITION) via the engine's lowered IR for the bound range —
      // the SAME mapping the table pour uses (tableCellPositionOf). When the
      // selection is outside the lowered range, fall back to the table's
      // first cell (the honest derivable subset — explicit in the UI wording).
      let content;
      try {
        content = state.engine.getRangeLowered(
          lastLoweredTable.sheet,
          lastLoweredTable.range,
          { includeGridRules: true },
        );
      } catch (err) {
        host.log.warn("newCellStyleFromSelection: lower read failed", err);
        return { ok: false as const, message: "could not read the lowered range" };
      }
      const pos =
        tableCellPositionOf(content, sel.anchorRow, sel.anchorCol) ??
        // Outside the lowered range → first cell of the table (documented
        // residual: the per-cell mapping is exact for in-range cells; this
        // keeps the affordance honest rather than guessing).
        { row: 0, col: 0 };
      const fromFirstCell =
        tableCellPositionOf(content, sel.anchorRow, sel.anchorCol) === null;

      const cellEid: ElementId = {
        kind: "tableCell",
        id: {
          story_id: lastLoweredTable.storyId,
          table_id: lastLoweredTable.tableId,
          row: pos.row,
          col: pos.col,
        },
      };

      // Read the cell's current properties (B-19). The engine may return a
      // thin entry set for a table cell (the Table NodeId surface is still
      // landing) — we carry whatever cell-appearance entries it gives.
      let entries: ReadEntry[] = [];
      try {
        const props = await host.document.elementProperties(cellEid);
        entries = (props?.entries ?? []) as ReadEntry[];
      } catch (err) {
        host.log.warn("newCellStyleFromSelection: elementProperties failed", err);
      }

      // Mint + populate the style (these DO land). The id is ours (selfId) —
      // createCellStyle may return createdId:null for collection creates.
      const styleId = `pgsheet.cellstyle.${Date.now().toString(36)}.${(
        nextCellStyleSeq++
      ).toString(36)}`;
      const plan = planCellStyleFromEntries(styleId, name, entries);

      try {
        const created = await host.document.mutate(plan.createOp);
        if (!created.applied) {
          host.log.warn("newCellStyleFromSelection: createCellStyle rejected", created);
          return { ok: false as const, message: "the host rejected createCellStyle" };
        }
        for (const op of plan.propertyOps) {
          const r = await host.document.mutate(op);
          if (!r.applied) {
            host.log.warn("newCellStyleFromSelection: setStyleProperty rejected", r);
          }
        }
      } catch (err) {
        host.log.error("newCellStyleFromSelection: mint/populate threw", err);
        return { ok: false as const, message: "minting the cell style failed" };
      }

      // Attempt the APPLY-BACK. wire.d.ts marks appliedCellStyle wire-shape-
      // only (UnsupportedProperty until the Table NodeId surface lands), so a
      // rejection here is EXPECTED — reported honestly, never faked.
      let applied = false;
      let applyMessage: string | null = null;
      try {
        const r = await host.document.mutate(plan.applyOp(cellEid));
        applied = r.applied;
        if (!r.applied) {
          applyMessage =
            "style created but not applied to the cell — appliedCellStyle is " +
            "not yet supported on table cells (Table NodeId surface pending)";
          host.log.info(`newCellStyleFromSelection: ${applyMessage}`);
        }
      } catch (err) {
        applyMessage =
          "style created but the apply-back threw (appliedCellStyle pending)";
        host.log.info("newCellStyleFromSelection: apply-back threw", err);
      }

      emitter.emit();
      return {
        ok: true as const,
        styleId,
        capturedCount: plan.capturedPaths.length,
        applied,
        applyMessage:
          applyMessage ?? (fromFirstCell ? "captured from the table's first cell" : null),
      };
    },

    discoverDatasets() {
      // S-15 — only ask the registry when a real one is wired
      // (`supports("dataProviders@1")`) AND the surface exists; both guard
      // the §2.1 graceful-absence posture (paged.data absent ⇒ no sources).
      // The host's gate also requires our `consume` capability; we declared
      // it, so discover is permitted.
      if (!host.supports("dataProviders@1") || !host.dataProviders) return [];
      try {
        return host.dataProviders.discover("dataset");
      } catch (err) {
        host.log.warn("discoverDatasets: discover failed", err);
        return [];
      }
    },

    async sourceFromDataset(providerId: string) {
      // S-15 — honest defer when no registry is wired (graceful absence,
      // like the existing honest-missing patterns). No surface ⇒ nothing
      // to source.
      if (!host.supports("dataProviders@1") || !host.dataProviders) {
        host.log.warn(
          `sourceFromDataset("${providerId}"): no data-provider registry ` +
            "wired (supports('dataProviders@1') is false) — install/enable " +
            "paged.data to source a sheet from a governed dataset",
        );
        return;
      }

      // Pull the resolved snapshot (the rows). The consumer NEVER fetches —
      // it receives an already-resolved RecordSet the platform hands it
      // (§1.1: paged.data owns the network + the §11 consent).
      let snapshot;
      try {
        snapshot = await host.dataProviders.get(providerId);
      } catch (err) {
        host.log.error(`sourceFromDataset("${providerId}"): get failed`, err);
        return;
      }
      if (!snapshot) {
        host.log.warn(
          `sourceFromDataset("${providerId}"): provider no longer exists`,
        );
        return;
      }

      // Boot a FRESH, EMPTY workbook (sheet 0 = "Sheet1") — a dataset-sourced
      // sheet replaces the workbook, it does not merge into the imported one.
      let engine: SheetEngine;
      try {
        engine = await bootEmptyEngine();
        state.bootError = null;
      } catch (err) {
        state.bootError = err instanceof Error ? err.message : ENGINE_NOT_BUILT;
        host.log.warn("sourceFromDataset: engine boot failed", err);
        emitter.emit();
        return;
      }

      // Tear down any prior workbook engine (we're replacing it).
      try {
        state.engine?.dispose();
      } catch (err) {
        host.log.warn("sourceFromDataset: prior engine dispose failed", err);
      }

      seedSheetFromRecords(engine, snapshot.records);

      state.engine = engine;
      state.activeSheet = 0;
      state.fileName = providerId;
      state.gridSelection = null;
      defaultRangeForActive();

      // Remember the linked (providerId, revision); the seeded values are
      // committed content (they travel with the document) — §1.1 honesty:
      // we do NOT auto-refetch. A later revision only MARKS the sheet stale;
      // re-sourcing is an explicit author action.
      state.dataSource = {
        providerId,
        revision: snapshot.revision,
        stale: false,
      };

      // Replace the prior dataset subscription with one for this provider.
      dataSourceSub?.dispose();
      dataSourceSub = host.dataProviders.onDidChange(providerId, (revision) => {
        if (state.dataSource?.providerId !== providerId) return;
        if (revision === state.dataSource.revision) return;
        state.dataSource = { ...state.dataSource, stale: true };
        host.log.info(
          `dataset "${providerId}" updated (revision ${revision}) — ` +
            "re-source to refresh (no auto-refetch, §1.1)",
        );
        emitter.emit();
      });

      emitter.emit();
    },

    saveWorkbook() {
      if (!state.engine) {
        host.log.warn("saveWorkbook: no workbook");
        return null;
      }
      try {
        const bytes = state.engine.saveXlsx();
        const base = (state.fileName ?? "workbook").replace(/\.xlsx$/i, "");
        return { bytes, fileName: `${base}.xlsx` };
      } catch (err) {
        host.log.error("saveWorkbook: engine save failed", err);
        return null;
      }
    },

    dispose() {
      // S-15 — drop the dataset revision subscription.
      try {
        dataSourceSub?.dispose();
      } catch (err) {
        host.log.warn("dataset subscription dispose failed", err);
      }
      dataSourceSub = null;
      // Disposing the scene-layer surface clears any in-frame grid it
      // submitted (the surface tracks + clears on dispose).
      try {
        sceneSurface?.dispose();
      } catch (err) {
        host.log.warn("scene-layer surface dispose failed", err);
      }
      sceneSurface = null;
      try {
        state.engine?.dispose();
      } catch (err) {
        host.log.warn("engine dispose failed", err);
      }
      state.engine = null;
      emitter.clear();
    },
  };
}
