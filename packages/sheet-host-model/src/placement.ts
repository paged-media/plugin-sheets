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

// Default frame placement — where a freshly lowered sheet frame lands on
// the page. PURE geometry over the lowered IR's total size: no host
// import, no engine call. The translator (lower-to-mutations.ts) takes
// the bounds this returns as the frame's page-local box and adds it as
// the origin for the content-space rule offsets (spec §8.5).

import type { PageId } from "@paged-media/plugin-api";

import { totalHeightPt, totalWidthPt, type LoweredContent } from "./lowered";

/** Page-local frame bounds, `[top, left, bottom, right]` in pt — the
 *  same order every wire frame op uses (`insertTextFrame` etc.). */
export type Bounds = [number, number, number, number];

/** A resolved placement: the page to insert into + the frame's bounds. */
export interface Placement {
  pageId: PageId;
  bounds: Bounds;
}

/** Fixed inset from the page origin for a default-placed frame (pt). */
export const DEFAULT_INSET_PT = 24;

/** Clamp so a huge range doesn't produce an off-page frame on first
 *  drop — the user repositions/reflows after (T1 pagination splits a
 *  tall range across linked frames; T0 places one bounded frame). */
export const MAX_WIDTH_PT = 540;
export const MAX_HEIGHT_PT = 720;

/** Default placement for a newly lowered frame: a box sized to the
 *  content's total width/height (summed from the IR), clamped to a
 *  sane maximum, at a fixed inset from the page's top-left. Returns
 *  page-local `[top, left, bottom, right]`.
 *
 *  `fit` — the native-table lane passes what the engine's layout adds
 *  to the IR's row heights ({@link TableFit}); the box is then as tall
 *  as the table the engine will lay out ({@link tableFrameHeightPt}),
 *  not as tall as the workbook's rows. Without it the height is the IR
 *  sum (the tab-text lane, and any caller that sizes the box itself). */
export function defaultPlacement(
  pageId: PageId,
  content: LoweredContent,
  fit?: TableFit,
): Placement {
  const width = Math.min(totalWidthPt(content), MAX_WIDTH_PT);
  const height = Math.min(
    fit ? tableFrameHeightPt(content, fit) : totalHeightPt(content),
    MAX_HEIGHT_PT,
  );
  const top = DEFAULT_INSET_PT;
  const left = DEFAULT_INSET_PT;
  return {
    pageId,
    bounds: [top, left, top + height, left + width],
  };
}

// ── Sizing a frame to the native table the engine lays out ──────────────────
//
// The IR's row heights are the WORKBOOK's (Excel's 15 pt for its 11 pt
// Calibri). A native table on the page is laid out by the engine, and two
// things make it taller than that sum — measured against the real engine,
// and the reason the film's 7-row price list (105 pt of rows) overset in a
// 105 pt frame:
//
//   1. THE LEAD-IN. `insertTable` appends the table on a fresh paragraph
//      after the frame story's empty first one, so the table starts one
//      line DOWN the frame. Core sets that blank first line on its face's
//      ascender (the frame's ascent first baseline), advances one auto
//      leading (120 % of the size), and backs the table up by 80 % of the
//      engine's default size — so the table's top sits at
//      `ascender + 1.2·s − 0.8·s` = `ascender + 0.4·s` with s = 12 pt.
//      Measured: 14.4 pt with the engine's metric-less estimate (ascender
//      0.8·s), 16.43 pt with Inter (ascender 11.625 pt), against a
//      predicted 14.4 / 16.425.
//   2. ROW GROWTH. A row is never shorter than its tallest cell needs, and
//      a one-line cell needs its first baseline — the face's ascender at
//      the size it is poured at (cells carry no insets).
//
// Both are the ENGINE's layout rules, read here so the frame can be placed
// at the right size in the one placement batch. When core stops leaving the
// blank paragraph (the table placed in the story's first paragraph), the
// lead-in goes away and `lower-fit.spec.ts` — which pins the frame to within
// a point of what the engine needs — says so.

/** The engine's default point size — the size the blank lead-in line and
 *  every un-sized cell are laid out at (`PipelineOptions::default_point_size`). */
export const ENGINE_DEFAULT_SIZE_PT = 12;

/** Rounding allowance (pt) on a fitted table frame: the engine places lines
 *  on a 1/64 pt grid and rounds the cursor, so a frame cut EXACTLY to the
 *  predicted height can miss its last row by a hundredth of a point. Half a
 *  point is invisible and always enough. */
export const FIT_ALLOWANCE_PT = 0.5;

/** What the engine's layout adds to a lowered region's IR geometry. */
export interface TableFit {
  /** The ascender (pt) of the face an un-styled run resolves to, at
   *  {@link ENGINE_DEFAULT_SIZE_PT} — what the blank lead-in line sits on. */
  defaultAscenderPt: number;
  /** The ascender (pt) of the face and size cell `cell` of row `row` is
   *  poured at — the height a one-line cell needs. Absent: the row keeps
   *  its IR height. */
  cellAscenderPt?: (row: number, col: number) => number;
}

/** The blank line the engine lays out above a table placed in a fresh
 *  frame (see the section note): `ascender + 0.4 × 12 pt`. */
export function tableLeadInPt(fit: TableFit): number {
  return fit.defaultAscenderPt + 0.4 * ENGINE_DEFAULT_SIZE_PT;
}

/** The height of each lowered row as the engine lays it out: never less
 *  than the IR's (workbook) height, never less than its tallest cell's
 *  first baseline. */
export function tableRowHeightsPt(content: LoweredContent, fit: TableFit): number[] {
  return content.rows.map((row, r) => {
    let h = row.heightPt;
    if (fit.cellAscenderPt) {
      for (const cell of row.cells) {
        if (cell.text.length === 0) continue;
        h = Math.max(h, fit.cellAscenderPt(r, cell.col));
      }
    }
    return h;
  });
}

/** The frame height (pt) that holds a lowered region's native table: the
 *  engine's lead-in line, every row at the height the engine gives it, and
 *  the rounding allowance. An empty region needs no frame. PURE. */
export function tableFrameHeightPt(content: LoweredContent, fit: TableFit): number {
  if (content.rows.length === 0) return 0;
  const rows = tableRowHeightsPt(content, fit).reduce((sum, h) => sum + h, 0);
  return tableLeadInPt(fit) + rows + FIT_ALLOWANCE_PT;
}
