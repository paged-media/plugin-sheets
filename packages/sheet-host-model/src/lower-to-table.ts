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
//   → cells (insertText per cell, addressed by the new tableId).
//
// SCOPE NOTE (this increment): cell TEXT + column widths land natively.
// Per-cell FILL background + BORDERS — the facets S-03 forced to
// "blocked" — are a follow-on: the wire's `ElementId` has no tableCell
// kind, so `cellFillColor`/`cell*EdgeStroke*` (real PropertyPaths) need a
// cell-addressing door first. Not emitting them is NO regression (the
// tab-text path placed neither); it is the next native-table increment.
//
// ZERO spreadsheet semantics (CLAUDE.md hard rule): every value here is
// already-decided geometry/text from the Rust engine.

import type { Mutation } from "@paged-media/plugin-api";

import type { LoweredContent, Page } from "./lowered";

/** The table's column order (ascending model indices) — the mapping from
 *  a cell's model `col` to its 0-based table column position. */
export function columnOrder(content: LoweredContent): number[] {
  return content.cols.map((c) => c.index);
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
  /** Build phase 3 once the tableId is resolved — pour each non-empty cell. */
  cells(tableId: string): Mutation;
}

/** Translate ONE paginated `Page` into its frame's native-table mutations
 *  (Wave 2D / S-05). PURE: data in, the two-phase op pair out. Reuses
 *  `tableInsertOp`/`tableCellOps` over the page's own `content` — each page
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
    cells: (tableId: string) => tableCellOps(page.content, storyId, tableId),
  };
}
