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

/** A row's identity for alignment: its cells' text + style keys + height. */
function rowSignature(content: LoweredContent, r: number, colPos: Map<number, number>): string {
  const row = content.rows[r];
  const cells = row.cells
    .filter((c) => c.text.length > 0 || (c.styleKey ?? 0) !== 0)
    .map((c) => `${colPos.get(c.col)}=${c.styleKey ?? 0}:${c.text}`)
    .join("\u0001");
  return `${row.heightPt}|${cells}`;
}

/** Rows beyond which the O(n·m) alignment falls back to tail reshaping. */
const ALIGN_LIMIT = 2000;

/** Match next rows to prev rows (longest common subsequence of row
 *  signatures): `match[j]` is the prev row next row `j` keeps, or -1 when
 *  `j` is a new row. A page split that moved rows between frames is then a
 *  few row inserts/deletes, not a re-pour of every shifted cell. */
function alignRows(prev: LoweredContent, next: LoweredContent): number[] {
  const n = prev.rows.length;
  const m = next.rows.length;
  const match = new Array<number>(m).fill(-1);
  if (n === 0 || m === 0) return match;
  if (n * m > ALIGN_LIMIT * ALIGN_LIMIT / 4) {
    // Too big to align: keep positions (tail reshape).
    for (let j = 0; j < Math.min(n, m); j++) match[j] = j;
    return match;
  }
  const pc = new Map<number, number>();
  columnOrder(prev).forEach((c, i) => pc.set(c, i));
  const nc = new Map<number, number>();
  columnOrder(next).forEach((c, i) => nc.set(c, i));
  const a = prev.rows.map((_, i) => rowSignature(prev, i, pc));
  const b = next.rows.map((_, j) => rowSignature(next, j, nc));
  // lcs[i][j] = LCS length of a[i..], b[j..].
  const lcs: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] =
        a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      match[j] = i;
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      i++;
    } else {
      j++;
    }
  }
  // Rows that did not match but sit at the same place pair up positionally
  // (a changed cell in a kept row re-pours that cell, not the row).
  const used = new Set(match.filter((x) => x >= 0));
  const free: number[] = [];
  for (let k = 0; k < n; k++) if (!used.has(k)) free.push(k);
  let lastPrev = -1;
  for (let jj = 0; jj < m; jj++) {
    if (match[jj] >= 0) {
      lastPrev = match[jj];
      continue;
    }
    const nextMatched = match.slice(jj + 1).find((x) => x >= 0) ?? n;
    const cand = free.find((k) => k > lastPrev && k < nextMatched);
    if (cand !== undefined) {
      match[jj] = cand;
      free.splice(free.indexOf(cand), 1);
      lastPrev = cand;
    }
  }
  return match;
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
  const match = alignRows(prev, next);

  // The table as it will stand after the row edits, seen as PREV content:
  // kept rows carry their old cells, new rows are empty. Text and decor diff
  // against this, so only what really changed is re-poured.
  const kept = new Set(match.filter((x) => x >= 0));
  const virtualPrev: LoweredContent = {
    ...prev,
    rows: next.rows.map((row, j) =>
      match[j] >= 0
        ? { ...prev.rows[match[j]], index: row.index }
        : { index: row.index, heightPt: row.heightPt, cells: [] },
    ),
    merges: [],
    rules: { h: [], v: [] },
  };

  // Spans that go away are reset first, while their anchors still exist.
  const prevDecor = tableDecorOps(prev, storyId, tableId).ops;
  const nextDecor = tableDecorOps(next, storyId, tableId).ops;
  const nextKeys = new Set(nextDecor.map(decorKey));
  for (const op of prevDecor) {
    if (op.op !== "setCellSpan" || nextKeys.has(decorKey(op))) continue;
    if (op.args.row < prevRows && op.args.col < prevCols) {
      structure.push({
        op: "setCellSpan",
        args: { ...op.args, rowSpan: 1, columnSpan: 1 },
      });
    }
  }

  // Rows: delete the prev rows nothing keeps (bottom-up, so indices stay
  // valid), then insert each new row at its final index (top-down: when row
  // j is inserted, rows 0..j-1 already stand).
  for (let i = prevRows - 1; i >= 0; i--) {
    if (!kept.has(i)) {
      structure.push({ op: "deleteTableRow", args: { storyId, tableId, at: i } });
    }
  }
  for (let j = 0; j < nextRows; j++) {
    if (match[j] < 0) {
      structure.push({ op: "insertTableRow", args: { storyId, tableId, at: j } });
    }
  }
  // Columns: append or drop at the END.
  for (let c = prevCols; c < nextCols; c++) {
    structure.push({ op: "insertTableColumn", args: { storyId, tableId, at: c } });
  }
  for (let c = prevCols; c > nextCols; c--) {
    structure.push({
      op: "deleteTableColumn",
      args: { storyId, tableId, at: c - 1 },
    });
  }
  next.rows.forEach((row, j) => {
    const was = match[j] >= 0 ? prev.rows[match[j]].heightPt : null;
    if (was !== row.heightPt) {
      structure.push({
        op: "setRowHeight",
        args: { storyId, tableId, row: j, height: row.heightPt },
      });
    }
  });
  for (let c = 0; c < nextCols; c++) {
    const w = nextWidths[c];
    if (w !== undefined && (c >= prevCols || prevWidths[c] !== w)) {
      structure.push({ op: "setColumnWidth", args: { storyId, tableId, col: c, width: w } });
    }
  }

  // Text: only the cells that changed against the realigned table.
  const before = cellTexts(virtualPrev);
  const after = cellTexts(next);
  for (let r = 0; r < nextRows; r++) {
    for (let c = 0; c < nextCols; c++) {
      const key = `${r}:${c}`;
      const old = c < prevCols ? (before.get(key) ?? "") : "";
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

  // Decor: resets for fills/edges the realigned table had (its styles) and
  // the new content lacks, then the new decor in full.
  const decor: Mutation[] = [];
  const virtualDecor = tableDecorOps(virtualPrev, storyId, tableId).ops;
  const prevRuleDecor = prevDecor.filter(
    (op) => op.op === "setElementProperty" && op.args.path !== "cellFillColor",
  );
  for (const op of [...virtualDecor, ...prevRuleDecor]) {
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
  const resets = decor.filter(
    (op, i) => decor.findIndex((o) => decorKey(o) === decorKey(op)) === i,
  );
  return { structure, text, decor: [...resets, ...nextDecor] };
}

