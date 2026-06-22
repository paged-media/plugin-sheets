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

// GridScene — the TS MIRROR of the Rust grid-scene IR (sheet-grid, spec
// §8.1, S-02). The Rust side serialises camelCase (serde
// rename_all="camelCase"); this file is the host-side CONTRACT for the
// already-windowed viewport the engine hands across the wasm door, PLUS
// the PURE geometry translation the grid panel draws — `gridSceneToSvg`
// and the click/selection helpers. It is rendering-geometry ONLY: zero
// spreadsheet semantics (CLAUDE.md hard rule — parse/eval/format/window
// all happen in the Rust crates; this is the shape the panel consumes and
// the SVG it paints over the active sheet).
//
// Coordinates: the scene's `xOffsets`/`yOffsets` are VIEWPORT-LOCAL
// content-space points (the first visible track starts at 0), carrying
// `cols + 1` / `rows + 1` cumulative boundaries (leading edge of every
// track plus a trailing edge). A cell at `(row, col)` draws its text at
// `xOffsets[col - firstCol] + pad` / `yOffsets[row - firstRow] + baseline`.
// The translation is the same geometry the page lowering uses for its
// rules — one geometry, two surfaces (spec §8.1).
//
// The styles table mirrors `LoweredStyle` (re-exported from ./lowered):
// the grid and the page lowering share ONE style wire shape so a cell
// reads identically on both surfaces (spec §8.3). `styleKey` on a cell
// indexes into `styles`.

import type {
  SceneItem,
  SceneLayer,
  ScenePaint,
  ScenePathSeg,
} from "@paged-media/plugin-api";

import type { Align, LoweredStyle, Rule, Rules } from "./lowered";

/** The viewport window: the first visible `(row, col)`, how many of each
 *  track fit, and the cumulative viewport-local pt boundaries along each
 *  axis. `xOffsets` carries `cols + 1` entries (incl. the trailing edge);
 *  `yOffsets` carries `rows + 1`. */
export interface GridViewport {
  firstRow: number;
  firstCol: number;
  rows: number;
  cols: number;
  /** Cumulative pt boundaries along x (len === `cols + 1`). */
  xOffsets: number[];
  /** Cumulative pt boundaries along y (len === `rows + 1`). */
  yOffsets: number[];
}

/** One materialized visible cell: its ABSOLUTE `(row, col)`, the FORMATTED
 *  text (the number-format output IS the text — spec §8.3), the resolved
 *  alignment, and its style key (indexes `GridScene.styles`). Only
 *  POPULATED cells inside the viewport are present; blanks are drawn from
 *  the viewport geometry. `styleKey` is additive (optional here so
 *  hand-written fixtures stay valid; the engine always emits it — `0` in
 *  Phase A). */
export interface GridCell {
  row: number;
  col: number;
  text: string;
  align: Align;
  styleKey?: number;
}

/** A selection rectangle anchored at `(anchorRow, anchorCol)`, spanning
 *  `rows`×`cols`. `null`/absent when nothing is selected. */
export interface GridSelection {
  anchorRow: number;
  anchorCol: number;
  rows: number;
  cols: number;
}

/** The frozen-pane split rendered in a viewport (spec §8.1). `rows`/`cols`
 *  are the leading SHEET rows/columns held fixed; `frozenWidthPt`/
 *  `frozenHeightPt` are the pt extents of those frozen bands (the split line
 *  sits there). `null`/absent when no pane is frozen. Mirror of the Rust
 *  `sheet_grid::GridFreeze`. */
export interface GridFreeze {
  rows: number;
  cols: number;
  frozenWidthPt: number;
  frozenHeightPt: number;
}

/** One cell-comment indicator on a grid cell (preserve-first display; spec
 *  §10.2). Its absolute `(row, col)` + the cell's top-right corner (`x`, `y`,
 *  viewport-local pt) where the panel draws the comment-triangle marker. The
 *  comment TEXT rides in `engine.listComments()`, not here. Mirror of the Rust
 *  `sheet_grid::GridCommentMarker`. */
export interface GridCommentMarker {
  row: number;
  col: number;
  x: number;
  y: number;
}

/** One conditional-formatting DATA BAR drawn in a grid viewport (spec
 *  §8.2/§10.4 — the page-draw geometry lane, mirrored on the grid). Its
 *  absolute `(row, col)` + the viewport-local rect (`x`, `y`, `w`, `h`, pt): the
 *  bar fills `fillFraction ∈ [0, 1]` of the cell width, inset from the cell
 *  edges. `fill` is the bar colour as `#RRGGBB`. GEOMETRY the engine already
 *  computed — the panel just draws a filled rect (the SAME bar the page lowering
 *  emits, so it reads identically on both surfaces). Mirror of the Rust
 *  `sheet_grid::GridDataBar`. */
export interface GridDataBar {
  row: number;
  col: number;
  x: number;
  y: number;
  w: number;
  h: number;
  fillFraction: number;
  fill: string;
}

/** The complete grid scene for one viewport (spec §8.1): the windowed
 *  geometry, the visible populated cells, the style table, the gridlines
 *  (viewport-local content-space rules, h/v), the optional selection
 *  rectangle, the optional frozen-pane split, the cell-comment markers, and
 *  the conditional-formatting data bars. The panel turns this into an SVG it
 *  overlays the sheet frame with; it never computes any of it. */
export interface GridScene {
  viewport: GridViewport;
  cells: GridCell[];
  styles: LoweredStyle[];
  gridlines: Rules;
  selection?: GridSelection | null;
  /** The frozen-pane split (spec §8.1); `null`/absent when nothing is
   *  frozen. The panel draws a heavier split rule at the band edge. */
  freeze?: GridFreeze | null;
  /** Cell-comment indicators for the visible commented cells (spec §10.2,
   *  preserve-first display). Absent/empty when none. */
  comments?: GridCommentMarker[];
  /** Conditional-formatting data bars for the visible numeric cells a
   *  `dataBar` rule covers (spec §8.2/§10.4 — drawn geometry, not a fill).
   *  Absent/empty when none. */
  databars?: GridDataBar[];
}

/** Tunable geometry for `gridSceneToSvg` — paint-time constants the panel
 *  picks (token-resolved colours, text padding/baseline). All defaults
 *  are sober publishing values; the panel overrides colours from the
 *  token layer. */
export interface GridSvgOptions {
  /** Left inset of cell text from its column's leading edge (pt). */
  pad: number;
  /** Baseline offset of cell text from its row's top edge (pt). */
  baseline: number;
  /** Cell text font-size (pt). */
  fontSizePt: number;
  /** Cell text font-family (CSS value; the panel passes `--font-*`). */
  fontFamily: string;
  /** Default text colour when a style carries none. */
  textColor: string;
  /** Gridline stroke colour. */
  gridColor: string;
  /** Gridline stroke width (pt). */
  gridWidth: number;
  /** Selection rectangle stroke colour. */
  selectionColor: string;
  /** Selection rectangle fill (a translucent wash). */
  selectionFill: string;
  /** Selection stroke width (pt). */
  selectionWidth: number;
  /** Frozen-pane split-line colour (spec §8.1). */
  freezeColor: string;
  /** Frozen-pane split-line width (pt) — heavier than a gridline. */
  freezeWidth: number;
  /** Cell-comment indicator (corner triangle) colour (spec §10.2). */
  commentColor: string;
  /** Cell-comment indicator triangle leg length (pt). */
  commentSize: number;
  /** Conditional-formatting data-bar opacity wash (0–1) — the bar is drawn
   *  behind the cell value, so a light alpha keeps the text legible (spec
   *  §8.2/§10.4). The bar's hue comes from the rule (`GridDataBar.fill`). */
  dataBarOpacity: number;
}

/** Sober defaults; the panel overrides the colour fields from the token
 *  layer (`--pg-*`) so the grid reads native in both themes. */
export const DEFAULT_GRID_SVG_OPTIONS: GridSvgOptions = {
  pad: 3,
  baseline: 11,
  fontSizePt: 10,
  fontFamily: "sans-serif",
  textColor: "#1a1a1a",
  gridColor: "#d0d0d0",
  gridWidth: 0.5,
  selectionColor: "#2b6cb0",
  selectionFill: "rgba(43,108,176,0.12)",
  selectionWidth: 1.5,
  freezeColor: "#808080",
  freezeWidth: 1.25,
  commentColor: "#d93636",
  commentSize: 4,
  dataBarOpacity: 0.85,
};

/** Total viewport width (pt) — the trailing x boundary (0 when empty). */
export function viewportWidthPt(scene: GridScene): number {
  const xs = scene.viewport.xOffsets;
  return xs.length > 0 ? xs[xs.length - 1] : 0;
}

/** Total viewport height (pt) — the trailing y boundary (0 when empty). */
export function viewportHeightPt(scene: GridScene): number {
  const ys = scene.viewport.yOffsets;
  return ys.length > 0 ? ys[ys.length - 1] : 0;
}

/** The text-anchor SVG attribute value for an alignment. `"general"` is
 *  resolved upstream in Rust (the engine emits a concrete left/right/
 *  center for general cells), so it is treated as left here — a defensive
 *  default the panel never actually hits. */
function anchorFor(align: Align): "start" | "middle" | "end" {
  switch (align) {
    case "right":
      return "end";
    case "center":
      return "middle";
    default:
      return "start";
  }
}

/** The x text position for a cell, honouring its alignment within its
 *  column band (`[x0, x1]` viewport-local pt). */
function textX(align: Align, x0: number, x1: number, pad: number): number {
  switch (align) {
    case "right":
      return x1 - pad;
    case "center":
      return (x0 + x1) / 2;
    default:
      return x0 + pad;
  }
}

/** Escape the five XML-significant characters for safe embedding in SVG
 *  text / attribute values. The cell text is ALREADY the formatted value
 *  (Rust); this only makes it XML-safe. */
function xmlEscape(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&apos;");
}

/** Look up a style by key in the scene's table (the default key-0 style
 *  when absent — matching the engine's IR-v2 default). */
function styleOf(scene: GridScene, key: number | undefined): LoweredStyle | undefined {
  if (key === undefined) return scene.styles.find((s) => s.key === 0);
  return scene.styles.find((s) => s.key === key) ?? scene.styles.find((s) => s.key === 0);
}

/** Build the `<rect>` fills for every cell carrying a `fillRgb` style.
 *  Drawn FIRST (under text + gridlines). Viewport-local pt geometry. */
function fillRects(scene: GridScene, vp: GridViewport): string {
  const parts: string[] = [];
  for (const cell of scene.cells) {
    const ci = cell.col - vp.firstCol;
    const ri = cell.row - vp.firstRow;
    if (ci < 0 || ci >= vp.cols || ri < 0 || ri >= vp.rows) continue;
    const style = styleOf(scene, cell.styleKey);
    const fill = style?.fillRgb;
    if (!fill) continue;
    const x0 = vp.xOffsets[ci];
    const x1 = vp.xOffsets[ci + 1];
    const y0 = vp.yOffsets[ri];
    const y1 = vp.yOffsets[ri + 1];
    parts.push(
      `<rect x="${x0}" y="${y0}" width="${x1 - x0}" height="${y1 - y0}" ` +
        `fill="${xmlEscape(fill)}"/>`,
    );
  }
  return parts.join("");
}

/** Build the gridline `<line>` elements from the scene's rule sets
 *  (viewport-local pt). H-rules run along x at a given y (`at`); v-rules
 *  run along y at a given x. */
function gridLines(rules: Rules, o: GridSvgOptions): string {
  const line = (x1: number, y1: number, x2: number, y2: number): string =>
    `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" ` +
    `stroke="${o.gridColor}" stroke-width="${o.gridWidth}"/>`;
  const parts: string[] = [];
  for (const r of rules.h) parts.push(line(r.from, r.at, r.to, r.at));
  for (const r of rules.v) parts.push(line(r.at, r.from, r.at, r.to));
  return parts.join("");
}

/** Build the per-cell border `<line>` elements from style flags
 *  (top/right/bottom/left). Drawn over the gridlines so an explicit
 *  border wins visually. */
function cellBorders(scene: GridScene, vp: GridViewport, o: GridSvgOptions): string {
  const parts: string[] = [];
  const seg = (x1: number, y1: number, x2: number, y2: number): string =>
    `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" ` +
    `stroke="${o.textColor}" stroke-width="${o.gridWidth * 2}"/>`;
  for (const cell of scene.cells) {
    const ci = cell.col - vp.firstCol;
    const ri = cell.row - vp.firstRow;
    if (ci < 0 || ci >= vp.cols || ri < 0 || ri >= vp.rows) continue;
    const style = styleOf(scene, cell.styleKey);
    if (!style) continue;
    const x0 = vp.xOffsets[ci];
    const x1 = vp.xOffsets[ci + 1];
    const y0 = vp.yOffsets[ri];
    const y1 = vp.yOffsets[ri + 1];
    if (style.borderTop) parts.push(seg(x0, y0, x1, y0));
    if (style.borderBottom) parts.push(seg(x0, y1, x1, y1));
    if (style.borderLeft) parts.push(seg(x0, y0, x0, y1));
    if (style.borderRight) parts.push(seg(x1, y0, x1, y1));
  }
  return parts.join("");
}

/** Build the cell `<text>` elements (already-formatted text, alignment,
 *  per-style colour/weight/italic/size). Viewport-local pt geometry. */
function cellTexts(scene: GridScene, vp: GridViewport, o: GridSvgOptions): string {
  const parts: string[] = [];
  for (const cell of scene.cells) {
    const ci = cell.col - vp.firstCol;
    const ri = cell.row - vp.firstRow;
    if (ci < 0 || ci >= vp.cols || ri < 0 || ri >= vp.rows) continue;
    if (cell.text.length === 0) continue;
    const style = styleOf(scene, cell.styleKey);
    const x0 = vp.xOffsets[ci];
    const x1 = vp.xOffsets[ci + 1];
    const y0 = vp.yOffsets[ri];
    const x = textX(cell.align, x0, x1, o.pad);
    const y = y0 + o.baseline;
    const size = style?.fontSizePt ?? o.fontSizePt;
    const family = style?.fontName ?? o.fontFamily;
    const color = style?.textRgb ?? o.textColor;
    const weight = style?.bold ? ' font-weight="700"' : "";
    const italic = style?.italic ? ' font-style="italic"' : "";
    parts.push(
      `<text x="${x}" y="${y}" font-size="${size}" ` +
        `font-family="${xmlEscape(family)}" fill="${xmlEscape(color)}" ` +
        `text-anchor="${anchorFor(cell.align)}"${weight}${italic}>` +
        `${xmlEscape(cell.text)}</text>`,
    );
  }
  return parts.join("");
}

/** The viewport-local pt rectangle of the selection (clamped to the
 *  visible window), or `null` when there is no selection or it lies
 *  entirely outside the window. `[x, y, width, height]`. */
export function selectionRect(scene: GridScene): [number, number, number, number] | null {
  const sel = scene.selection;
  if (!sel) return null;
  const vp = scene.viewport;
  const c0 = sel.anchorCol - vp.firstCol;
  const r0 = sel.anchorRow - vp.firstRow;
  const c1 = c0 + sel.cols; // exclusive band index
  const r1 = r0 + sel.rows;
  // Clamp the band to the visible tracks; bail if it does not intersect.
  const lo = Math.max(0, c0);
  const hi = Math.min(vp.cols, c1);
  const top = Math.max(0, r0);
  const bot = Math.min(vp.rows, r1);
  if (lo >= hi || top >= bot) return null;
  const x = vp.xOffsets[lo];
  const y = vp.yOffsets[top];
  const w = vp.xOffsets[hi] - x;
  const h = vp.yOffsets[bot] - y;
  return [x, y, w, h];
}

/** Build the frozen-pane split `<line>`s (spec §8.1): a heavier rule at the
 *  bottom edge of the frozen row band and the right edge of the frozen column
 *  band. v1 draws the split only when the viewport shows the sheet origin
 *  (`firstRow`/`firstCol` 0) — i.e. the frozen band is on screen; a scrolled-
 *  past composite (the frozen band pinned over a scrolled body) is a panel
 *  follow-on. "" when no pane is frozen or the band is off-screen. */
function freezeSplitSvg(scene: GridScene, o: GridSvgOptions): string {
  const fz = scene.freeze;
  if (!fz) return "";
  const vp = scene.viewport;
  const w = viewportWidthPt(scene);
  const h = viewportHeightPt(scene);
  const line = (x1: number, y1: number, x2: number, y2: number): string =>
    `<line x1="${x1}" y1="${y1}" x2="${x2}" y2="${y2}" ` +
    `stroke="${o.freezeColor}" stroke-width="${o.freezeWidth}"/>`;
  const parts: string[] = [];
  // Horizontal split at the frozen row band's bottom edge (only when the
  // origin row is on screen and the band fits in the viewport).
  if (fz.rows > 0 && vp.firstRow === 0 && fz.frozenHeightPt <= h) {
    parts.push(line(0, fz.frozenHeightPt, w, fz.frozenHeightPt));
  }
  // Vertical split at the frozen column band's right edge.
  if (fz.cols > 0 && vp.firstCol === 0 && fz.frozenWidthPt <= w) {
    parts.push(line(fz.frozenWidthPt, 0, fz.frozenWidthPt, h));
  }
  return parts.join("");
}

/** Build the cell-comment indicator `<polygon>`s (spec §10.2, preserve-first):
 *  a small filled triangle in each commented cell's top-right corner — the
 *  classic comment-note marker. The engine already gave each marker's top-right
 *  corner `(x, y)` (viewport-local pt); the triangle hangs down-and-left from
 *  it. "" when no cell carries a comment. */
function commentMarkersSvg(scene: GridScene, o: GridSvgOptions): string {
  const markers = scene.comments ?? [];
  if (markers.length === 0) return "";
  const s = o.commentSize;
  return markers
    .map((m) => {
      // Triangle: corner, corner-left, corner-down (a right triangle hugging
      // the cell's top-right corner).
      const pts = `${m.x - s},${m.y} ${m.x},${m.y} ${m.x},${m.y + s}`;
      return `<polygon points="${pts}" fill="${o.commentColor}"/>`;
    })
    .join("");
}

/** Build the conditional-formatting data-bar `<rect>`s (spec §8.2/§10.4): one
 *  filled rect per visible data bar, in viewport-local pt with the engine-given
 *  geometry (already inset + proportional). Drawn UNDER the cell text (so the
 *  value reads over the bar, like Excel) with a light opacity wash. "" when the
 *  scene carries no data bars. Each bar's `(row, col)` is absolute; only those
 *  inside the window draw (the engine already windowed them, but we re-check
 *  defensively against a hand-built scene). PURE geometry — zero spreadsheet
 *  semantics (the engine computed the proportion). */
function dataBarsSvg(scene: GridScene, o: GridSvgOptions): string {
  const bars = scene.databars ?? [];
  if (bars.length === 0) return "";
  const vp = scene.viewport;
  const parts: string[] = [];
  for (const b of bars) {
    const ci = b.col - vp.firstCol;
    const ri = b.row - vp.firstRow;
    if (ci < 0 || ci >= vp.cols || ri < 0 || ri >= vp.rows) continue;
    if (b.w <= 0 || b.h <= 0) continue; // a zero-fraction bar draws nothing
    parts.push(
      `<rect x="${b.x}" y="${b.y}" width="${b.w}" height="${b.h}" ` +
        `fill="${xmlEscape(b.fill)}" fill-opacity="${o.dataBarOpacity}"/>`,
    );
  }
  return parts.join("");
}

/** Build the selection `<rect>` (fill wash + stroke), or "" when there is
 *  no visible selection. */
function selectionSvg(scene: GridScene, o: GridSvgOptions): string {
  const rect = selectionRect(scene);
  if (!rect) return "";
  const [x, y, w, h] = rect;
  return (
    `<rect x="${x}" y="${y}" width="${w}" height="${h}" ` +
    `fill="${o.selectionFill}" stroke="${o.selectionColor}" ` +
    `stroke-width="${o.selectionWidth}"/>`
  );
}

/**
 * Translate a [`GridScene`] into a self-contained SVG string the panel
 * overlays the active sheet with (spec §8.1). PURE rendering geometry:
 * fills (under), gridlines, cell borders, cell text (already formatted in
 * Rust), and the selection rectangle (over). The SVG `viewBox` is the
 * viewport's total content-space pt extent so the panel can size it to the
 * frame at any zoom (vector-crisp by construction).
 *
 * Layer order (back to front): cell fills → gridlines → cell borders →
 * data bars → cell text → frozen-pane split → comment markers → selection
 * chrome. (Data bars sit behind the value text, like Excel.)
 */
export function gridSceneToSvg(
  scene: GridScene,
  opts: Partial<GridSvgOptions> = {},
): string {
  const o: GridSvgOptions = { ...DEFAULT_GRID_SVG_OPTIONS, ...opts };
  const vp = scene.viewport;
  const w = viewportWidthPt(scene);
  const h = viewportHeightPt(scene);
  const body =
    fillRects(scene, vp) +
    gridLines(scene.gridlines, o) +
    cellBorders(scene, vp, o) +
    dataBarsSvg(scene, o) +
    cellTexts(scene, vp, o) +
    freezeSplitSvg(scene, o) +
    commentMarkersSvg(scene, o) +
    selectionSvg(scene, o);
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${w} ${h}" ` +
    `width="${w}" height="${h}" data-grid-svg>${body}</svg>`
  );
}

// ── in-frame grid (C-1 sceneLayer, S-02) ────────────────────────────────────
// The SAME windowed geometry `gridSceneToSvg` paints in the panel, lowered to
// the engine's `SceneLayer` IR so the grid renders INSIDE a frame on the
// canvas through `host.contribute.sceneLayer()` — one geometry, two surfaces
// (the panel SVG + the in-frame vector layer). Coordinates are viewport-local
// content points (the engine already windowed them); core applies the frame's
// ItemTransform + content-box clip. v1: cell text is positioned at the cell's
// leading edge (left); right/centre alignment in the in-frame grid is a
// follow-on (the print lowering already aligns natively, §8.3). Colours are
// the concrete RGB the engine/style table carry (or the sober defaults); CSS
// `var(--…)` tokens don't parse and fall back to the default.

/** Parse a CSS colour string (`#rgb`, `#rrggbb`, `rgb(...)`, `rgba(...)`) to a
 *  sRGB [`ScenePaint`]. Falls back to opaque black on anything else (e.g. a
 *  `var(--…)` token), so the layer is never silently invisible. */
export function cssColorToScenePaint(css: string): ScenePaint {
  const s = css.trim();
  const hex = /^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(s);
  if (hex) {
    const h = hex[1];
    const full =
      h.length === 3
        ? h
            .split("")
            .map((c) => c + c)
            .join("")
        : h;
    return {
      r: parseInt(full.slice(0, 2), 16) / 255,
      g: parseInt(full.slice(2, 4), 16) / 255,
      b: parseInt(full.slice(4, 6), 16) / 255,
      a: 1,
    };
  }
  const rgb = /^rgba?\(([^)]+)\)$/.exec(s);
  if (rgb) {
    const p = rgb[1].split(",").map((v) => parseFloat(v.trim()));
    if (p.length >= 3 && p.every((v) => Number.isFinite(v))) {
      return {
        r: p[0] / 255,
        g: p[1] / 255,
        b: p[2] / 255,
        a: p.length >= 4 ? p[3] : 1,
      };
    }
  }
  return { r: 0, g: 0, b: 0, a: 1 };
}

function rectPath(x0: number, y0: number, x1: number, y1: number): ScenePathSeg[] {
  return [
    { op: "moveTo", x: x0, y: y0 },
    { op: "lineTo", x: x1, y: y0 },
    { op: "lineTo", x: x1, y: y1 },
    { op: "lineTo", x: x0, y: y1 },
    { op: "close" },
  ];
}

/** Lower a [`GridScene`] to a [`SceneLayer`] (cell fills + gridlines + cell
 *  text) for in-frame rendering via `host.contribute.sceneLayer()` (C-1 /
 *  S-02). The submission order — fills, then lines, then text — paints text
 *  over fills over lines. */
export function gridSceneToSceneLayer(
  scene: GridScene,
  opts: Partial<GridSvgOptions> = {},
): SceneLayer {
  const o: GridSvgOptions = { ...DEFAULT_GRID_SVG_OPTIONS, ...opts };
  const vp = scene.viewport;
  const inView = (cell: GridCell): boolean => {
    const ci = cell.col - vp.firstCol;
    const ri = cell.row - vp.firstRow;
    return ci >= 0 && ci < vp.cols && ri >= 0 && ri < vp.rows;
  };
  const items: SceneItem[] = [];

  // 1 — cell background fills.
  for (const cell of scene.cells) {
    if (!inView(cell)) continue;
    const fill = styleOf(scene, cell.styleKey)?.fillRgb;
    if (!fill) continue;
    const ci = cell.col - vp.firstCol;
    const ri = cell.row - vp.firstRow;
    items.push({
      kind: "fillPath",
      path: rectPath(
        vp.xOffsets[ci],
        vp.yOffsets[ri],
        vp.xOffsets[ci + 1],
        vp.yOffsets[ri + 1],
      ),
      paint: cssColorToScenePaint(fill),
    });
  }

  // 2 — gridlines (h run along x at `at`; v run along y at `at`).
  const gridPaint = cssColorToScenePaint(o.gridColor);
  for (const r of scene.gridlines.h) {
    items.push({
      kind: "strokePath",
      path: [
        { op: "moveTo", x: r.from, y: r.at },
        { op: "lineTo", x: r.to, y: r.at },
      ],
      paint: gridPaint,
      width: o.gridWidth,
    });
  }
  for (const r of scene.gridlines.v) {
    items.push({
      kind: "strokePath",
      path: [
        { op: "moveTo", x: r.at, y: r.from },
        { op: "lineTo", x: r.at, y: r.to },
      ],
      paint: gridPaint,
      width: o.gridWidth,
    });
  }

  // 2.5 — conditional-formatting data bars (spec §8.2/§10.4): one filled rect
  //       per visible bar, drawn over fills + gridlines but UNDER the cell text
  //       (so the value reads over the bar). The geometry is engine-computed
  //       (inset + proportional); a light opacity keeps text legible.
  for (const b of scene.databars ?? []) {
    const ci = b.col - vp.firstCol;
    const ri = b.row - vp.firstRow;
    if (ci < 0 || ci >= vp.cols || ri < 0 || ri >= vp.rows) continue;
    if (b.w <= 0 || b.h <= 0) continue;
    const paint = cssColorToScenePaint(b.fill);
    items.push({
      kind: "fillPath",
      path: rectPath(b.x, b.y, b.x + b.w, b.y + b.h),
      paint: { ...paint, a: paint.a * o.dataBarOpacity },
    });
  }

  // 3 — cell text (v1: leading-edge / left).
  for (const cell of scene.cells) {
    if (!inView(cell) || cell.text.length === 0) continue;
    const style = styleOf(scene, cell.styleKey);
    const ci = cell.col - vp.firstCol;
    const ri = cell.row - vp.firstRow;
    items.push({
      kind: "text",
      x: vp.xOffsets[ci] + o.pad,
      y: vp.yOffsets[ri] + o.baseline,
      text: cell.text,
      size: style?.fontSizePt ?? o.fontSizePt,
      paint: cssColorToScenePaint(style?.textRgb ?? o.textColor),
    });
  }

  // 4 — selection rectangle (K-1: click-to-select reads in-frame). A
  //     translucent wash + stroke, drawn LAST so it reads over the cell
  //     content. Same geometry the SVG `selectionSvg` draws; the in-frame
  //     scene-layer path expresses it as a filled + stroked rect.
  const sel = selectionRect(scene);
  if (sel) {
    const [sx, sy, sw, sh] = sel;
    const seg = rectPath(sx, sy, sx + sw, sy + sh);
    items.push({
      kind: "fillPath",
      path: seg,
      paint: cssColorToScenePaint(o.selectionFill),
    });
    items.push({
      kind: "strokePath",
      path: seg,
      paint: cssColorToScenePaint(o.selectionColor),
      width: o.selectionWidth,
    });
  }

  return { items };
}

/**
 * Hit-test a viewport-local pt point `(x, y)` to the ABSOLUTE `(row, col)`
 * it falls in, using the scene's cumulative offsets. Returns `null` when
 * the point lies outside the windowed tracks (left/above the first edge or
 * past the trailing edge). PURE geometry — the inverse of how a cell is
 * placed; the panel's click-to-select reads it.
 */
export function hitCell(
  scene: GridScene,
  x: number,
  y: number,
): { row: number; col: number } | null {
  const vp = scene.viewport;
  const ci = trackIndex(vp.xOffsets, x);
  const ri = trackIndex(vp.yOffsets, y);
  if (ci === null || ri === null) return null;
  return { row: vp.firstRow + ri, col: vp.firstCol + ci };
}

/** Which track band a coordinate falls in, given the cumulative
 *  boundaries (`len = tracks + 1`). `null` when outside `[0, last)`. */
function trackIndex(offsets: number[], v: number): number | null {
  const n = offsets.length - 1; // track count
  if (n <= 0) return null;
  if (v < offsets[0] || v >= offsets[n]) return null;
  // Linear scan is fine: a viewport holds O(visible) tracks, not O(sheet).
  for (let i = 0; i < n; i += 1) {
    if (v >= offsets[i] && v < offsets[i + 1]) return i;
  }
  return null;
}

/**
 * The viewport-local pt rectangle to overlay a CELL EDITOR input at for the
 * absolute `(row, col)`, or `null` when the cell is outside the window.
 * `[x, y, width, height]` — the panel positions its `<input>` here on
 * double-click / typing. PURE geometry from the cumulative offsets.
 */
export function cellEditorRect(
  scene: GridScene,
  row: number,
  col: number,
): [number, number, number, number] | null {
  const vp = scene.viewport;
  const ci = col - vp.firstCol;
  const ri = row - vp.firstRow;
  if (ci < 0 || ci >= vp.cols || ri < 0 || ri >= vp.rows) return null;
  const x = vp.xOffsets[ci];
  const y = vp.yOffsets[ri];
  return [x, y, vp.xOffsets[ci + 1] - x, vp.yOffsets[ri + 1] - y];
}

export type { Align, LoweredStyle, Rule, Rules };
