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

export {
  activate,
  PANEL_ID,
  GRID_PANEL_ID,
  DATASETS_PANEL_ID,
} from "./activate";

// The engine facade + boot (S-10), exported for the engine spec.
export {
  bootEngine,
  bootEmptyEngine,
  wrapEngine,
  ENGINE_NOT_BUILT,
  type CellChange,
  type ChartInfo,
  type FrameBox,
  type GridSceneOptions,
  type LowerOptions,
  type PaginateOptions,
  type SheetEngine,
  type SheetInfo,
  type SheetWasmEngine,
  type SheetWasmModule,
} from "./engine";

// The in-memory workbook session (S-08), exported for the flow spec.
export {
  createWorkbookSession,
  cellToString,
  columnLabel,
  usedRangeA1,
  type SessionState,
  type WorkbookSession,
} from "./session";

// The two-phase page lower (S-03), exported for the flow spec.
export { lowerSelectionToFrame } from "./lower";
// Live multi-frame pagination across the host frame chain (Wave 2D, S-05).
export {
  lowerPaginatedToChain,
  resolveChain,
  subscribeChainReflow,
  type ChainLowerResult,
} from "./lower";
// The chart → paged.draw vector lower (M2 charts track, spec §8.4).
export { lowerChartToFrame } from "./lower-chart";
export { importXlsx } from "./import-xlsx";
export { makeWorkbookPanel } from "./panels/workbook-panel";
export { makeGridPanel } from "./panels/grid-panel";
export { makeDatasetsPanel } from "./panels/datasets-panel";
