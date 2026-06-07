// @paged-media/sheet-bundle — manifest + activate(host) for paged.sheet.
// Thin glue ONLY: lifecycle, panel, file input, engine boot, mutation
// submission. All spreadsheet semantics live in the Rust wasm engine.

import { defineBundle } from "@paged-media/plugin-sdk";
import type { PluginManifest } from "@paged-media/plugin-api";

import { activate } from "./activate";
import manifestJson from "../manifest.json";

export const sheetBundle = defineBundle({
  manifest: manifestJson as PluginManifest,
  activate,
});

export { activate, PANEL_ID } from "./activate";

// The engine facade + boot (S-10), exported for the engine spec.
export {
  bootEngine,
  wrapEngine,
  ENGINE_NOT_BUILT,
  type CellChange,
  type LowerOptions,
  type SheetEngine,
  type SheetInfo,
  type SheetWasmEngine,
  type SheetWasmModule,
} from "./engine";

// The in-memory workbook session (S-08), exported for the flow spec.
export {
  createWorkbookSession,
  columnLabel,
  usedRangeA1,
  type SessionState,
  type WorkbookSession,
} from "./session";

// The two-phase page lower (S-03), exported for the flow spec.
export { lowerSelectionToFrame } from "./lower";
export { importXlsx } from "./import-xlsx";
export { makeWorkbookPanel } from "./panels/workbook-panel";
