// Grid navigation — the selection model and the key map the grid panel
// and the in-frame grid share (Wave 5). Pure: no host, no engine. This is
// cursor geometry, not spreadsheet semantics (what a fill or paste WRITES
// is the engine's — sheet-js fill_range / shift_formulas).
//
// The model: an ANCHOR (where the selection began — the fixed corner), a
// FOCUS (the corner that moves on shift-click / shift-arrow / drag) and an
// ACTIVE cell (where typing lands; the anchor, except while Tab/Enter
// walk inside a multi-cell selection).

import type { GridScene, GridSelection } from "../../sheet-host-model/src";

export interface Cell {
  row: number;
  col: number;
}

export interface SelectionModel {
  anchor: Cell;
  focus: Cell;
  active: Cell;
}

const clampCell = (c: Cell): Cell => ({
  row: Math.max(0, c.row),
  col: Math.max(0, c.col),
});

/** A one-cell selection. */
export function collapsed(cell: Cell): SelectionModel {
  const c = clampCell(cell);
  return { anchor: c, focus: c, active: c };
}

/** The normalized rectangle a model selects. */
export function rectOf(m: SelectionModel): GridSelection {
  const top = Math.min(m.anchor.row, m.focus.row);
  const left = Math.min(m.anchor.col, m.focus.col);
  return {
    anchorRow: top,
    anchorCol: left,
    rows: Math.abs(m.focus.row - m.anchor.row) + 1,
    cols: Math.abs(m.focus.col - m.anchor.col) + 1,
  };
}

/** The model for a rectangle set from outside (a panel / API call): the
 *  top-left is the anchor and the active cell. */
export function modelOfRect(r: GridSelection): SelectionModel {
  const anchor = { row: r.anchorRow, col: r.anchorCol };
  return {
    anchor,
    focus: {
      row: r.anchorRow + Math.max(1, r.rows) - 1,
      col: r.anchorCol + Math.max(1, r.cols) - 1,
    },
    active: anchor,
  };
}

/** Shift-click / drag: the focus moves to `cell`, the anchor stays. */
export function extendTo(m: SelectionModel, cell: Cell): SelectionModel {
  return { anchor: m.anchor, focus: clampCell(cell), active: m.anchor };
}

/** An arrow key: `extend` moves the focus (shift), otherwise the active
 *  cell moves and the selection collapses onto it. */
export function moveBy(
  m: SelectionModel,
  dRow: number,
  dCol: number,
  extend: boolean,
): SelectionModel {
  if (extend) {
    return extendTo(m, { row: m.focus.row + dRow, col: m.focus.col + dCol });
  }
  return collapsed({ row: m.active.row + dRow, col: m.active.col + dCol });
}

export type AdvanceDir = "right" | "left" | "down" | "up";

/** Tab / Shift-Tab / Enter / Shift-Enter: one cell on — inside a
 *  multi-cell selection the active cell walks the selection and wraps
 *  (rows for Tab, columns for Enter), the selection stays. */
export function advance(m: SelectionModel, dir: AdvanceDir): SelectionModel {
  const r = rectOf(m);
  const step = dir === "right" || dir === "down" ? 1 : -1;
  if (r.rows === 1 && r.cols === 1) {
    const horizontal = dir === "right" || dir === "left";
    return collapsed({
      row: m.active.row + (horizontal ? 0 : step),
      col: m.active.col + (horizontal ? step : 0),
    });
  }
  // Walk inside the rectangle: index the cells in the walk order.
  const rowMajor = dir === "right" || dir === "left";
  const n = r.rows * r.cols;
  const i0 = rowMajor
    ? (m.active.row - r.anchorRow) * r.cols + (m.active.col - r.anchorCol)
    : (m.active.col - r.anchorCol) * r.rows + (m.active.row - r.anchorRow);
  const i = (((i0 + step) % n) + n) % n;
  const active = rowMajor
    ? { row: r.anchorRow + Math.floor(i / r.cols), col: r.anchorCol + (i % r.cols) }
    : { row: r.anchorRow + (i % r.rows), col: r.anchorCol + Math.floor(i / r.rows) };
  return { anchor: m.anchor, focus: m.focus, active };
}

/** The target rectangle of a fill-handle drag from `source` to the cell
 *  under the pointer: the source extended along the axis the pointer left
 *  it by the most (Excel), or null while the pointer is inside it. */
export function fillTarget(source: GridSelection, cell: Cell): GridSelection | null {
  const top = source.anchorRow;
  const left = source.anchorCol;
  const bottom = top + source.rows - 1;
  const right = left + source.cols - 1;
  const below = cell.row - bottom;
  const above = top - cell.row;
  const after = cell.col - right;
  const before = left - cell.col;
  const vertical = Math.max(below, above);
  const horizontal = Math.max(after, before);
  if (vertical <= 0 && horizontal <= 0) return null;
  if (vertical >= horizontal) {
    return below > 0
      ? { anchorRow: top, anchorCol: left, rows: source.rows + below, cols: source.cols }
      : { anchorRow: cell.row, anchorCol: left, rows: source.rows + above, cols: source.cols };
  }
  return after > 0
    ? { anchorRow: top, anchorCol: left, rows: source.rows, cols: source.cols + after }
    : { anchorRow: top, anchorCol: cell.col, rows: source.rows, cols: source.cols + before };
}

/** How many leading rows / columns of a window are FULLY visible within
 *  `wPt × hPt` (at least one). */
export function fullyVisible(
  scene: GridScene,
  wPt: number,
  hPt: number,
): { rows: number; cols: number } {
  const vp = scene.viewport;
  let rows = 0;
  while (rows < vp.rows && vp.yOffsets[rows + 1] <= hPt + 1e-6) rows += 1;
  let cols = 0;
  while (cols < vp.cols && vp.xOffsets[cols + 1] <= wPt + 1e-6) cols += 1;
  return { rows: Math.max(1, rows), cols: Math.max(1, cols) };
}

/** The window origin that shows `cell`, moving as little as possible. */
export function scrollToShow(
  origin: { firstRow: number; firstCol: number },
  visible: { rows: number; cols: number },
  cell: Cell,
): { firstRow: number; firstCol: number } {
  let { firstRow, firstCol } = origin;
  if (cell.row < firstRow) firstRow = cell.row;
  else if (cell.row >= firstRow + visible.rows) firstRow = cell.row - visible.rows + 1;
  if (cell.col < firstCol) firstCol = cell.col;
  else if (cell.col >= firstCol + visible.cols) firstCol = cell.col - visible.cols + 1;
  return { firstRow: Math.max(0, firstRow), firstCol: Math.max(0, firstCol) };
}

// ───────────────────────────────────────────────────────────── the key map

/** The part of a KeyboardEvent the map reads (a DOM event, a React one,
 *  or a test literal). */
export interface KeyLike {
  key: string;
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
}

/** What a key does on the grid. */
export type GridKeyAction =
  | { kind: "move"; dRow: number; dCol: number; extend: boolean }
  | { kind: "advance"; dir: AdvanceDir }
  | { kind: "page"; dir: 1 | -1; extend: boolean }
  | { kind: "home"; origin: boolean }
  | { kind: "edit" }
  | { kind: "type"; ch: string }
  | { kind: "backspace" }
  | { kind: "commit"; then: AdvanceDir | null }
  | { kind: "commitMove"; dRow: number; dCol: number }
  | { kind: "cancel" }
  | { kind: "clear" }
  | { kind: "copy" }
  | { kind: "paste" }
  | { kind: "fill"; dir: "down" | "right" }
  | { kind: "selectAll" }
  | { kind: "find" }
  | { kind: "undo" }
  | { kind: "redo" }
  | { kind: "none" };

const ARROWS: Record<string, [number, number]> = {
  ArrowUp: [-1, 0],
  ArrowDown: [1, 0],
  ArrowLeft: [0, -1],
  ArrowRight: [0, 1],
};

/** Map a key to a grid action. `editing` is the open cell edit: `null`
 *  (navigating), `"enter"` (typing replaced the cell — arrows commit and
 *  move, as in Excel) or `"edit"` (F2 — arrows are the editor's, and with
 *  no caret here they do nothing). */
export function gridKeyAction(
  e: KeyLike,
  editing: null | "enter" | "edit",
): GridKeyAction {
  const mod = !!(e.metaKey || e.ctrlKey);
  const shift = !!e.shiftKey;
  const key = e.key;
  if (mod && !e.altKey) {
    switch (key.toLowerCase()) {
      case "z":
        return shift ? { kind: "redo" } : { kind: "undo" };
      case "y":
        return { kind: "redo" };
      case "c":
        return editing ? { kind: "none" } : { kind: "copy" };
      case "v":
        return editing ? { kind: "none" } : { kind: "paste" };
      case "d":
        return editing ? { kind: "none" } : { kind: "fill", dir: "down" };
      case "r":
        return editing ? { kind: "none" } : { kind: "fill", dir: "right" };
      case "a":
        return editing ? { kind: "none" } : { kind: "selectAll" };
      case "f":
        return { kind: "find" };
      case "home":
        return editing ? { kind: "none" } : { kind: "home", origin: true };
      default:
        return { kind: "none" };
    }
  }
  if (editing) {
    if (key === "Enter") return { kind: "commit", then: shift ? "up" : "down" };
    if (key === "Tab") return { kind: "commit", then: shift ? "left" : "right" };
    if (key === "Escape") return { kind: "cancel" };
    if (key === "Backspace") return { kind: "backspace" };
    if (key in ARROWS) {
      if (editing === "edit") return { kind: "none" };
      const [dRow, dCol] = ARROWS[key];
      return { kind: "commitMove", dRow, dCol };
    }
    if (key.length === 1) return { kind: "type", ch: key };
    return { kind: "none" };
  }
  if (key in ARROWS) {
    const [dRow, dCol] = ARROWS[key];
    return { kind: "move", dRow, dCol, extend: shift };
  }
  switch (key) {
    case "Tab":
      return { kind: "advance", dir: shift ? "left" : "right" };
    case "Enter":
      return { kind: "advance", dir: shift ? "up" : "down" };
    case "F2":
      return { kind: "edit" };
    case "Delete":
    case "Backspace":
      return { kind: "clear" };
    case "PageDown":
      return { kind: "page", dir: 1, extend: shift };
    case "PageUp":
      return { kind: "page", dir: -1, extend: shift };
    case "Home":
      return { kind: "home", origin: false };
    default:
      break;
  }
  if (key.length === 1) return { kind: "type", ch: key };
  return { kind: "none" };
}
