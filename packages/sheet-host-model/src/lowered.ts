// LoweredContent — the TS MIRROR of the Rust lowering IR (sheet-lower,
// spec §8.2). The Rust side serialises camelCase (serde
// rename_all="camelCase"); this file is the host-side CONTRACT for the
// already-computed output the engine hands across the wasm door. It is
// PURE DATA — zero spreadsheet semantics (CLAUDE.md hard rule: all
// Excel-like work happens in the Rust crates; this is the shape the
// translator and the panel consume).
//
// Coordinates are content-space points (pt): widths/heights and rule
// offsets are relative to the lowered region's own top-left origin. The
// translator (lower-to-mutations.ts) adds the frame's page-local bounds
// origin to place them on the page (spec §8.5 content-space principle:
// the plugin always works in frame-content coordinates).
//
// CONTRACT NOTE (Phase-2 join): the `align` value is the lowercased serde
// rendering of the Rust `Align` enum — `rename_all="camelCase"` on the
// variants yields `"general"` (the Excel default), `"left"`, `"center"`,
// `"right"`. If the Rust side ends up emitting capitalised variants, the
// Phase-2 join fixes ONE side; THIS file is the authority for the wire
// shape and the translator reads it verbatim.

/** Horizontal cell alignment — the serde-camelCased `Align` variants.
 *  `"general"` is Excel's default (numbers right, text left), resolved
 *  in Rust; the lowered text is already the formatted value, so the
 *  translator treats alignment as an opaque tag it forwards. */
export type Align = "general" | "left" | "center" | "right";

/** One column's geometry: its sparse model index + lowered width (pt). */
export interface LoweredColumn {
  index: number;
  widthPt: number;
}

/** One lowered cell: the column it sits in, the FORMATTED text (the
 *  number-format output IS the text — spec §8.3), and its alignment. */
export interface LoweredCell {
  col: number;
  text: string;
  align: Align;
}

/** One row's geometry + its populated cells (sparse — empty cells are
 *  simply absent). */
export interface LoweredRow {
  index: number;
  heightPt: number;
  cells: LoweredCell[];
}

/** One grid rule (a drawn line). `at` is the offset along the rule's
 *  cross-axis (y for an h-rule, x for a v-rule); `from`..`to` is the
 *  extent along the rule's own axis. All content-space pt. */
export interface Rule {
  at: number;
  from: number;
  to: number;
}

/** The h/v rule sets (spec §8.2 grid rules, content-space). */
export interface Rules {
  /** Horizontal rules — run along x at a given y (`at`). */
  h: Rule[];
  /** Vertical rules — run along y at a given x (`at`). */
  v: Rule[];
}

/** A merged span anchored at its top-left cell (spec §8.2 merges). */
export interface Merge {
  row: number;
  col: number;
  rowSpan: number;
  colSpan: number;
}

/** The complete lowered region for one frame: column + row geometry,
 *  rules, and merges. The translator turns this into host mutations;
 *  it never computes any of it. */
export interface LoweredContent {
  cols: LoweredColumn[];
  rows: LoweredRow[];
  rules: Rules;
  merges: Merge[];
}

/** Total content width (pt) — the sum of column widths. Pure geometry,
 *  used by default placement. */
export function totalWidthPt(content: LoweredContent): number {
  return content.cols.reduce((sum, c) => sum + c.widthPt, 0);
}

/** Total content height (pt) — the sum of row heights. */
export function totalHeightPt(content: LoweredContent): number {
  return content.rows.reduce((sum, r) => sum + r.heightPt, 0);
}
