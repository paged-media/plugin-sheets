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
 *  number-format output IS the text — spec §8.3), and its alignment.
 *  `styleKey` (IR v2, M1 style-map track) indexes into
 *  `LoweredContent.styles`; `0` is the default style. ADDITIVE — optional
 *  here so hand-written fixtures stay valid; the Rust engine always emits
 *  it (`0` in T0). */
export interface LoweredCell {
  col: number;
  text: string;
  align: Align;
  styleKey?: number;
}

/** One visual cell style (IR v2, M1 style-map track) — the mirror of the
 *  Rust `LoweredStyle`. `key` indexes `LoweredContent.styles`; the rest is
 *  a flat, host-ready style description. T0 emits only the default (key 0,
 *  all-false / all-undefined); the style-map track fills real entries in
 *  Phase B. ADDITIVE on the wire. */
export interface LoweredStyle {
  key: number;
  bold: boolean;
  italic: boolean;
  fontSizePt?: number | null;
  fontName?: string | null;
  fillRgb?: string | null;
  textRgb?: string | null;
  borderTop: boolean;
  borderRight: boolean;
  borderBottom: boolean;
  borderLeft: boolean;
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

/** One conditional-formatting DATA BAR as a drawn rect (spec §8.2/§10.4 — the
 *  page-draw geometry lane). The rect is in content-space pt (origin = the
 *  region's top-left, the same system as `Rule`); `fillFraction` is the bar's
 *  value share of the cell width (`w` already encodes it), `fill` is the bar
 *  colour as `#RRGGBB`. The translator lowers each to a `paged.draw`
 *  `insertPath` (the native-vector lane, NOT a style fill). Mirror of the Rust
 *  `sheet_lower::DataBarRect`. */
export interface DataBarRect {
  row: number;
  col: number;
  x: number;
  y: number;
  w: number;
  h: number;
  fillFraction: number;
  fill: string;
}

/** The complete lowered region for one frame: column + row geometry,
 *  rules, and merges. The translator turns this into host mutations;
 *  it never computes any of it. `styles` (IR v2, M1 style-map track) is
 *  the style table `LoweredCell.styleKey` indexes; key 0 is the default.
 *  ADDITIVE — optional so existing fixtures stay valid; the Rust engine
 *  always emits it (a single default entry in T0). */
export interface LoweredContent {
  cols: LoweredColumn[];
  rows: LoweredRow[];
  rules: Rules;
  merges: Merge[];
  styles?: LoweredStyle[];
  /** Conditional-formatting data bars as drawn rects (spec §8.2 page-draw
   *  lane). ADDITIVE — optional so existing fixtures stay valid; the engine
   *  emits it (empty unless a data-bar cf rule covers numeric cells). */
  databars?: DataBarRect[];
}

/** One paginated frame (the TS MIRROR of the Rust `sheet_lower::Page`,
 *  Wave 2D / S-05). The engine threads a tall range across the host frame
 *  chain's content boxes and returns one `Page` per filled frame — each a
 *  self-contained `LoweredContent` (header band + that frame's body rows,
 *  re-based to 0) plus the chain index it targets and continuation flags.
 *  PURE DATA — the host translator compiles `content` exactly like a
 *  single-frame lowering. serde camelCase on the Rust side. */
export interface Page {
  /** Index into the caller's frame list (the chain order) this page fills. */
  frameIndex: number;
  /** The lowered content for this frame (header band + body rows, re-based). */
  content: LoweredContent;
  /** `true` when more body rows follow on a later frame (drives the
   *  continued-marker chrome; only ever set when the option is on). */
  continued: boolean;
  /** `true` when this frame holds a single block/row taller than the whole
   *  frame — the spec's pathological case, placed alone and flagged. */
  oversize: boolean;
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
