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

// The workbook session — the bundle's workbook handle. It holds the
// booted engine + the active sheet/range/file name, exposes import +
// lower + dispose, and emits a change signal the panel subscribes to.
// Persistence (S-08 + the container part): the import writes the bytes,
// and every committed edit re-saves the workbook (debounced, flushed on
// dispose) — see `markEdited`. All spreadsheet work is the engine's; this is
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
  workbookPalette,
  type ChartGeometry,
  type FunctionEntry,
  type GridCell,
  type GridScene,
  type GridSelection,
  type LoweredContent,
  type PaletteEntry,
} from "../../sheet-host-model/src";

import {
  bootEmptyEngine,
  bootEngine,
  ENGINE_NOT_BUILT,
  type CalcSettingsInfo,
  type CellEditRecord,
  type ChartInfo,
  type FindMatch,
  type FindOptions,
  type SheetEngine,
  type SheetInfo,
  type StructuralEditKind,
} from "./engine";
import {
  lowerPaginatedToChain,
  lowerSelectionToFrame,
  pageContent,
  placementForContent,
  refreshLoweredTable,
  selectionAnchor,
  storyOfFrame,
  subscribeChainReflow,
  type ChainSubscription,
  type LoweredTableInfo,
} from "./lower";
import {
  CHART_SIZE_PT,
  lowerChartToFrame,
  removePlacedChart,
  type PlacedChart,
} from "./lower-chart";
import { readWorkbookPart, writeWorkbookPart } from "./workbook-part";
import {
  planCellStyleFromEntries,
  tableCellPositionOf,
  type ReadEntry,
} from "../../sheet-host-model/src";

/** How long a burst of committed edits settles before the placed tables,
 *  charts and paginated chains are refreshed ONCE (Wave 4). */
export const REFRESH_DEBOUNCE_MS = 300;

/** A session verb's outcome: done, or the reason it was not. */
export type SessionResult = { ok: true } | { ok: false; message: string };

/** S-08 persistence keys: the workbook bytes live in `host.blob` (binary),
 *  its display name in the KV `host.storage`. Per-plugin — the last
 *  imported workbook is the one restored on reload. */
const BLOB_KEY = "workbook";
const BLOB_NAME_KEY = "workbook.name";

/** How long after the last committed edit the workbook is re-saved to the
 *  container part + blob. One `saveXlsx` per burst of edits, not per
 *  keystroke-commit: the save re-emits the whole workbook. */
export const PERSIST_DEBOUNCE_MS = 750;

/** The nominal box the palette probe asks chart geometry for. A chart's
 *  COLOURS do not depend on its size — only the primitive coordinates do
 *  — so the palette read passes a fixed box rather than pretending to
 *  know a frame it is not rendering into. */
const PALETTE_PROBE_WPT = 400;
const PALETTE_PROBE_HPT = 300;

/** What a change signal carried (Wave 2) — so a listener that only cares
 *  about some cells can skip the rest. Anything not classified is
 *  `other`, which every listener must treat as "anything may have
 *  changed".
 *
 *  - `selection`: only the grid selection moved.
 *  - `cells`: only the VALUES / inputs of these cells changed (an edit,
 *    paste, sort, replace, undo/redo of those) — no style, sheet or
 *    workbook structure. `regions` bound every written cell. */
export type SessionChange =
  | { kind: "selection" }
  | { kind: "cells"; regions: readonly CellRegion[] }
  | { kind: "other" };

/** A rectangle of cells on one sheet (inclusive bounds). */
export interface CellRegion {
  sheet: number;
  firstRow: number;
  firstCol: number;
  lastRow: number;
  lastCol: number;
}

const OTHER_CHANGE: SessionChange = { kind: "other" };

/** The regions (one bounding box per sheet) covering `cells`. */
export function regionsOf(
  cells: readonly { sheet: number; row: number; col: number }[],
): CellRegion[] {
  const bySheet = new Map<number, CellRegion>();
  for (const { sheet, row, col } of cells) {
    const r = bySheet.get(sheet);
    if (!r) {
      bySheet.set(sheet, { sheet, firstRow: row, firstCol: col, lastRow: row, lastCol: col });
      continue;
    }
    r.firstRow = Math.min(r.firstRow, row);
    r.firstCol = Math.min(r.firstCol, col);
    r.lastRow = Math.max(r.lastRow, row);
    r.lastCol = Math.max(r.lastCol, col);
  }
  return [...bySheet.values()];
}

/** Run `cb` at the next animation frame — or, with no
 *  `requestAnimationFrame` (a worker, the headless host), after the
 *  current microtask turn. A hidden tab never fires rAF, so a timeout
 *  races it: the work still lands, just not frame-aligned. */
function nextFrame(cb: () => void): void {
  const raf = (globalThis as { requestAnimationFrame?: (f: () => void) => unknown })
    .requestAnimationFrame;
  if (typeof raf !== "function") {
    queueMicrotask(cb);
    return;
  }
  let ran = false;
  const once = () => {
    if (ran) return;
    ran = true;
    cb();
  };
  raf(once);
  setTimeout(once, 100);
}

/** A tiny synchronous event emitter (one channel: "did the session
 *  state change"). Avoids dragging a dependency for a single signal. */
class Emitter {
  private listeners = new Set<(change: SessionChange) => void>();
  /** Runs before the listeners on every emit (the session drops its read
   *  caches there, so no listener can read a stale answer). */
  constructor(private readonly beforeEmit?: () => void) {}
  on(listener: (change: SessionChange) => void): { dispose(): void } {
    this.listeners.add(listener);
    return {
      dispose: () => {
        this.listeners.delete(listener);
      },
    };
  }
  emit(change: SessionChange = OTHER_CHANGE): void {
    this.beforeEmit?.();
    for (const l of [...this.listeners]) l(change);
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
  /** Subscribe to state changes (the panel's render trigger). The
   *  listener receives what changed ({@link SessionChange}); `other`
   *  means anything may have. */
  onDidChange(listener: (change: SessionChange) => void): { dispose(): void };
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
  /** ADR 023 — the WORKBOOK PALETTE: every colour this workbook makes
   *  paged.sheet mint as a DOCUMENT swatch (chart series + conditional-
   *  format data bars), at the deterministic ids the lowering uses.
   *
   *  This is what the host Swatches panel binds to while the `sheet`
   *  edit context is active. It is derived, never stored: the colours
   *  are decided in Rust (`sheet-chart`'s palette, `sheet-lower`'s
   *  data-bar rules) and `sheet-host-model/palette.ts` only projects
   *  them onto core's swatch vocabulary. Empty when no workbook is
   *  loaded, or when the workbook has no charts and no data bars. */
  workbookPalette(): PaletteEntry[];
  /** Lower a parsed chart to a paged.draw vector frame (spec §8.4 — the
   *  two-phase flow in lower-chart.ts). `chartIndex` indexes [`listCharts`].
   *  Returns false when there is no engine or the lower fails. */
  lowerChart(chartIndex: number): Promise<boolean>;
  /** The chart-kind tags the ENGINE accepts, in panel order (empty when no
   *  workbook is loaded). The panel renders this list rather than a hard-coded
   *  one, so the kind vocabulary has exactly one home: Rust. */
  chartKinds(): string[];
  /** AUTHOR a chart over the ACTIVE sheet's live data (editor-ui-coverage
   *  M — the model was import-only). `seriesIn` is the transpose control
   *  (`"columns"` default / `"rows"`). Page-side only: never written back
   *  to the xlsx (the writer re-derives no chart parts). */
  authorChart(
    values: string,
    categories: string,
    kind: string,
    title: string,
    seriesIn?: string,
  ): { ok: true; index: number } | { ok: false; message: string };
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
  /** The workbook's sheets (`engine.listSheets`), memoised until the next
   *  change signal — the panels read it on every render. Empty when there
   *  is no engine (never throws). */
  sheets(): readonly SheetInfo[];
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
   *  engine's boundary message (e.g. "sort over a spilled region not
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
   *  The engine applies `appliedCellStyle` on a `tableCell` element (S-04,
   *  probe-verified; the old "wire-shape-only" note in wire.d.ts is stale).
   *  A host can still refuse the apply, so the outcome is reported as
   *  `applied` true/false rather than assumed.
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
  /** ADR 023 — WHAT THE HOST'S CHARACTER/PARAGRAPH PANELS ARE ABOUT
   *  while the `sheet` context is active: the (sheet, A1 range) whose
   *  cell text formatting those panels should show.
   *
   *  Two cases, and the second is the one that makes the panels useful
   *  rather than empty:
   *
   *    · a CELL SELECTION exists → exactly those cells;
   *    · none yet → THE WHOLE RANGE THE ENTERED FRAME PROJECTS. This is
   *      not a fallback, it is the analogue of selecting a text frame
   *      with the selection tool: the panel is about the frame's whole
   *      content, and reports MIXED wherever that content disagrees.
   *
   *  Null when there is no workbook / active sheet / nothing lowered. */
  textSelectionRange(): { sheet: number; range: string } | null;
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
  /** Write any committed-but-unsaved edits to the container part + blob
   *  NOW (cancelling the pending debounce) and resolve when every queued
   *  write has landed. Edits persist on their own after
   *  [`PERSIST_DEBOUNCE_MS`] and on `dispose`; this is the explicit door
   *  for a host save. (The plugin contract has no pre-save hook yet —
   *  RFI: a host `onWillSave` would call this.) Never rejects. */
  flushPersist(): Promise<void>;
  /** Tear down: flush unsaved edits (the bytes are taken synchronously,
   *  the write completes in the background), free the engine, drop
   *  listeners. */
  dispose(): void;

  // ── Wave 4 ──────────────────────────────────────────────────────────

  /** Paginate the active range across the threaded frames of the selected
   *  text frame (or, with none selected, the frame last placed into), live:
   *  a resize of a chain frame re-paginates (debounced) and REPLACES the
   *  tables; workbook edits refresh them too. */
  paginateSelection(): Promise<SessionResult>;
  /** Bring every placed table, chart and paginated chain up to date with
   *  the workbook now (edits schedule this on their own, debounced). */
  refreshPlacements(): Promise<void>;
  /** Replace the workbook with a new blank one (one sheet, "Book1"). */
  newWorkbook(): Promise<void>;
  /** Import delimited text (CSV/TSV) as a one-sheet workbook. The host
   *  language types numbers and dates; `name`'s extension picks the
   *  delimiter (.tsv → tab), otherwise it is sniffed. */
  importCsv(text: string, name: string): Promise<void>;
  /** Add a worksheet (empty name = next free `SheetN`) and make it active. */
  addSheet(name?: string): SessionResult;
  /** Rename a worksheet. */
  renameSheet(id: number, name: string): SessionResult;
  /** Delete a worksheet (references to it become #REF!). */
  deleteSheet(id: number): SessionResult;
  /** Insert/delete rows or columns at the grid selection (its rows / cols
   *  are the count). Clears the cell-edit journal (its addresses moved). */
  structuralEdit(kind: StructuralEditKind): SessionResult;
  /** The workbook's iteration settings (`<calcPr>`), or null. */
  calcSettings(): CalcSettingsInfo | null;
  /** Toggle iterative calculation (persisted into `<calcPr>`). */
  setIterative(on: boolean): SessionResult;
  /** The workbook content version (bumps on every committed edit) — the
   *  number placed frames' bindings carry. */
  contentVersion(): number;
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
  // Panel-facing engine READS, memoised until the next change signal
  // (every engine write is followed by one): a panel re-renders on every
  // keystroke in its own inputs, and each render used to re-window the
  // grid scene, re-list the sheets and re-read the selected cell's input
  // through the wasm boundary.
  const readCache = new Map<string, unknown>();
  function cachedRead<T>(key: string, read: () => T): T {
    if (readCache.has(key)) return readCache.get(key) as T;
    const v = read();
    readCache.set(key, v);
    return v;
  }
  const emitter = new Emitter(() => readCache.clear());
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
  // ADR 023 — frameId → the table that frame projects. `lastLoweredTable`
  // answers "the most recent lowering", which is the right question for
  // S-04 and the WRONG one for a panel retargeting on the frame you just
  // entered: with two sheet frames on the page they disagree.
  const loweredTables = new Map<string, LoweredTableInfo>();
  // Wave 4 — the charts this session placed (their elements, so a refresh
  // can replace them) and the live paginated chains (storyId → handle).
  let placedCharts: PlacedChart[] = [];
  const chains = new Map<
    string,
    { sub: ChainSubscription; sheet: number; range: string }
  >();
  // The workbook content version: +1 per committed edit. Placed frames'
  // bindings carry the version they show (it was always 0 before).
  let revision = 0;
  let refreshTimer: ReturnType<typeof setTimeout> | null = null;
  let refreshChain: Promise<void> = Promise.resolve();
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
    const engine = state.engine;
    if (!engine) return false;
    let prev: string;
    try {
      prev = writeOneCell(engine, sheet, row, col, input);
    } catch (err) {
      host.log.error("setCell failed", err);
      return false;
    }
    editJournal.length = journalCursor; // drop any redo tail
    editJournal.push({ sheet, row, col, prev, next: input });
    journalCursor = editJournal.length;
    markEdited();
    return true;
  }

  /** Write one cell and return its PRIOR input (the journal's inverse).
   *  The batch door is preferred: its slim `{changedCount, circular,
   *  prevInputs}` reply carries the prior input, so the write costs one
   *  engine call (no read first, no changed cell's display marshalled
   *  back). An engine without the door — or one that refuses the input as a
   *  batch, or whose reply predates `prevInputs` (then the prior is the
   *  memoised read the formula bar usually made a moment ago) — reads the
   *  prior and takes `setCell`. */
  function writeOneCell(
    engine: SheetEngine,
    sheet: number,
    row: number,
    col: number,
    input: string,
  ): string {
    const readPrev = () =>
      cachedRead(`input:${sheet}:${row}:${col}`, () => engine.getCellInput(sheet, row, col));
    if (engine.setCells) {
      let res: ReturnType<NonNullable<SheetEngine["setCells"]>> | undefined;
      // An engine whose reply has no `prevInputs` must be read BEFORE the
      // write; we only learn that from a reply, so a memoised read (free
      // when the bar just showed the cell) is the safe fallback for it.
      try {
        res = engine.setCells([{ sheet, row, col, input }]);
      } catch {
        // fall through: setCell is the authority on a single input
      }
      if (res) {
        const prev = res.prevInputs?.[0];
        if (prev !== undefined) return prev;
        host.log.warn("setCells reply has no prevInputs — the journal reads the new input");
        return readPrev();
      }
    }
    const prev = readPrev();
    engine.setCell(sheet, row, col, input);
    return prev;
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
    markEdited();
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
    emitter.emit({ kind: "selection" });
  }

  /** The in-frame grid as it is NOW, submitted once per animation frame
   *  (Wave 2). Every edit, keystroke and selection change asks for it;
   *  the asks inside one frame (one microtask turn where there is no
   *  `requestAnimationFrame`, e.g. a worker or the headless host) collapse
   *  into ONE window + submit of the final state — typing a burst of
   *  characters no longer submits the whole grid per character. The
   *  returned promise settles with that submit's outcome. Never throws. */
  let gridSubmitPending: Promise<boolean> | null = null;
  function submitInFrameGrid(): Promise<boolean> {
    if (gridSubmitPending) return gridSubmitPending;
    const pending = new Promise<boolean>((resolve) => {
      nextFrame(() => {
        // Asks arriving while this submit is in flight schedule the next.
        gridSubmitPending = null;
        void flushInFrameGrid().then(resolve);
      });
    });
    gridSubmitPending = pending;
    return pending;
  }

  /** Re-window + submit the in-frame grid for `lastGridWindow` (carrying
   *  the current selection and the open cell edit). Caches `lastGridScene`
   *  for the next hit-test. Returns false when there is no target frame /
   *  scene channel / window / engine. Never throws.
   *
   *  The scene contract (`SceneLayerSurface.submit`) replaces the frame's
   *  whole layer — there is no per-item patch — so a keystroke still
   *  re-sends the window; what it no longer does is re-window it: the
   *  engine scene is memoised until the next change signal, and the edit
   *  buffer is overlaid on a copy. */
  async function flushInFrameGrid(): Promise<boolean> {
    if (!lastFrameId || !lastGridWindow) return false;
    const surface = sceneChannel();
    if (!surface) return false;
    const base = computeGridScene(
      lastGridWindow.firstRow,
      lastGridWindow.firstCol,
      lastGridWindow.wPt,
      lastGridWindow.hPt,
    );
    if (!base) return false;
    // K-1 — overlay the in-progress cell-edit text on its cell (the engine
    // scene still shows the COMMITTED value; the buffer is uncommitted).
    const scene = cellEdit
      ? withCellText(base, cellEdit.row, cellEdit.col, cellEdit.text)
      : base;
    lastGridScene = scene;
    try {
      await surface.submit(lastFrameId, gridSceneToSceneLayer(scene));
    } catch (err) {
      host.log.error("showGridInFrame: submit failed", err);
      return false;
    }
    return true;
  }

  /** `scene` with `(row, col)` rendering `text` — the uncommitted cell-edit
   *  buffer. A copy: `scene` is the memoised engine window. Replaces the
   *  cell if present in the window, else appends a left-aligned one so an
   *  edit on an empty cell still shows. */
  function withCellText(
    scene: GridScene,
    row: number,
    col: number,
    text: string,
  ): GridScene {
    let found = false;
    const cells = scene.cells.map((c) => {
      if (c.row !== row || c.col !== col) return c;
      found = true;
      return { ...c, text };
    });
    if (!found) {
      const cell: GridCell = { row, col, text, align: "left", styleKey: 0 };
      cells.push(cell);
    }
    return { ...scene, cells };
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
    // Header row (row 0) — the schema field names; body rows 1.. — column-
    // major: columns[c][r] is the cell value for data-row r, which lands
    // on sheet row r + 1.
    const inputs: { sheet: number; row: number; col: number; input: string }[] = [];
    for (let c = 0; c < fields.length; c++) {
      inputs.push({ sheet: 0, row: 0, col: c, input: fields[c].name });
    }
    for (let c = 0; c < records.columns.length; c++) {
      const col = records.columns[c];
      for (let r = 0; r < records.rowCount; r++) {
        inputs.push({ sheet: 0, row: r + 1, col: c, input: cellToString(col[r]) });
      }
    }
    // ONE write + ONE recalc through the batch door when the engine has it;
    // per cell otherwise, or when the batch refuses an input (a bad cell is
    // then skipped alone).
    if (engine.setCells) {
      try {
        engine.setCells(inputs);
        return;
      } catch (err) {
        host.log.debug("sourceFromDataset: batch seed refused — per cell", err);
      }
    }
    for (const i of inputs) writeCell(engine, i.row, i.col, i.input);
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
    const engine = state.engine;
    const sheet = state.activeSheet;
    if (!engine || sheet === null) return null;
    // Memoised until the next change signal (a selection change is one),
    // so callers must treat the scene as read-only.
    return cachedRead(`scene:${sheet}:${firstRow}:${firstCol}:${wPt}:${hPt}`, () => {
      let scene: GridScene;
      try {
        scene = engine.getGridScene(sheet, firstRow, firstCol, wPt, hPt, {
          includeGridlines: true,
        });
      } catch (err) {
        host.log.warn("gridScene: engine windowing failed", err);
        return null;
      }
      if (state.gridSelection) scene.selection = state.gridSelection;
      return scene;
    });
  }

  /** S-08: persist the imported bytes + name (best-effort — never let a
   *  persist failure break an import). Per-plugin keyed: the LAST imported
   *  workbook is the one restored on reload.
   *
   *  Two homes, by design: the `.paged` container PART (the portable one —
   *  it travels WITH the document, the read PREFERENCE on restore) and the
   *  per-browser `host.blob` (a fast local cache + backward-compat for hosts
   *  with no container writer). */
  async function writeWorkbook(bytes: Uint8Array, name: string): Promise<void> {
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

  // Edit persistence. Every committed engine write (cell edit, paste,
  // sort, replace, undo/redo, dataset seed) calls `markEdited`; after
  // PERSIST_DEBOUNCE_MS the workbook is re-saved (`engine.saveXlsx`, the
  // preservation-first re-emit) and written to both homes. Writes are
  // serialised on one chain so an older snapshot can never land after a
  // newer one. Until 2026-10-04 only the import wrote, so a reload lost
  // every edit since.
  let persistTimer: ReturnType<typeof setTimeout> | null = null;
  let persistDirty = false;
  let persistChain: Promise<void> = Promise.resolve();

  function enqueueWrite(bytes: Uint8Array, name: string): Promise<void> {
    persistChain = persistChain.then(() => writeWorkbook(bytes, name));
    return persistChain;
  }

  function cancelPendingPersist(): void {
    if (persistTimer !== null) clearTimeout(persistTimer);
    persistTimer = null;
    persistDirty = false;
  }

  function markEdited(): void {
    readCache.clear();
    if (!state.engine) return;
    revision += 1;
    scheduleRefresh();
    persistDirty = true;
    if (persistTimer !== null) clearTimeout(persistTimer);
    persistTimer = setTimeout(() => {
      persistTimer = null;
      void flushPersist();
    }, PERSIST_DEBOUNCE_MS);
  }

  /** Take the bytes NOW (synchronous — safe right before the engine is
   *  freed) and queue the write. */
  function flushPersist(): Promise<void> {
    if (!persistDirty || !state.engine) {
      cancelPendingPersist();
      return persistChain;
    }
    cancelPendingPersist();
    let bytes: Uint8Array;
    try {
      bytes = state.engine.saveXlsx();
    } catch (err) {
      // Keep the edit marked: the next edit or flush tries again.
      persistDirty = true;
      host.log.warn("workbook persist: engine save failed (kept in memory)", err);
      return persistChain;
    }
    return enqueueWrite(bytes, state.fileName ?? "workbook.xlsx");
  }

  /** Wave 4 — after a burst of edits, refresh what this session placed. */
  function scheduleRefresh(): void {
    if (loweredTables.size === 0 && placedCharts.length === 0 && chains.size === 0) {
      return;
    }
    if (refreshTimer !== null) clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => {
      refreshTimer = null;
      void refreshPlacements();
    }, REFRESH_DEBOUNCE_MS);
  }

  /** The name of sheet `id` (bindings carry names), or null if it is gone. */
  function sheetNameOf(id: number): string | null {
    return state.engine?.listSheets().find((s) => s.id === id)?.name ?? null;
  }

  /** Refresh every placed table (in place), chart (replaced) and chain
   *  (re-paginated over its own tables). Serialised: a refresh requested
   *  while one runs queues behind it. Never rejects. */
  function refreshPlacements(): Promise<void> {
    refreshChain = refreshChain.then(async () => {
      const engine = state.engine;
      if (!engine) return;
      const version = revision;
      for (const [frameId, info] of [...loweredTables]) {
        const name = sheetNameOf(info.sheet);
        if (name === null || !info.content) continue;
        let next: LoweredContent;
        try {
          next = pageContent(engine, info.sheet, info.range);
        } catch (err) {
          host.log.warn(`refresh: could not lower ${info.range}`, err);
          continue;
        }
        if (
          info.contentVersion === version ||
          JSON.stringify(next) === JSON.stringify(info.content)
        ) {
          continue;
        }
        const updated = await refreshLoweredTable(host, info, next, version, name);
        loweredTables.set(frameId, updated);
        if (lastLoweredTable?.frameId === frameId) lastLoweredTable = updated;
      }
      const charts: PlacedChart[] = [];
      for (const placed of placedCharts) {
        if (placed.contentVersion === version || placed.elementIds.length === 0) {
          charts.push(placed);
          continue;
        }
        let key: string;
        try {
          key = JSON.stringify(
            engine.getChartGeometry(placed.chartIndex, ...CHART_SIZE_PT),
          );
        } catch {
          charts.push(placed);
          continue;
        }
        if (key === placed.geometryKey) {
          // The edit did not touch this chart's data: nothing to redraw.
          charts.push({ ...placed, contentVersion: version });
          continue;
        }
        await removePlacedChart(host, placed);
        let replaced: PlacedChart | null = null;
        await lowerChartToFrame(host, engine, placed.chartIndex, {
          placement: { pageId: placed.pageId, bounds: placed.bounds },
          contentVersion: version,
          onLowered: (p) => {
            replaced = p;
          },
        });
        charts.push(replaced ?? { ...placed, elementIds: [] });
      }
      placedCharts = charts;
      for (const chain of chains.values()) {
        await chain.sub.refresh();
      }
    }).catch((err) => {
      host.log.warn("refresh of placed content failed", err);
    });
    return refreshChain;
  }

  /** The placements belong to the workbook they were lowered from: a new
   *  workbook (import, blank, CSV, dataset) forgets them. */
  function forgetPlacements(): void {
    if (refreshTimer !== null) clearTimeout(refreshTimer);
    refreshTimer = null;
    lastLoweredTable = null;
    loweredTables.clear();
    placedCharts = [];
    for (const c of chains.values()) c.sub.dispose();
    chains.clear();
  }

  /** Make `engine` the session's workbook (a blank or CSV one): the old
   *  one is freed, placements and journal forgotten, persisted as an edit. */
  function adoptWorkbook(engine: SheetEngine, name: string): void {
    if (state.engine && state.engine !== engine) {
      try {
        state.engine.dispose();
      } catch (err) {
        host.log.warn("prior engine dispose failed", err);
      }
    }
    cancelPendingPersist();
    forgetPlacements();
    state.engine = engine;
    state.fileName = name;
    state.gridSelection = null;
    state.dataSource = null;
    editJournal = [];
    journalCursor = 0;
    const sheets = engine.listSheets();
    state.activeSheet = sheets.length > 0 ? sheets[0].id : null;
    defaultRangeForActive();
    markEdited();
    emitter.emit();
  }

  /** Run a workbook-structure verb; a throw becomes `{ok:false}`. */
  function structureVerb(fn: (engine: SheetEngine) => void): SessionResult {
    if (!state.engine) return { ok: false, message: "no workbook open" };
    try {
      fn(state.engine);
    } catch (err) {
      return { ok: false, message: err instanceof Error ? err.message : String(err) };
    }
    markEdited();
    emitter.emit();
    return { ok: true };
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
      forgetPlacements(); // the prior placements belonged to the old workbook
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
    // A newly loaded workbook replaces the old one: unsaved edits to the
    // old one are moot (and must not overwrite the import below).
    cancelPendingPersist();
    if (persist) await enqueueWrite(bytes, name);
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
      // Wave 4 — land at the current selection, not a fixed page inset.
      let placement;
      let content: LoweredContent;
      try {
        content = pageContent(state.engine, state.activeSheet, state.selectedRange);
        placement = await placementForContent(host, content);
      } catch (err) {
        host.log.warn("lowerSelection: could not lower the range", err);
        return null;
      }
      const id = await lowerSelectionToFrame(
        host,
        state.engine,
        state.activeSheet,
        state.selectedRange,
        {
          placement: placement ?? undefined,
          contentVersion: revision,
          content,
          // S-04 — record the resolved native table so "new style from cell"
          // can address its cells.
          onLowered: (info) => {
            lastLoweredTable = info;
            loweredTables.set(info.frameId, info);
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
      const sheet = state.activeSheet;
      if (state.engine && sheet !== null) {
        journaledSetCell(sheet, row, col, text);
        emitter.emit({ kind: "cells", regions: regionsOf([{ sheet, row, col }]) });
      } else {
        emitter.emit();
      }
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

    workbookPalette() {
      const engine = state.engine;
      if (!engine) return [];
      // CHART colours. The geometry is size-parameterised, but a
      // chart's PALETTE is not — the size only moves the primitives —
      // so a fixed nominal box is used rather than a real frame's,
      // which the palette read has no business knowing about.
      const charts: ChartGeometry[] = [];
      try {
        for (const info of engine.listCharts()) {
          charts.push(
            engine.getChartGeometry(
              info.index,
              PALETTE_PROBE_WPT,
              PALETTE_PROBE_HPT,
            ),
          );
        }
      } catch (err) {
        host.log.warn("workbookPalette: chart geometry read failed", err);
      }
      // DATA-BAR colours, from the currently selected region — the one
      // the user is looking at, and the one a lower would emit swatches
      // for. A conditional-format rule outside the selection has no
      // lowered rect, so it has no swatch to show.
      const regions: LoweredContent[] = [];
      if (state.activeSheet !== null && state.selectedRange) {
        try {
          regions.push(
            engine.getRangeLowered(state.activeSheet, state.selectedRange),
          );
        } catch (err) {
          host.log.warn("workbookPalette: region lower failed", err);
        }
      }
      return workbookPalette({ charts, regions });
    },

    chartKinds() {
      const engine = state.engine;
      if (!engine) return [];
      return cachedRead("chartKinds", () => {
        try {
          return engine.chartKinds() ?? [];
        } catch (err) {
          host.log.warn("chartKinds: engine call failed", err);
          return [];
        }
      });
    },

    authorChart(values, categories, kind, title, seriesIn) {
      if (!state.engine || state.activeSheet === null) {
        return { ok: false as const, message: "no workbook loaded" };
      }
      try {
        const index = state.engine.addChart(
          state.activeSheet,
          values,
          categories,
          kind,
          title,
          seriesIn ?? "columns",
        );
        emitter.emit();
        return { ok: true as const, index };
      } catch (err) {
        return {
          ok: false as const,
          message: err instanceof Error ? err.message : String(err),
        };
      }
    },

    async lowerChart(chartIndex) {
      if (!state.engine) {
        host.log.warn("lowerChart: no workbook");
        return false;
      }
      const at = await selectionAnchor(host);
      return lowerChartToFrame(host, state.engine, chartIndex, {
        placement: at
          ? { pageId: at.pageId, bounds: [at.top, at.left, at.top, at.left] }
          : undefined,
        contentVersion: revision,
        onLowered: (placed) => {
          placedCharts.push(placed);
        },
      });
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
      emitter.emit({ kind: "cells", regions: regionsOf([{ sheet, row, col }]) });
      return true;
    },

    cellInputAt(row, col) {
      // S-04 formula bar — re-enterable input (engine.getCellInput), so the
      // bar shows a cell's FORMULA, not its computed display. Never throws.
      const engine = state.engine;
      const sheet = state.activeSheet;
      if (!engine || sheet === null) return "";
      return cachedRead(`input:${sheet}:${row}:${col}`, () => {
        try {
          return engine.getCellInput(sheet, row, col) ?? "";
        } catch (err) {
          host.log.warn("cellInputAt: engine read failed", err);
          return "";
        }
      });
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
      const engine = state.engine;
      const edits: CellEditRecord[] = [];
      let written = 0;
      const cells: { row: number; col: number; next: string }[] = [];
      for (let r = 0; r < grid.length; r++) {
        const row = grid[r];
        for (let c = 0; c < row.length; c++) {
          cells.push({ row: sel.anchorRow + r, col: sel.anchorCol + c, next: row[c] });
        }
      }
      // ONE write + ONE recalc through the batch door when the engine has
      // it; its reply carries every cell's prior input (the journal's
      // inverse), so nothing is read first. It refuses the whole batch on
      // any bad input — then the cells go one by one (prior read per cell),
      // so a single bad cell is skipped, not the paste.
      let batched = false;
      if (engine.setCells && cells.length > 0) {
        try {
          const res = engine.setCells(
            cells.map((t) => ({ sheet, row: t.row, col: t.col, input: t.next })),
          );
          batched = true;
          written = cells.length;
          const prev = res.prevInputs;
          if (prev && prev.length === cells.length) {
            cells.forEach((t, i) =>
              edits.push({ sheet, row: t.row, col: t.col, prevInput: prev[i], nextInput: t.next }),
            );
          } else {
            // An engine whose reply predates `prevInputs`: the cells are
            // written but their priors are gone — journal nothing rather
            // than an undo that would restore the wrong text.
            host.log.warn("pasteAtSelection: setCells reply has no prevInputs — paste not journaled");
          }
        } catch (err) {
          host.log.debug("pasteAtSelection: batch write refused — per cell", err);
        }
      }
      if (!batched) {
        for (const t of cells) {
          let prev: string;
          try {
            prev = engine.getCellInput(sheet, t.row, t.col);
          } catch (err) {
            host.log.warn(`pasteAtSelection: read (${sheet},${t.row},${t.col}) failed`, err);
            continue;
          }
          try {
            engine.setCell(sheet, t.row, t.col, t.next);
          } catch (err) {
            host.log.warn(`pasteAtSelection: setCell(${sheet},${t.row},${t.col}) failed`, err);
            continue;
          }
          edits.push({ sheet, row: t.row, col: t.col, prevInput: prev, nextInput: t.next });
          written += 1;
        }
      }
      if (written === 0) {
        return { ok: false as const, message: "the paste wrote no cells" };
      }
      journalBatch(edits); // one grouped Cmd-Z undoes the whole paste
      const touched: CellEditRecord[] =
        edits.length > 0
          ? edits
          : cells.map((t) => ({ sheet, row: t.row, col: t.col, prevInput: "", nextInput: t.next }));
      emitter.emit({ kind: "cells", regions: regionsOf(touched) });
      void submitInFrameGrid();
      return {
        ok: true as const,
        rows: grid.length,
        cols: Math.max(...grid.map((r) => r.length)),
      };
    },

    sheets() {
      const engine = state.engine;
      if (!engine) return [];
      return cachedRead("sheets", () => {
        try {
          return engine.listSheets();
        } catch (err) {
          host.log.warn("sheets: engine read failed", err);
          return [];
        }
      });
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
        emitter.emit({ kind: "cells", regions: [] }); // only the buffer went
        void submitInFrameGrid();
        return true;
      }
      if (journalCursor === 0 || !state.engine) return false;
      // A batched group (sort / replace-all) unwinds WHOLE — one undo step;
      // plain entries (no batch) unwind singly. Cells in a batch are
      // disjoint, so reverse-order re-entry is order-independent.
      const group = editJournal[journalCursor - 1].batch;
      const undone: { sheet: number; row: number; col: number }[] = [];
      do {
        const entry = editJournal[journalCursor - 1];
        undone.push(entry);
        try {
          state.engine.setCell(entry.sheet, entry.row, entry.col, entry.prev);
        } catch (err) {
          host.log.error("undoCellEdit: engine setCell failed", err);
          return false;
        }
        markEdited();
        journalCursor -= 1;
      } while (
        group !== undefined &&
        journalCursor > 0 &&
        editJournal[journalCursor - 1].batch === group
      );
      emitter.emit({ kind: "cells", regions: regionsOf(undone) });
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
      const redone: { sheet: number; row: number; col: number }[] = [];
      do {
        const entry = editJournal[journalCursor];
        redone.push(entry);
        try {
          state.engine.setCell(entry.sheet, entry.row, entry.col, entry.next);
        } catch (err) {
          host.log.error("redoCellEdit: engine setCell failed", err);
          return false;
        }
        markEdited();
        journalCursor += 1;
      } while (
        group !== undefined &&
        journalCursor < editJournal.length &&
        editJournal[journalCursor].batch === group
      );
      emitter.emit({ kind: "cells", regions: regionsOf(redone) });
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
        emitter.emit({ kind: "cells", regions: regionsOf(res.edits) });
        void submitInFrameGrid();
        return { ok: true };
      } catch (err) {
        // The honest boundary (e.g. "sort over a spilled region not
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
        emitter.emit({ kind: "cells", regions: regionsOf(res.edits) });
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

      // The APPLY-BACK. The engine applies appliedCellStyle on a tableCell;
      // a host that refuses it is reported, never faked.
      let applied = false;
      let applyMessage: string | null = null;
      try {
        const r = await host.document.mutate(plan.applyOp(cellEid));
        applied = r.applied;
        if (!r.applied) {
          applyMessage =
            "style created but not applied to the cell — the host refused " +
            "appliedCellStyle";
          host.log.info(`newCellStyleFromSelection: ${applyMessage}`);
        }
      } catch (err) {
        applyMessage =
          "style created but applying it threw (appliedCellStyle)";
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

      cancelPendingPersist(); // the prior workbook is replaced
      forgetPlacements();
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

      // The seeded workbook is document content: persist it like an edit.
      markEdited();
      emitter.emit();
    },

    textSelectionRange() {
      if (!state.engine || state.activeSheet === null) return null;
      const sel = state.gridSelection;
      if (sel) {
        return {
          sheet: state.activeSheet,
          range: selectionRangeA1(sel.anchorRow, sel.anchorCol, sel.rows, sel.cols),
        };
      }
      // No cell picked yet — the panels are about the ENTERED frame's
      // whole projected range. Keyed on the frame the in-frame grid is
      // showing, not on "the last thing lowered": with two sheet frames
      // on the page, the last-lowered one is the WRONG answer for the
      // one you double-clicked.
      const entered = lastFrameId ? loweredTables.get(lastFrameId) : null;
      if (entered) return { sheet: entered.sheet, range: entered.range };
      if (state.selectedRange)
        return { sheet: state.activeSheet, range: state.selectedRange };
      return null;
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

    flushPersist() {
      return flushPersist().catch((err) => {
        host.log.warn("workbook persist failed", err);
      });
    },

    async paginateSelection() {
      const engine = state.engine;
      if (!engine || state.activeSheet === null || !state.selectedRange) {
        return { ok: false as const, message: "no workbook / sheet / range" };
      }
      const selected = host.selection
        .get()
        .find((id) => id.kind === "textFrame");
      const frameId = selected ? (selected.id as string) : lastFrameId;
      if (!frameId) {
        return {
          ok: false as const,
          message: "select a text frame of the chain to paginate into",
        };
      }
      const storyId = await storyOfFrame(host, frameId);
      if (!storyId) {
        return { ok: false as const, message: "the selected frame has no story" };
      }
      const sheet = state.activeSheet;
      const range = state.selectedRange;
      // Re-paginating the same chain replaces the old placement's handle
      // (its tables are refreshed by the new pass, not duplicated).
      const prior = chains.get(storyId);
      const from = prior?.sub.current() ?? null;
      prior?.sub.dispose();
      const first = from
        ? null
        : await lowerPaginatedToChain(host, engine, sheet, range, storyId);
      if (!from && !first) {
        return { ok: false as const, message: "the frame threads no chain" };
      }
      const sub = subscribeChainReflow(host, engine, sheet, range, storyId, {
        from: from ?? first,
      });
      chains.set(storyId, { sub, sheet, range });
      if (from) await sub.refresh();
      return { ok: true as const };
    },

    refreshPlacements() {
      return refreshPlacements();
    },

    async newWorkbook() {
      let engine: SheetEngine;
      try {
        engine = await bootEmptyEngine();
        state.bootError = null;
      } catch (err) {
        state.bootError = err instanceof Error ? err.message : ENGINE_NOT_BUILT;
        host.log.warn("newWorkbook: engine boot failed", err);
        emitter.emit();
        return;
      }
      adoptWorkbook(engine, "Book1.xlsx");
    },

    async importCsv(text, name) {
      let engine: SheetEngine;
      try {
        engine = await bootEmptyEngine();
        state.bootError = null;
      } catch (err) {
        state.bootError = err instanceof Error ? err.message : ENGINE_NOT_BUILT;
        host.log.warn("importCsv: engine boot failed", err);
        emitter.emit();
        return;
      }
      const base = name.replace(/\.[^.]*$/, "");
      const delimiter = /\.tsv$/i.test(name) ? "\t" : "";
      const locale =
        (typeof navigator !== "undefined" && navigator.language) || "en-US";
      try {
        if (!engine.loadCsv) throw new Error("engine wasm predates load_csv");
        engine.loadCsv(text, delimiter, locale, base);
      } catch (err) {
        host.log.error("CSV import failed", err);
        engine.dispose();
        return;
      }
      adoptWorkbook(engine, `${base}.xlsx`);
    },

    addSheet(name) {
      let id = -1;
      const r = structureVerb((e) => {
        if (!e.addSheet) throw new Error("engine wasm predates add_sheet");
        id = e.addSheet(name ?? "");
      });
      if (r.ok) {
        state.activeSheet = id;
        state.gridSelection = null;
        defaultRangeForActive();
        emitter.emit();
      }
      return r;
    },

    renameSheet(id, name) {
      return structureVerb((e) => {
        if (!e.renameSheet) throw new Error("engine wasm predates rename_sheet");
        e.renameSheet(id, name);
      });
    },

    deleteSheet(id) {
      const r = structureVerb((e) => {
        if (!e.deleteSheet) throw new Error("engine wasm predates delete_sheet");
        e.deleteSheet(id);
      });
      if (r.ok) {
        // Sheet ids shifted; the journal and placements on that sheet are moot.
        editJournal = [];
        journalCursor = 0;
        for (const [k, info] of [...loweredTables]) {
          if (info.sheet === id) loweredTables.delete(k);
          else if (info.sheet > id) loweredTables.set(k, { ...info, sheet: info.sheet - 1 });
        }
        state.activeSheet = 0;
        state.gridSelection = null;
        defaultRangeForActive();
        emitter.emit();
      }
      return r;
    },

    structuralEdit(kind) {
      const sel = state.gridSelection;
      if (state.activeSheet === null) {
        return { ok: false as const, message: "no sheet" };
      }
      if (!sel) {
        return { ok: false as const, message: "select the rows or columns first" };
      }
      const rows = kind === "insertRows" || kind === "deleteRows";
      const at = rows ? sel.anchorRow : sel.anchorCol;
      const n = Math.max(1, rows ? sel.rows : sel.cols);
      const sheet = state.activeSheet;
      const r = structureVerb((e) => {
        if (!e.structuralEdit) throw new Error("engine wasm predates structural_edit");
        e.structuralEdit(sheet, kind, at, n);
      });
      if (r.ok) {
        // The undo journal addresses cells by position — they moved.
        editJournal = [];
        journalCursor = 0;
        defaultRangeForActive();
        emitter.emit();
      }
      return r;
    },

    calcSettings() {
      const engine = state.engine;
      if (!engine) return null;
      return cachedRead("calcSettings", () => {
        try {
          return engine.calcSettings?.() ?? null;
        } catch {
          return null;
        }
      });
    },

    setIterative(on) {
      return structureVerb((e) => {
        const cur = e.calcSettings?.();
        if (!e.setIterative || !cur) throw new Error("engine wasm predates set_iterative");
        e.setIterative(on, cur.maxIter, cur.maxChange);
      });
    },

    contentVersion() {
      return revision;
    },

    dispose() {
      forgetPlacements();
      // Flush unsaved edits BEFORE the engine is freed: the bytes are
      // taken synchronously here; the write finishes in the background.
      void flushPersist().catch((err) => {
        host.log.warn("workbook persist on dispose failed", err);
      });
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
