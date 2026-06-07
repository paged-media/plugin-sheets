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
  type Merge,
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
  type LowerPlacement,
  type LowerResult,
} from "./lower-to-mutations";
