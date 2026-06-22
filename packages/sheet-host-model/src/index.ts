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

// @paged-media/sheet-host-model — PURE translation layer: the engine's
// LoweredContent IR (computed in Rust, sheet-lower) → host Mutation[].
// Zero host imports, zero spreadsheet semantics (CLAUDE.md hard rule:
// all Excel-like operations live in the Rust crates).

export {
  type Align,
  type DataBarRect,
  type LoweredCell,
  type LoweredColumn,
  type LoweredRow,
  type LoweredContent,
  type LoweredStyle,
  type Merge,
  type Page,
  type Rule,
  type Rules,
  totalHeightPt,
  totalWidthPt,
} from "./lowered";

export {
  BINDING_KEY,
  BINDING_VERSION,
  makeBinding,
  parseBinding,
  type Binding,
  type BindingData,
} from "./binding";

export {
  defaultPlacement,
  DEFAULT_INSET_PT,
  MAX_HEIGHT_PT,
  MAX_WIDTH_PT,
  type Bounds,
  type Placement,
} from "./placement";

export {
  joinText,
  lowerToMutations,
  styleEmissions,
  styleProps,
  type BlockedFacet,
  type LowerPlacement,
  type LowerResult,
  type StyleEmission,
  type StyleProp,
} from "./lower-to-mutations";

export {
  CELL_EDGE_STROKE_PT,
  columnOrder,
  pageTableMutations,
  tableCellOps,
  tableCellPositionOf,
  tableContentBatch,
  tableDecorOps,
  tableInsertOp,
  type PageTableOps,
  type TableDecor,
} from "./lower-to-table";

export {
  isCellStylePath,
  planCellStyleFromEntries,
  type CellStylePlan,
  type ReadEntry,
} from "./cell-style";

export {
  DEFAULT_GRID_SVG_OPTIONS,
  cellEditorRect,
  cssColorToScenePaint,
  gridSceneToSceneLayer,
  gridSceneToSvg,
  hitCell,
  selectionRect,
  viewportHeightPt,
  viewportWidthPt,
  type GridCell,
  type GridCommentMarker,
  type GridDataBar,
  type GridFreeze,
  type GridScene,
  type GridSelection,
  type GridSvgOptions,
  type GridViewport,
} from "./grid";

export {
  applyCompletion,
  arityHint,
  completionTokenAt,
  matchFunctions,
  type CompletionToken,
  type FunctionEntry,
} from "./completions";

export {
  chartGeometryToMutations,
  type ChartGeometry,
  type ChartLowerResult,
  type ChartPlacement,
  type ChartPrimitive,
  type ChartTextLabel,
  type LinePrim,
  type PolygonPrim,
  type RectPrim,
  type TextAnchor,
  type TextPrim,
  type WedgePrim,
} from "./chart";
