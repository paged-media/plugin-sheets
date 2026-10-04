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

// Refresh a PLACED native table in place (Wave 4). A placed table projects a
// (sheet, range); when the workbook changes — an edit, a recalc, a
// re-pagination that moves the row split — the table must show the new
// content WITHOUT a second table appearing beside the first. The wire has
// no table-delete op (`deleteTable` is not on the wire; core's RemoveNode
// for a Table backs only undo), so "replace" is done by RESHAPING the
// existing table to the new content and re-pouring only what changed:
//
//   structure — rows/columns appended or removed at the end, row heights
//               and column widths that moved, spans that no longer exist
//               reset to 1×1;
//   text      — per cell whose text changed: delete the old text, insert
//               the new (cell-addressed text ops);
//   decor     — the new spans/fills/edges, plus a RESET for every fill or
//               edge the old content had and the new one lacks.
//
// PURE: (previous content, next content, ids, widths) in, mutations out.
// The bundle drives them (lower.ts).

import type { Mutation } from "@paged-media/plugin-api";

import type { LoweredContent } from "./lowered";
import { columnOrder, tableDecorOps } from "./lower-to-table";

/** The ops a refresh needs, by apply lane (text cannot share a frame-op
 *  batch on older engines, so the lanes stay separable). */
export interface TableRefreshOps {
  structure: Mutation[];
  text: Mutation[];
  decor: Mutation[];
}

/** The IDML "no colour" swatch — what an un-filled cell's fill resets to. */
export const NO_FILL_SWATCH = "Swatch/None";

/** UTF-8 byte length — the unit core's story offsets count in. */
export function textOffsetLength(s: string): number {
  return new TextEncoder().encode(s).length;
}

/** `(tablePosRow, tablePosCol) → text` for the populated cells. */
function cellTexts(content: LoweredContent): Map<string, string> {
  const colPos = new Map<number, number>();
  columnOrder(content).forEach((m, i) => colPos.set(m, i));
  const out = new Map<string, string>();
  content.rows.forEach((row, r) => {
    for (const cell of row.cells) {
      const c = colPos.get(cell.col);
      if (c !== undefined && cell.text.length > 0) out.set(`${r}:${c}`, cell.text);
    }
  });
  return out;
}

/** A stable key for a decor op (the property it sets on which cell). */
function decorKey(op: Mutation): string | null {
  if (op.op === "setElementProperty" && op.args.elementId.kind === "tableCell") {
    const id = op.args.elementId.id;
    return `${id.row}:${id.col}:${op.args.path}`;
  }
  if (op.op === "setCellSpan") return `${op.args.row}:${op.args.col}:span`;
  return null;
}

/** Compute the in-place refresh of a placed table from `prev` to `next`. */
export function tableRefreshOps(
  prev: LoweredContent,
  next: LoweredContent,
  storyId: string,
  tableId: string,
  prevWidths: readonly number[],
  nextWidths: readonly number[],
): TableRefreshOps {
  const structure: Mutation[] = [];
  const text: Mutation[] = [];

  const prevRows = prev.rows.length;
  const nextRows = next.rows.length;
  const prevCols = prev.cols.length;
  const nextCols = next.cols.length;

  // Spans that go away are reset first, while their anchors still exist.
  const prevDecor = tableDecorOps(prev, storyId, tableId).ops;
  const nextDecor = tableDecorOps(next, storyId, tableId).ops;
  const nextKeys = new Set(nextDecor.map(decorKey));
  for (const op of prevDecor) {
    if (op.op !== "setCellSpan" || nextKeys.has(decorKey(op))) continue;
    if (op.args.row < nextRows && op.args.col < nextCols) {
      structure.push({
        op: "setCellSpan",
        args: { ...op.args, rowSpan: 1, columnSpan: 1 },
      });
    }
  }

  // Rows / columns: append or drop at the END (the table's tail is where a
  // range grows or a page split moves).
  for (let r = prevRows; r < nextRows; r++) {
    structure.push({ op: "insertTableRow", args: { storyId, tableId, at: r } });
  }
  for (let r = prevRows; r > nextRows; r--) {
    structure.push({ op: "deleteTableRow", args: { storyId, tableId, at: r - 1 } });
  }
  for (let c = prevCols; c < nextCols; c++) {
    structure.push({ op: "insertTableColumn", args: { storyId, tableId, at: c } });
  }
  for (let c = prevCols; c > nextCols; c--) {
    structure.push({
      op: "deleteTableColumn",
      args: { storyId, tableId, at: c - 1 },
    });
  }
  next.rows.forEach((row, r) => {
    if (r >= prevRows || prev.rows[r].heightPt !== row.heightPt) {
      structure.push({
        op: "setRowHeight",
        args: { storyId, tableId, row: r, height: row.heightPt },
      });
    }
  });
  for (let c = 0; c < nextCols; c++) {
    const w = nextWidths[c];
    if (w !== undefined && (c >= prevCols || prevWidths[c] !== w)) {
      structure.push({ op: "setColumnWidth", args: { storyId, tableId, col: c, width: w } });
    }
  }

  // Text: only the cells that changed. Cells in removed rows/cols went with
  // them; cells in added rows/cols start empty.
  const before = cellTexts(prev);
  const after = cellTexts(next);
  for (let r = 0; r < nextRows; r++) {
    for (let c = 0; c < nextCols; c++) {
      const key = `${r}:${c}`;
      const old = r < prevRows && c < prevCols ? (before.get(key) ?? "") : "";
      const now = after.get(key) ?? "";
      if (old === now) continue;
      const cell = { tableId, row: r, col: c };
      if (old.length > 0) {
        text.push({
          op: "deleteRange",
          args: { storyId, start: 0, end: textOffsetLength(old), cell },
        });
      }
      if (now.length > 0) {
        text.push({ op: "insertText", args: { storyId, offset: 0, text: now, cell } });
      }
    }
  }

  // Decor: resets for what disappeared (inside the new grid), then the new.
  const decor: Mutation[] = [];
  for (const op of prevDecor) {
    if (op.op !== "setElementProperty" || nextKeys.has(decorKey(op))) continue;
    if (op.args.elementId.kind !== "tableCell") continue;
    const { row, col } = op.args.elementId.id;
    if (row >= nextRows || col >= nextCols) continue;
    decor.push({
      op: "setElementProperty",
      args: {
        elementId: op.args.elementId,
        path: op.args.path,
        value:
          op.args.path === "cellFillColor"
            ? { type: "colorRef", value: NO_FILL_SWATCH }
            : { type: "length", value: 0 },
      },
    });
  }
  decor.push(...nextDecor);

  return { structure, text, decor };
}
