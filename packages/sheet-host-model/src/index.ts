// @paged-media/sheet-host-model — PURE translation layer: the engine's
// LoweredContent IR (computed in Rust, sheet-lower) → host Mutation[].
// Zero host imports, zero spreadsheet semantics (CLAUDE.md hard rule:
// all Excel-like operations live in the Rust crates).

export {
  type Align,
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
