// The workbook session — the bundle's IN-MEMORY workbook handle (S-08:
// no persistence; workbook bytes NEVER touch host.storage — the panel
// says so). It holds the booted engine + the active sheet/range/file
// name, exposes import + lower + dispose, and emits a change signal the
// panel subscribes to. All spreadsheet work is the engine's; this is
// session bookkeeping + the host write path.

import type { BundleHost } from "@paged-media/plugin-api";

import { bootEngine, ENGINE_NOT_BUILT, type SheetEngine } from "./engine";
import { lowerSelectionToFrame } from "./lower";

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
}

export interface WorkbookSession {
  /** Read the current state snapshot. */
  state(): SessionState;
  /** Subscribe to state changes (the panel's render trigger). */
  onDidChange(listener: () => void): { dispose(): void };
  /** Import XLSX bytes under a display name: boots the engine on first
   *  use, loads the workbook, defaults the active sheet + range. */
  import(bytes: Uint8Array, name: string): Promise<void>;
  /** Set which sheet is active (and default its range to the used
   *  extent). */
  setActiveSheet(id: number): void;
  /** Set the A1 range the next lower will project. */
  setRange(range: string): void;
  /** Lower the active sheet's selected range to a new page frame
   *  (the two-phase flow in lower.ts). Returns the created frame id. */
  lowerSelection(): Promise<string | null>;
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
  };

  function defaultRangeForActive(): void {
    if (!state.engine || state.activeSheet === null) return;
    const sheet = state.engine
      .listSheets()
      .find((s) => s.id === state.activeSheet);
    if (sheet) state.selectedRange = usedRangeA1(sheet.rows, sheet.cols);
  }

  return {
    state: () => state,
    onDidChange: (l) => emitter.on(l),

    async import(bytes, name) {
      try {
        if (!state.engine) state.engine = await bootEngine();
        state.bootError = null;
      } catch (err) {
        // Boot failure (the artifact isn't built — S-10). Surface it; the
        // panel renders the honest "not built" state.
        state.engine = null;
        state.bootError =
          err instanceof Error ? err.message : ENGINE_NOT_BUILT;
        host.log.warn("sheet engine boot failed", err);
        emitter.emit();
        return;
      }
      try {
        state.engine.loadXlsx(bytes);
        state.fileName = name;
        const sheets = state.engine.listSheets();
        state.activeSheet = sheets.length > 0 ? sheets[0].id : null;
        defaultRangeForActive();
      } catch (err) {
        host.log.error("workbook import failed", err);
        state.fileName = null;
        state.activeSheet = null;
        state.selectedRange = null;
      }
      emitter.emit();
    },

    setActiveSheet(id) {
      state.activeSheet = id;
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
      return lowerSelectionToFrame(
        host,
        state.engine,
        state.activeSheet,
        state.selectedRange,
      );
    },

    dispose() {
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
