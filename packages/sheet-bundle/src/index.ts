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
  type CellEditRecord,
  type ChartInfo,
  type FindMatch,
  type FindOptions,
  type FrameBox,
  type FunctionInfo,
  type GridSceneOptions,
  type LowerOptions,
  type PaginateOptions,
  type ReplaceResult,
  type SheetEngine,
  type SheetInfo,
  type SkippedCell,
  type SortResult,
  type SheetWasmEngine,
  type SheetWasmModule,
} from "./engine";

// The in-memory workbook session (S-08), exported for the flow spec.
export {
  createWorkbookSession,
  cellToString,
  columnLabel,
  usedRangeA1,
  selectionRangeA1,
  tsvToRows,
  type SessionState,
  type WorkbookSession,
} from "./session";

// The native-table page lower (S-03 RESOLVED; tab-text lane retained as
// the explicit fallback), exported for the flow spec.
export {
  lowerSelectionToFrame,
  type LoweredTableInfo,
  type LowerLane,
  type LowerLaneOptions,
} from "./lower";
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
