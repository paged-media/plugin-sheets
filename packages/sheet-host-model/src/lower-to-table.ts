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

// Native-table page lowering (S-03 RESOLVED — core protocol v37 added
// `Mutation::InsertTable`). This REPLACES the tab-text + drawn-rules
// degradation (`lower-to-mutations.ts`, kept as the old-engine fallback):
// the engine's lowered IR becomes a real Paged `<Table>` — rows/cells
// with font-metric column widths (S-13), selectable + printable +
// IDML-round-tripping + transforming with the frame.
//
// PURE: data in, Mutation[] out. No host calls — the caller (lower.ts)
// measures column widths via host.text.measureString and passes them in,
// and resolves the storyId/tableId between phases. THREE phases:
//   frame (insertTextFrame) → table (insertTable in the frame's story)
//   → content (one batch: insertText per cell addressed via TextCellAddr,
//   setCellSpan per merge, and tableCell-scoped setElementProperty for
//   cell fills + edge strokes).
//
// CELL ADDRESSING: the wire's `ElementId` carries a `tableCell` kind
// (`{ story_id, table_id, row, col }`), so the facets the tab-text
// degradation had to report as "blocked" — fill background + per-edge
// borders — now LAND natively (`cellFillColor`, `cell*EdgeStrokeWeight`).
// Grid rules map onto cell edges (a rule at a row/column boundary becomes
// top/bottom/left/right edge strokes on the boundary's cells); a rule that
// aligns to NO boundary is counted `unmappedRules` (honest shortfall, the
// caller warns) rather than silently dropped.
//
// IR GAP (noted, not faked): the IR's `Rule` and `LoweredStyle` borders
// carry GEOMETRY/booleans only — no stroke weight or colour. The weight
// here is the documented hairline default below; colour stays the host
// default. Carrying weight/colour in the IR is a sheet-lower (Rust)
// follow-on, not a TS invention.
//
// ZERO spreadsheet semantics (CLAUDE.md hard rule): every value here is
// already-decided geometry/text from the Rust engine.

import type { ElementId, Mutation, PropertyPath } from "@paged-media/plugin-api";

import type { LoweredContent, Page } from "./lowered";

/** The table's column order (ascending model indices) — the mapping from
 *  a cell's model `col` to its 0-based table column position. */
export function columnOrder(content: LoweredContent): number[] {
  return content.cols.map((c) => c.index);
}

/** The 0-based table (row, col) POSITION of a model cell `(modelRow,
 *  modelCol)` in a lowered region, or null when that cell is outside the
 *  lowered range. This is the SAME mapping `tableCellOps`/`tableDecorOps`
 *  use to address cells (model col → its index in `columnOrder`; model row →
 *  its index in the lowered row order). It lets a consumer that knows a
 *  model coordinate (e.g. the grid selection) address the corresponding
 *  native `tableCell` in the frame this region lowered to (S-04). PURE. */
export function tableCellPositionOf(
  content: LoweredContent,
  modelRow: number,
  modelCol: number,
): { row: number; col: number } | null {
  const row = content.rows.findIndex((r) => r.index === modelRow);
  if (row < 0) return null;
  const col = columnOrder(content).indexOf(modelCol);
  if (col < 0) return null;
  return { row, col };
}

/** Phase-2 op: create the table in an already-resolved story. `rows`/`cols`
 *  are the lowered grid extent; `columnWidths` (pt) are caller-measured
 *  (font metrics, S-13), falling back to the IR's char-based widths;
 *  `rowHeights` (pt) come straight from the IR. Header/footer rows are 0 in
 *  T0 (repeated-header view options are a later wire-through). The host
 *  returns `createdId = tableId`. */
export function tableInsertOp(
  content: LoweredContent,
  storyId: string,
  columnWidths: number[],
): Mutation {
  return {
    op: "insertTable",
    args: {
      storyId,
      rows: content.rows.length,
      cols: content.cols.length,
      headerRows: 0,
      footerRows: 0,
      columnWidths,
      rowHeights: content.rows.map((r) => r.heightPt),
    },
  };
}

/** Phase-3 batch: pour each non-empty cell's formatted text into its table
 *  cell (`insertText` with the `TextCellAddr` qualifier). `tableId` is the
 *  resolved id from `tableInsertOp`'s outcome. Row/col are 0-based table
 *  positions (the IR row order; the column's position in `columnOrder`).
 *  One undoable step. */
export function tableCellOps(
  content: LoweredContent,
  storyId: string,
  tableId: string,
): Mutation {
  const colPos = new Map<number, number>();
  columnOrder(content).forEach((modelCol, i) => colPos.set(modelCol, i));

  const ops: Mutation[] = [];
  content.rows.forEach((row, r) => {
    for (const cell of row.cells) {
      const c = colPos.get(cell.col);
      if (c === undefined || cell.text.length === 0) continue;
      ops.push({
        op: "insertText",
        args: {
          storyId,
          offset: 0,
          text: cell.text,
          cell: { tableId, row: r, col: c },
        },
      });
    }
  });

  return { op: "batch", args: { ops } };
}

// ── Cell decor: merges, fills, edge strokes (the native S-03 upgrade) ───────

/** The hairline stroke weight (pt) for grid-rule / style-border edges. The
 *  IR carries border PRESENCE only (rule geometry, style booleans) — weight
 *  and colour are not lowered yet (an honest IR gap; see the header note).
 *  0.5 pt is the publishing hairline the drawn-rule degradation rendered. */
export const CELL_EDGE_STROKE_PT = 0.5;

/** Boundary-matching tolerance (pt) when aligning a rule's offset to the
 *  cumulative row/column boundaries. */
const RULE_EPS = 0.01;

/** A tableCell-scoped ElementId (the wire's cell-addressing door). */
function cellId(
  storyId: string,
  tableId: string,
  row: number,
  col: number,
): ElementId {
  return {
    kind: "tableCell",
    id: { story_id: storyId, table_id: tableId, row, col },
  };
}

/** One tableCell-scoped edge-stroke-weight override. */
function edgeOp(
  storyId: string,
  tableId: string,
  row: number,
  col: number,
  path: PropertyPath,
): Mutation {
  return {
    op: "setElementProperty",
    args: {
      elementId: cellId(storyId, tableId, row, col),
      path,
      value: { type: "length", value: CELL_EDGE_STROKE_PT },
    },
  };
}

/** Cumulative boundaries from a width/height list: `[0, w0, w0+w1, …]`. */
function boundaries(sizes: number[]): number[] {
  const out = [0];
  for (const s of sizes) out.push(out[out.length - 1] + s);
  return out;
}

/** Index of the boundary `at` aligns to (within RULE_EPS), or null. */
function boundaryIndex(bounds: number[], at: number): number | null {
  for (let i = 0; i < bounds.length; i++) {
    if (Math.abs(bounds[i] - at) <= RULE_EPS) return i;
  }
  return null;
}

/** Table positions whose `[bounds[i], bounds[i+1]]` extent overlaps
 *  `[from, to]` by more than RULE_EPS. */
function coveredPositions(bounds: number[], from: number, to: number): number[] {
  const out: number[] = [];
  for (let i = 0; i + 1 < bounds.length; i++) {
    const overlap = Math.min(bounds[i + 1], to) - Math.max(bounds[i], from);
    if (overlap > RULE_EPS) out.push(i);
  }
  return out;
}

/** The decor ops for one lowered region: `setCellSpan` per merged span,
 *  `cellFillColor` per filled cell, and `cell*EdgeStrokeWeight` per grid
 *  rule / style border — all addressed via the `tableCell` ElementId kind.
 *  `unmappedRules` counts rules that aligned to no row/column boundary
 *  (should not happen for engine-emitted grid rules; reported honestly). */
export interface TableDecor {
  ops: Mutation[];
  unmappedRules: number;
}

/** Build the decor ops (merges + fills + edge strokes) for a resolved
 *  table. PURE arithmetic over already-decided IR geometry:
 *
 *  - MERGES: the IR's range-relative `Merge` anchors map to table
 *    positions via the lowered row/column order; the span is the count of
 *    lowered rows/cols the model span covers → one `setCellSpan` each.
 *  - STYLES: a cell whose `LoweredStyle` carries `fillRgb` gets
 *    `cellFillColor`; its border booleans get per-edge stroke weights.
 *  - RULES: an h-rule at boundary k becomes the top edge of row 0 (k=0)
 *    or the bottom edge of row k-1, on every column its extent covers;
 *    v-rules symmetrically (left edge of col 0 / right edge of col j-1).
 *
 *  Edge ops are deduped per (row, col, edge); style borders and grid
 *  rules emit the same documented hairline weight. */
export function tableDecorOps(
  content: LoweredContent,
  storyId: string,
  tableId: string,
): TableDecor {
  const ops: Mutation[] = [];

  const colIdx = columnOrder(content);
  const colPos = new Map<number, number>();
  colIdx.forEach((modelCol, i) => colPos.set(modelCol, i));
  const rowPos = new Map<number, number>();
  content.rows.forEach((row, r) => rowPos.set(row.index, r));

  // (1) Merges → setCellSpan (anchored at the span's table position; the
  // span counts the LOWERED rows/cols the model span covers).
  for (const m of content.merges) {
    const r = rowPos.get(m.row);
    const c = colPos.get(m.col);
    if (r === undefined || c === undefined) continue; // anchor outside the grid
    const rowSpan = content.rows.filter(
      (row) => row.index >= m.row && row.index < m.row + m.rowSpan,
    ).length;
    const columnSpan = content.cols.filter(
      (col) => col.index >= m.col && col.index < m.col + m.colSpan,
    ).length;
    if (rowSpan <= 1 && columnSpan <= 1) continue; // degenerate span
    ops.push({
      op: "setCellSpan",
      args: { storyId, tableId, row: r, col: c, rowSpan, columnSpan },
    });
  }

  // (2 + 3) Fills and edges. Edge ops dedupe per (row, col, edge): grid
  // rules first, style borders overwrite (same weight — pure dedupe).
  const edges = new Map<string, Mutation>();
  const putEdge = (row: number, col: number, path: PropertyPath) => {
    edges.set(`${row}:${col}:${path}`, edgeOp(storyId, tableId, row, col, path));
  };

  const yBounds = boundaries(content.rows.map((r) => r.heightPt));
  const xBounds = boundaries(content.cols.map((c) => c.widthPt));
  let unmappedRules = 0;

  for (const rule of content.rules.h) {
    const k = boundaryIndex(yBounds, rule.at);
    if (k === null) {
      unmappedRules += 1;
      continue;
    }
    const row = k === 0 ? 0 : k - 1;
    const path: PropertyPath =
      k === 0 ? "cellTopEdgeStrokeWeight" : "cellBottomEdgeStrokeWeight";
    for (const col of coveredPositions(xBounds, rule.from, rule.to)) {
      putEdge(row, col, path);
    }
  }
  for (const rule of content.rules.v) {
    const k = boundaryIndex(xBounds, rule.at);
    if (k === null) {
      unmappedRules += 1;
      continue;
    }
    const col = k === 0 ? 0 : k - 1;
    const path: PropertyPath =
      k === 0 ? "cellLeftEdgeStrokeWeight" : "cellRightEdgeStrokeWeight";
    for (const row of coveredPositions(yBounds, rule.from, rule.to)) {
      putEdge(row, col, path);
    }
  }

  const fills: Mutation[] = [];
  const styleByKey = new Map((content.styles ?? []).map((s) => [s.key, s]));
  content.rows.forEach((row, r) => {
    for (const cell of row.cells) {
      const c = colPos.get(cell.col);
      if (c === undefined) continue;
      const style =
        cell.styleKey != null && cell.styleKey !== 0
          ? styleByKey.get(cell.styleKey)
          : undefined;
      if (!style) continue;
      if (style.fillRgb != null) {
        fills.push({
          op: "setElementProperty",
          args: {
            elementId: cellId(storyId, tableId, r, c),
            path: "cellFillColor",
            value: { type: "colorRef", value: style.fillRgb },
          },
        });
      }
      if (style.borderTop) putEdge(r, c, "cellTopEdgeStrokeWeight");
      if (style.borderBottom) putEdge(r, c, "cellBottomEdgeStrokeWeight");
      if (style.borderLeft) putEdge(r, c, "cellLeftEdgeStrokeWeight");
      if (style.borderRight) putEdge(r, c, "cellRightEdgeStrokeWeight");
    }
  });

  ops.push(...fills, ...edges.values());
  return { ops, unmappedRules };
}

/** The full phase-3 content batch: the cell pour (`tableCellOps`) followed
 *  by the decor ops (spans, fills, edge strokes) — ONE undoable step. Text
 *  pours BEFORE spans so cell addressing targets the pre-merge grid (span
 *  anchors are the spans' top-left cells, the only ones the IR populates). */
export function tableContentBatch(
  content: LoweredContent,
  storyId: string,
  tableId: string,
): { batch: Mutation; unmappedRules: number } {
  const pour = tableCellOps(content, storyId, tableId);
  const pourOps = pour.op === "batch" ? pour.args.ops : [pour];
  const decor = tableDecorOps(content, storyId, tableId);
  return {
    batch: { op: "batch", args: { ops: [...pourOps, ...decor.ops] } },
    unmappedRules: decor.unmappedRules,
  };
}

/** The two phases of native-table emission for ONE paginated page's frame
 *  (Wave 2D / S-05). A paginated `Page` carries the same `LoweredContent`
 *  shape a single-frame lowering does, so each page becomes a native
 *  `<Table>` in its own frame's story — the chain lower drives `insert`
 *  first (its outcome mints the tableId), then pours `cells`. `columnWidths`
 *  are the caller's font-metric measurements for THIS page (S-13). */
export interface PageTableOps {
  /** Phase 2 — create the table in the page's frame story. The host
   *  outcome's `createdId` is the new tableId. */
  insert: Mutation;
  /** Build phase 3 once the tableId is resolved — pour each non-empty cell
   *  plus the page's decor (spans, fills, edge strokes), one batch. */
  cells(tableId: string): Mutation;
}

/** Translate ONE paginated `Page` into its frame's native-table mutations
 *  (Wave 2D / S-05). PURE: data in, the two-phase op pair out. Reuses
 *  `tableInsertOp`/`tableContentBatch` over the page's own `content` — each page
 *  is a table in its frame's story (the chain lower resolves `storyId` per
 *  frame, then applies `insert`, then `cells(tableId)`). No host calls, no
 *  spreadsheet semantics — the engine already paginated + formatted. */
export function pageTableMutations(
  page: Page,
  storyId: string,
  columnWidths: number[],
): PageTableOps {
  return {
    insert: tableInsertOp(page.content, storyId, columnWidths),
    cells: (tableId: string) =>
      tableContentBatch(page.content, storyId, tableId).batch,
  };
}
