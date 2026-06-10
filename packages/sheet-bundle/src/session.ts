// The workbook session — the bundle's IN-MEMORY workbook handle (S-08:
// no persistence; workbook bytes NEVER touch host.storage — the panel
// says so). It holds the booted engine + the active sheet/range/file
// name, exposes import + lower + dispose, and emits a change signal the
// panel subscribes to. All spreadsheet work is the engine's; this is
// session bookkeeping + the host write path.

import type { BundleHost, SceneLayerSurface } from "@paged-media/plugin-api";
import {
  gridSceneToSceneLayer,
  type GridScene,
  type GridSelection,
} from "@paged-media/sheet-host-model";

import {
  bootEngine,
  ENGINE_NOT_BUILT,
  type ChartInfo,
  type SheetEngine,
} from "./engine";
import { lowerSelectionToFrame } from "./lower";
import { lowerChartToFrame } from "./lower-chart";

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
  /** Re-emit the loaded workbook as XLSX bytes for the exporter
   *  contribution (S-06). Preservation-first (`engine.saveXlsx` — the
   *  lazy-verbatim re-emit, §10.2). Returns the bytes + a suggested file
   *  name, or null when there is no workbook (nothing to export). */
  saveWorkbook(): { bytes: Uint8Array; fileName: string } | null;
  /** Tear down: free the engine, drop listeners. */
  dispose(): void;
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

export function createWorkbookSession(host: BundleHost): WorkbookSession {
  const emitter = new Emitter();
  const state: SessionState = {
    engine: null,
    fileName: null,
    activeSheet: null,
    selectedRange: null,
    bootError: null,
    gridSelection: null,
  };

  // C-1 / S-02 — the last frame this session lowered into (the target for
  // the in-frame grid) + the lazily-obtained scene-layer surface.
  let lastFrameId: string | null = null;
  let sceneSurface: SceneLayerSurface | null = null;
  const sceneChannel = (): SceneLayerSurface | null => {
    if (!host.supports("rendering.sceneLayer@1")) return null;
    if (!sceneSurface) sceneSurface = host.contribute.sceneLayer();
    return sceneSurface;
  };

  function defaultRangeForActive(): void {
    if (!state.engine || state.activeSheet === null) return;
    const sheet = state.engine
      .listSheets()
      .find((s) => s.id === state.activeSheet);
    if (sheet) state.selectedRange = usedRangeA1(sheet.rows, sheet.cols);
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

  /** S-08: persist the imported bytes + name to `host.blob` (best-effort —
   *  never let a persist failure break an import). Per-plugin keyed: the
   *  LAST imported workbook is the one restored on reload. */
  async function persistWorkbook(bytes: Uint8Array, name: string): Promise<void> {
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
      return loadWorkbook(bytes, name, false);
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
      const scene = computeGridScene(0, 0, wPt, hPt);
      if (!scene) {
        host.log.warn("showGridInFrame: grid windowing failed");
        return false;
      }
      try {
        await surface.submit(lastFrameId, gridSceneToSceneLayer(scene));
      } catch (err) {
        host.log.error("showGridInFrame: submit failed", err);
        return false;
      }
      return true;
    },

    hideGridInFrame() {
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
    },

    editCell(sheet, row, col, input) {
      if (!state.engine || state.activeSheet === null) {
        host.log.warn("editCell: no workbook / sheet");
        return false;
      }
      try {
        state.engine.setCell(sheet, row, col, input);
      } catch (err) {
        host.log.error("editCell: engine setCell failed", err);
        return false;
      }
      // The dirty cut recomputed in Rust; refresh the panel (it re-requests
      // the windowed scene on the next render).
      emitter.emit();
      return true;
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
