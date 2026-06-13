// The tab-text FALLBACK translator (spec §2.2 degradation): the engine
// has ALREADY computed the lowered IR (column/row geometry, formatted
// cell text, grid rules, merges); this turns that pure data into host
// Mutations. ZERO spreadsheet semantics live here (CLAUDE.md hard rule)
// — it is arithmetic over already-decided geometry plus the host
// mutation vocabulary.
//
// LANE STATUS (S-03 RESOLVED): the wire NOW carries native table ops
// (`insertTable`, cell-addressed `insertText`, `setCellSpan`, tableCell-
// scoped properties) and the page lower PREFERS that lane
// (lower-to-table.ts). THIS module is the retained, explicit fallback —
// selected via the bundle's `lane: "tab-text"` option or when a host
// rejects `insertTable` — and stays tested. Its constraint is real:
// `insertText` keys off a `storyId` that exists only AFTER the frame
// applies, so the spec §2.2 fallback — tab-aligned text in a text frame
// + drawn rules — splits across two phases:
//
//   Phase 1 (this function's `batch`): insertTextFrame + an insertLine
//     per grid rule + setPluginMetadata writing the binding onto the
//     batch-created frame (the protocol-v34 `$created` sentinel). ONE
//     undoable step; the host returns the created frame's id.
//   Phase 2 (the caller, lower.ts): resolve the frame's storyId from the
//     created id, then `insertText` the tab/newline-joined `text` this
//     function also returns.
//
// We return BOTH the batch and the text so the caller never re-derives
// the join. Geometry is content-space in the IR (offsets from the
// region's own top-left, spec §8.5); we add the frame's page-local
// bounds origin so rules land on the page.

import type {
  ElementId,
  Mutation,
  PropertyPath,
  SwatchSpec,
  Value,
} from "@paged-media/plugin-api";
import type { PageId } from "@paged-media/plugin-api";

import { BINDING_KEY, type Binding } from "./binding";
import type { Bounds } from "./placement";
import type { DataBarRect, LoweredContent, LoweredStyle } from "./lowered";

/** The page + bounds the frame is placed at (from placement.ts). */
export interface LowerPlacement {
  pageId: PageId;
  bounds: Bounds;
}

/** What the translator yields: the phase-1 mutation batch and the
 *  phase-2 text to pour once the caller resolves the new frame's story. */
export interface LowerResult {
  /** One undoable `batch`: frame + rules + binding metadata. */
  batch: Mutation;
  /** The cells joined tab-within-row, newline-between-rows — the
   *  phase-2 `insertText` payload. */
  text: string;
  /** The non-default styles' prepared host overrides (IR-v2 style-map
   *  track, spec §8.3). Character-level props are ready to apply once a run
   *  span is known (the S-04 doc-style-group path); `blocked` facets need a
   *  real table cell — only the native lane (lower-to-table.ts) has one.
   *  Empty for an unstyled region. */
  styles: StyleEmission[];
}

/** The protocol-v34 batch-created sentinel: an `insertTextFrame` mints a
 *  textFrame, and a later op in the SAME batch addresses it by this
 *  placeholder id. The host resolves `$created` to the just-minted frame
 *  and the metadata gate verifies the key is this plugin's own namespace
 *  (the plugin-web insert.ts precedent). */
const CREATED_FRAME: ElementId = { kind: "textFrame", id: "$created" };

/** Build the tab/newline join of the lowered cells. Rows in IR order;
 *  within a row, cells are placed at their `col` so empty columns become
 *  empty tab fields (tab-aligned text is the fallback degradation — the
 *  columns line up under monospace/tab stops). A trailing newline is
 *  NOT added (rows are SEPARATED by newlines, not terminated). */
export function joinText(content: LoweredContent): string {
  // The column order the text follows: the lowered column indices,
  // ascending. Empty leading/interior columns become empty fields so
  // every row aligns to the same tab grid.
  const colOrder = content.cols.map((c) => c.index);
  const lines = content.rows.map((row) => {
    const byCol = new Map<number, string>();
    for (const cell of row.cells) byCol.set(cell.col, cell.text);
    return colOrder.map((ci) => byCol.get(ci) ?? "").join("\t");
  });
  return lines.join("\n");
}

// ── Style emission (IR v2, M1 style-map track; spec §8.3) ──────────────────
//
// "Document-coherent styling is the most important property of the whole
// plugin" (spec §8.3). The engine has resolved each cell's visual style into
// the `content.styles` table (deduped LoweredStyles, indexed by a cell's
// `styleKey`). This turns the NON-DEFAULT styles into the host's character-
// level property overrides — the same `{ path, value }` shape a
// `setStyleProperty` carries to DEFINE a character style (the doc-style-group
// path), so a caller can apply them as the §8.3 "constrained local override"
// once the run offsets are known.
//
// THE HONEST WIRE BOUNDARY (constitution: "never fake reserved seams"):
//
//  - Character emphasis / size / face / TEXT colour ARE expressible at the
//    character level (`characterFontStyle`/`characterFontSize`/
//    `characterFontFamily`/`characterFillColor`) — emitted here.
//  - FILL background + per-edge BORDERS need a real table CELL to attach to;
//    THIS fallback lane pours tab-aligned text into ONE text frame, so there
//    is no cell to carry `cellFillColor` / `cell*EdgeStroke*`. They are
//    reported as BLOCKED here, never silently dropped or faked onto the
//    frame. (The preferred NATIVE lane, lower-to-table.ts, places both via
//    tableCell-scoped setElementProperty — S-03 resolved.)
//  - APPLYING an override to a poured run needs either a named character
//    style (`createCharacterStyle` + `applyStyle`) or run-offset addressing —
//    both the S-04 doc-style-group path (the style-management capability is an
//    SDK gap). So this function PREPARES the overrides; wiring them through
//    `applyStyle` is future (documented in the registry `doc-style-group`
//    row, status: planned).
//
// Pure: data in, descriptors out (no host calls).

/** One host property override derived from a `LoweredStyle` — the
 *  `{ path, value }` pair a `setStyleProperty`/`applyStyle` flow carries. */
export interface StyleProp {
  path: PropertyPath;
  value: Value;
}

/** A facet of a cell style the tab-text FALLBACK lane cannot place (no
 *  real table cell to attach a fill/border to; the native lane places
 *  both). Reported, never faked. */
export type BlockedFacet = "fillBackground" | "border";

/** The emittable overrides for ONE non-default style key, plus the facets
 *  the current degradation cannot express (so a caller/panel can flag them
 *  as off-style — the publishing affordance of spec §8.3). */
export interface StyleEmission {
  /** Index into `content.styles` (a cell's `styleKey` selects this). */
  styleKey: number;
  /** Character-level overrides expressible TODAY (font style/size/face,
   *  text colour). */
  props: StyleProp[];
  /** Facets requiring a real table cell — blocked by the tab-text
   *  fallback's single text frame (empty when nothing is blocked). */
  blocked: BlockedFacet[];
}

/** Map one `LoweredStyle` to its host character-level overrides. The default
 *  style (no emphasis/size/face/colour) yields an empty `props`. Bold/italic
 *  collapse to the single `characterFontStyle` token Paged uses
 *  ("Bold"/"Italic"/"Bold Italic"/"Regular"). */
export function styleProps(style: LoweredStyle): StyleProp[] {
  const props: StyleProp[] = [];

  // Bold/italic → one `characterFontStyle` face token.
  const face =
    style.bold && style.italic
      ? "Bold Italic"
      : style.bold
        ? "Bold"
        : style.italic
          ? "Italic"
          : null;
  if (face) props.push({ path: "characterFontStyle", value: { type: "text", value: face } });

  if (style.fontName != null)
    props.push({
      path: "characterFontFamily",
      value: { type: "text", value: style.fontName },
    });

  if (style.fontSizePt != null)
    props.push({
      path: "characterFontSize",
      value: { type: "length", value: style.fontSizePt },
    });

  // Cell TEXT colour → character fill (the glyph colour). Fill BACKGROUND is
  // a cell facet, handled separately (and blocked in this fallback lane).
  if (style.textRgb != null)
    props.push({
      path: "characterFillColor",
      value: { type: "colorRef", value: style.textRgb },
    });

  return props;
}

/** Which cell facets a `LoweredStyle` carries that the tab-text fallback
 *  frame cannot place (a fill background and/or borders need a real table
 *  cell — the native lane's job). */
function blockedFacets(style: LoweredStyle): BlockedFacet[] {
  const out: BlockedFacet[] = [];
  if (style.fillRgb != null) out.push("fillBackground");
  if (style.borderTop || style.borderRight || style.borderBottom || style.borderLeft)
    out.push("border");
  return out;
}

/** Turn the lowered styles table into per-key [`StyleEmission`]s, SKIPPING
 *  the default key 0 and any key that emits nothing AND blocks nothing
 *  (a visually-default style). Pure + deterministic (styles-table order). */
export function styleEmissions(content: LoweredContent): StyleEmission[] {
  const styles = content.styles ?? [];
  const out: StyleEmission[] = [];
  for (const style of styles) {
    if (style.key === 0) continue; // the default style is never an override
    const props = styleProps(style);
    const blocked = blockedFacets(style);
    if (props.length === 0 && blocked.length === 0) continue;
    out.push({ styleKey: style.key, props, blocked });
  }
  return out;
}

// ── Conditional-formatting data bars (spec §8.2 page-draw geometry lane) ─────
//
// A data bar is a DRAWN RECT, lowered to a `paged.draw` insertPath — the same
// native-vector lane charts use (chart.ts), NOT a style cell fill. The engine
// (sheet-lower) already decided every rect's content-space geometry + colour;
// this turns each into one insertPath (closed 4-corner ring) + a swatch + a
// frameFillColor ref. The swatch/colorRef discipline mirrors chart.ts (the
// host's frameFillColor takes a Color/<id> ref, not inline hex).

/** The `$created` sentinel as a polygon id — an insertPath mints a polygon
 *  node, addressed for its fill via this (the host resolves it to the
 *  most-recent creating child of the batch). */
const CREATED_DATABAR: ElementId = { kind: "polygon", id: "$created" };

/** Normalise a `#RRGGBB`/`#RGB` (any case) to the uppercase 6-digit body
 *  WITHOUT `#`, or null if malformed (a defensive guard — a bad colour degrades
 *  to "no fill", never crashes). Shared shape with chart.ts's normaliser. */
function normalizeBarHex(hex: string): string | null {
  const m = /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(hex.trim());
  if (!m) return null;
  let body = m[1];
  if (body.length === 3) {
    body = body[0] + body[0] + body[1] + body[1] + body[2] + body[2];
  }
  return body.toUpperCase();
}

/** The deterministic document-swatch id for a data-bar colour (keyed by the
 *  canonical hex so one colour reuses ONE swatch across bars + re-lowers). */
function barSwatchId(canonHex: string): string {
  return `Color/uPagedSheetDataBar${canonHex}`;
}

/** The distinct data-bar colours across the content, in first-appearance
 *  order (deterministic), as canonical hex bodies. */
function distinctBarColors(content: LoweredContent): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const bar of content.databars ?? []) {
    const h = normalizeBarHex(bar.fill);
    if (h == null || seen.has(h)) continue;
    seen.add(h);
    out.push(h);
  }
  return out;
}

/** The createSwatch ops for every distinct data-bar colour (RGB process
 *  swatches at deterministic ids; not "creating children", so they leave the
 *  `$created` sentinel untouched — emitted before the bar paths). */
function barSwatchOps(content: LoweredContent): Mutation[] {
  return distinctBarColors(content).map((canonHex) => {
    const spec: SwatchSpec = {
      selfId: barSwatchId(canonHex),
      name: `paged.sheet data bar ${canonHex}`,
      space: "RGB",
      value: [
        parseInt(canonHex.slice(0, 2), 16),
        parseInt(canonHex.slice(2, 4), 16),
        parseInt(canonHex.slice(4, 6), 16),
      ],
      model: "Process",
    };
    return { op: "createSwatch", args: { spec } };
  });
}

/** The insertPath (closed rect ring) + frameFillColor ops for ONE data bar,
 *  offset by the frame origin `[top, left]` into page-local pt. A zero-width
 *  bar (value at domain min) is skipped (nothing to draw). */
function dataBarOps(
  bar: DataBarRect,
  pageId: PageId,
  top: number,
  left: number,
): Mutation[] {
  if (bar.w <= 0) return [];
  const x0 = left + bar.x;
  const y0 = top + bar.y;
  const x1 = x0 + bar.w;
  const y1 = y0 + bar.h;
  const corner = (p: [number, number]) => ({ anchor: p, left: p, right: p });
  const anchors = (
    [
      [x0, y0],
      [x1, y0],
      [x1, y1],
      [x0, y1],
    ] as [number, number][]
  ).map(corner);
  const ops: Mutation[] = [
    { op: "insertPath", args: { pageId, anchors, open: false } },
  ];
  const fillHex = normalizeBarHex(bar.fill);
  if (fillHex != null) {
    ops.push({
      op: "setElementProperty",
      args: {
        elementId: CREATED_DATABAR,
        path: "frameFillColor",
        value: { type: "colorRef", value: barSwatchId(fillHex) },
      },
    });
  }
  return ops;
}

/** Translate lowered IR + a resolved placement + the frame binding into
 *  the phase-1 batch and the phase-2 text. Pure: no host import beyond
 *  wire TYPES. */
export function lowerToMutations(
  content: LoweredContent,
  placement: LowerPlacement,
  binding: Binding,
): LowerResult {
  const { pageId, bounds } = placement;
  const [top, left] = bounds;

  const ops: Mutation[] = [];

  // (1) The frame itself.
  ops.push({ op: "insertTextFrame", args: { pageId, bounds } });

  // (2) One drawn line per grid rule, in page-local coords. The IR
  // carries content-space offsets (relative to the region's top-left);
  // add the frame's [top, left] origin. An h-rule runs horizontally at
  // y = top + at, from x = left + from to x = left + to; a v-rule runs
  // vertically at x = left + at, from y = top + from to y = top + to.
  for (const rule of content.rules.h) {
    ops.push({
      op: "insertLine",
      args: {
        pageId,
        start: [left + rule.from, top + rule.at],
        end: [left + rule.to, top + rule.at],
      },
    });
  }
  for (const rule of content.rules.v) {
    ops.push({
      op: "insertLine",
      args: {
        pageId,
        start: [left + rule.at, top + rule.from],
        end: [left + rule.at, top + rule.to],
      },
    });
  }

  // (3) Conditional-formatting DATA BARS as drawn rects (spec §8.2 page-draw
  // geometry lane). The engine already decided each bar's content-space rect +
  // colour; we mint one swatch per distinct bar colour (first, so the per-path
  // frameFillColor refs resolve — not creating children, so they leave the
  // textFrame `$created` sentinel untouched), then one insertPath + fill ref
  // per bar. Emitted BEFORE the binding so `setPluginMetadata` stays LAST.
  ops.push(...barSwatchOps(content));
  for (const bar of content.databars ?? []) {
    ops.push(...dataBarOps(bar, pageId, top, left));
  }

  // (4) The binding, written onto the batch-created frame via the
  // `$created` sentinel. ONE undo removes the frame, its rules, the data
  // bars, AND the binding (the plugin-web single-undo property). The binding
  // targets the textFrame KIND, so the interleaved line/polygon creates do
  // not shift its `$created`. The value is the JSON-stringified envelope.
  ops.push({
    op: "setPluginMetadata",
    args: {
      elementId: CREATED_FRAME,
      key: BINDING_KEY,
      value: JSON.stringify(binding),
    },
  });

  return {
    batch: { op: "batch", args: { ops } },
    text: joinText(content),
    // The prepared style overrides (spec §8.3). The phase-1 batch stays the
    // honest tab-text degradation (frame + rules + binding); the styles ride
    // alongside so the caller can apply the expressible character-level
    // overrides once the run offsets resolve (the doc-style-group path).
    styles: styleEmissions(content),
  };
}
