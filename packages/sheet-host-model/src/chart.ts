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

// THE chart lowering translator (spec §8.4): the engine has ALREADY
// generated the pure chart GEOMETRY IR (sheet-chart, computed in Rust);
// this turns those vector primitives into host paged.draw Mutations.
// ZERO chart semantics live here (CLAUDE.md hard rule) — it is arithmetic
// over already-decided geometry plus the host mutation vocabulary.
//
// §8.4 / §2.1: charts lower to paged.draw — a CORE SDK surface reached
// through the native wire ops (insertPath / insertTextFrame / insertText),
// NEVER another plugin. A chart on the page is document-native vector art,
// regenerated on recalc, printable/exportable like anything Paged draws.
//
// COORDINATES (mirrors lower-to-mutations.ts). The geometry IR is in
// chart-content pt space (origin top-left, y DOWN — the frame-content
// system, spec §8.5). placement.bounds is the page-local box
// [top, left, bottom, right]; we add its [top, left] origin to every
// content-space point so the paths land on the page. Frame transforms are
// core's job (§8.5) — we never anticipate them.
//
// TWO-PHASE for text (the §8.4 analogue of the S-03 sheet lower). Vector
// primitives (rect/line/polygon/wedge) lower to insertPath in ONE undoable
// batch. Text labels (title, axis ticks, legend) need a storyId that only
// exists AFTER a text frame is created, so each label mints an
// insertTextFrame in the batch and its string rides out in `texts` for the
// caller to pour (insertText) once it resolves the new frames' stories —
// exactly the resolve-the-created-element door the sheet lower uses.
//
// COLOUR (spec §8.4 swatch-coherence). The geometry IR carries each
// primitive's fill/stroke as a `#RRGGBB` literal (the palette + the
// pie-slice distinction). The host's `frameFillColor`/`frameStrokeColor`
// take a `Value::ColorRef` — a `Color/<id>` SWATCH reference, NOT inline
// hex (verified against core: `paged-mutate` stores the ref string and the
// CMM resolves it through the document's swatch table; IDML FillColor is a
// reference, the FillStrokeCluster precedent). So colours can't ride inline.
// We therefore (1) mint ONE document swatch per DISTINCT chart colour
// (createSwatch, RGB space, 0..255 channels — the IDML convention, core
// graphic.rs `v/255.0`) at a DETERMINISTIC id derived from the hex, then
// (2) reference that swatch id on each path via setElementProperty. This is
// the path the wire actually supports AND it advances swatch-coherence
// (§8.4): the chart's colours become real, named, round-trippable document
// swatches a designer can see and re-use, not anonymous one-off fills.
//
// The per-element targeting uses the SAME protocol-v34 `$created` sentinel
// the binding uses: a setElementProperty whose elementId is `$created`
// resolves to the element minted by the MOST-RECENT creating child of the
// batch (core model.rs). So each path's style ops are emitted IMMEDIATELY
// after its insertPath, while `$created` still points at it. (createSwatch
// is NOT a "creating child" — core `created_element_id` only counts
// InsertNode/CreateGroup — so the swatch ops can sit first without
// disturbing the sentinel.) The binding rides the FIRST created element, so
// it is emitted right after that element's own style ops, before the next
// create shifts the sentinel.

import type {
  ElementId,
  Mutation,
  PageId,
  SwatchSpec,
  Value,
} from "@paged-media/plugin-api";

import { BINDING_KEY, type Binding } from "./binding";

// ── ChartGeometry mirror (the serialized sheet_chart IR, camelCase) ─────────
//
// Mirrors sheet-chart/src/geometry.rs's serde output exactly: the tagged
// `Primitive` enum (lowercase `kind`), camelCase fields, `pts` as [x,y]
// tuples. This is the wire shape `get_chart_geometry` returns.

/** Text alignment for a chart text label's anchor point. */
export type TextAnchor = "start" | "middle" | "end";

/** An axis-aligned rectangle (bars, plot frame, legend swatches). */
export interface RectPrim {
  kind: "rect";
  x: number;
  y: number;
  w: number;
  h: number;
  fill: string | null;
  stroke: string | null;
  strokeW: number;
}

/** A polyline (line-chart series, axis rules). At least two points. */
export interface LinePrim {
  kind: "line";
  pts: [number, number][];
  stroke: string;
  strokeW: number;
}

/** A closed polygon (area fills, scatter diamond markers). */
export interface PolygonPrim {
  kind: "polygon";
  pts: [number, number][];
  fill: string | null;
  stroke: string | null;
  strokeW: number;
}

/** A pie/donut wedge: center (cx,cy), radius r, angular span in degrees
 *  (clockwise from 12 o'clock). */
export interface WedgePrim {
  kind: "wedge";
  cx: number;
  cy: number;
  r: number;
  startDeg: number;
  endDeg: number;
  fill: string | null;
  stroke: string | null;
}

/** A text label (title, axis ticks, legend entries). */
export interface TextPrim {
  kind: "text";
  x: number;
  y: number;
  s: string;
  sizePt: number;
  anchor: TextAnchor;
}

/** One chart-geometry primitive (the tagged union). */
export type ChartPrimitive =
  | RectPrim
  | LinePrim
  | PolygonPrim
  | WedgePrim
  | TextPrim;

/** The generated geometry for one chart: the content-box size + ordered
 *  primitive list (`get_chart_geometry`'s return shape). */
export interface ChartGeometry {
  widthPt: number;
  heightPt: number;
  prims: ChartPrimitive[];
}

// ── Placement + result ──────────────────────────────────────────────────────

/** Where the chart frame is placed: the page + the page-local box the
 *  chart-content geometry is offset into. `bounds` is `[top, left, bottom,
 *  right]` — the same order every wire frame op uses. */
export interface ChartPlacement {
  pageId: PageId;
  bounds: [number, number, number, number];
}

/** A label text frame the caller must pour text into (phase 2): the text,
 *  its page-local anchor point, and its size — emitted in batch order so the
 *  caller can pair each `insertTextFrame` outcome with its string. */
export interface ChartTextLabel {
  /** The label string. */
  text: string;
  /** Page-local anchor point `[x, y]` (the text's baseline anchor). */
  at: [number, number];
  /** Font size in pt. */
  sizePt: number;
  /** The text alignment at the anchor. */
  anchor: TextAnchor;
}

/** What the chart translator yields: the phase-1 vector batch + the phase-2
 *  text labels (poured once the caller resolves the created frames' stories).
 *  Mirrors `LowerResult` (lower-to-mutations.ts). */
export interface ChartLowerResult {
  /** One undoable `batch`: every vector path + one insertTextFrame per
   *  label + the binding metadata on the first created element. */
  batch: Mutation;
  /** The text labels, in the SAME order their `insertTextFrame`s appear in
   *  the batch — the phase-2 pour list. */
  texts: ChartTextLabel[];
}

// ── Arc flattening (wedges) ─────────────────────────────────────────────────

/** Wedge arc flattening density: degrees per polyline segment. A wedge's
 *  arc is approximated by a fan of straight segments at this angular step
 *  (publishing-clean; the path stays a closed polygon paged.draw fills). */
const ARC_STEP_DEG = 6;

/** Clockwise-from-12-o'clock degrees → a content-space point on a circle of
 *  radius `r` about `(cx, cy)`. y grows DOWN (frame-content space), so 0° is
 *  straight up (`y = cy - r`), 90° is to the right, matching the generator's
 *  clockwise convention. */
function arcPoint(
  cx: number,
  cy: number,
  r: number,
  deg: number,
): [number, number] {
  const rad = (deg * Math.PI) / 180;
  return [cx + r * Math.sin(rad), cy - r * Math.cos(rad)];
}

/** Flatten a wedge (a pie slice OR a full 0..360 disc) into a closed point
 *  ring: the center, then the arc from start to end at ~ARC_STEP_DEG steps.
 *  A full circle (≈360° span) omits the center (it is a disc, not a slice). */
function wedgeRing(w: WedgePrim): [number, number][] {
  const span = w.endDeg - w.startDeg;
  const isFull = Math.abs(span - 360) < 1e-6 || Math.abs(span) >= 360;
  const steps = Math.max(1, Math.ceil(Math.abs(span) / ARC_STEP_DEG));
  const pts: [number, number][] = [];
  if (!isFull) pts.push([w.cx, w.cy]);
  for (let i = 0; i <= steps; i++) {
    const deg = w.startDeg + (span * i) / steps;
    pts.push(arcPoint(w.cx, w.cy, w.r, deg));
  }
  return pts;
}

// ── Path building (the paged.draw heart) ────────────────────────────────────

/** A corner anchor (no bezier handles): `left`/`right` collapse onto the
 *  anchor, so the segment to/from it is straight. This is how a polyline /
 *  polygon lowers to `insertPath` — the same corner-anchor convention the
 *  draw geometry uses for straight runs. */
function corner(p: [number, number]): {
  anchor: [number, number];
  left: [number, number];
  right: [number, number];
} {
  return { anchor: p, left: p, right: p };
}

/** An `insertPath` mutation over content-space points, offset by the frame
 *  origin `[ox, oy]` into page-local pt. `open` controls whether the path
 *  closes (polyline = open; rect/polygon/wedge = closed). */
function insertPath(
  pageId: PageId,
  ox: number,
  oy: number,
  pts: [number, number][],
  open: boolean,
): Mutation {
  const anchors = pts.map((p) => corner([ox + p[0], oy + p[1]]));
  return { op: "insertPath", args: { pageId, anchors, open } };
}

/** The four corners of a rect as a closed point ring (top-left, top-right,
 *  bottom-right, bottom-left). */
function rectRing(r: RectPrim): [number, number][] {
  return [
    [r.x, r.y],
    [r.x + r.w, r.y],
    [r.x + r.w, r.y + r.h],
    [r.x, r.y + r.h],
  ];
}

// ── Colour → document swatch (spec §8.4 swatch-coherence) ────────────────────

/** The `$created` sentinel as a polygon element id — an insertPath mints a
 *  polygon node, so its fill/stroke/binding address it through this. The host
 *  resolves `$created` to the most-recent creating child of the batch. */
const CREATED_POLYGON: ElementId = { kind: "polygon", id: "$created" };

/** Normalise a `#RGB` / `#RRGGBB` (case-insensitive) hex to the canonical
 *  uppercase 6-digit form WITHOUT the leading `#`, or `null` if it is not a
 *  well-formed hex colour (a defensive guard — a malformed colour degrades to
 *  "no swatch", never crashes the lower). 3-digit shorthand expands per CSS
 *  (`#abc` → `AABBCC`). */
function normalizeHex(hex: string): string | null {
  const m = /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(hex.trim());
  if (!m) return null;
  let body = m[1];
  if (body.length === 3) {
    body = body[0] + body[0] + body[1] + body[1] + body[2] + body[2];
  }
  return body.toUpperCase();
}

/** The deterministic document-swatch id for a chart colour. Keyed by the
 *  canonical hex so the SAME colour across primitives (and across re-lowers)
 *  reuses ONE swatch — the §8.4 coherence property. The `u`-prefixed local
 *  part follows the `Color/u<...>` minted-id convention; the hex makes it
 *  stable + human-recognisable in the Swatches panel. */
function swatchIdForHex(canonHex: string): string {
  return `Color/uPagedSheetChart${canonHex}`;
}

/** RGB channel triple (0..255) from a canonical 6-digit hex body. */
function rgb255(canonHex: string): [number, number, number] {
  return [
    parseInt(canonHex.slice(0, 2), 16),
    parseInt(canonHex.slice(2, 4), 16),
    parseInt(canonHex.slice(4, 6), 16),
  ];
}

/** A `frameFillColor` / `frameStrokeColor` colorRef Value over a swatch id. */
function colorRefValue(swatchId: string): Value {
  return { type: "colorRef", value: swatchId };
}

/** Collect the DISTINCT chart colours (fills + strokes) across the geometry,
 *  in first-appearance order, as canonical hex bodies. Deterministic. */
function distinctColors(geom: ChartGeometry): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  const add = (c: string | null) => {
    if (c == null) return;
    const h = normalizeHex(c);
    if (h == null || seen.has(h)) return;
    seen.add(h);
    out.push(h);
  };
  for (const prim of geom.prims) {
    switch (prim.kind) {
      case "rect":
        add(prim.fill);
        add(prim.stroke);
        break;
      case "line":
        add(prim.stroke);
        break;
      case "polygon":
        add(prim.fill);
        add(prim.stroke);
        break;
      case "wedge":
        add(prim.fill);
        add(prim.stroke);
        break;
      // text carries no fill/stroke colour in the IR.
    }
  }
  return out;
}

/** The createSwatch ops for every distinct chart colour — RGB process
 *  swatches at deterministic ids, 0..255 channels (IDML convention). Emitted
 *  FIRST in the batch; they are not "creating children" so they don't move
 *  the `$created` sentinel. */
function swatchOps(geom: ChartGeometry): Mutation[] {
  return distinctColors(geom).map((canonHex) => {
    const [r, g, b] = rgb255(canonHex);
    const spec: SwatchSpec = {
      selfId: swatchIdForHex(canonHex),
      name: `paged.sheet chart ${canonHex}`,
      space: "RGB",
      value: [r, g, b],
      model: "Process",
    };
    return { op: "createSwatch", args: { spec } };
  });
}

/** The fill/stroke/stroke-weight style ops for a just-created path (addressed
 *  through `$created`). Order is fill → stroke colour → stroke weight; a
 *  null/malformed colour emits nothing (it falls back to the document
 *  default, never a wrong colour). `strokeW` rides only when there IS a
 *  stroke (a 0-weight stroke is no stroke). */
function styleOps(
  fill: string | null,
  stroke: string | null,
  strokeW: number,
): Mutation[] {
  const ops: Mutation[] = [];
  const fillHex = fill == null ? null : normalizeHex(fill);
  if (fillHex != null) {
    ops.push({
      op: "setElementProperty",
      args: {
        elementId: CREATED_POLYGON,
        path: "frameFillColor",
        value: colorRefValue(swatchIdForHex(fillHex)),
      },
    });
  }
  const strokeHex = stroke == null ? null : normalizeHex(stroke);
  if (strokeHex != null) {
    ops.push({
      op: "setElementProperty",
      args: {
        elementId: CREATED_POLYGON,
        path: "frameStrokeColor",
        value: colorRefValue(swatchIdForHex(strokeHex)),
      },
    });
    if (strokeW > 0) {
      ops.push({
        op: "setElementProperty",
        args: {
          elementId: CREATED_POLYGON,
          path: "frameStrokeWeight",
          value: { type: "length", value: strokeW },
        },
      });
    }
  }
  return ops;
}

// ── The translator ──────────────────────────────────────────────────────────

/**
 * Translate a chart's geometry IR + a resolved placement + the frame binding
 * into the phase-1 paged.draw batch and the phase-2 text labels. PURE: no
 * host import beyond wire TYPES, no chart semantics (the geometry was already
 * decided in Rust). Deterministic — same geometry => same mutations.
 *
 * The batch emits, in order:
 *  - one createSwatch per DISTINCT chart colour (first, swatch-coherence),
 * then in geometry order:
 *  - rect      → insertPath (closed 4-corner ring) + fill/stroke style ops,
 *  - line      → insertPath (open polyline) + stroke style ops,
 *  - polygon   → insertPath (closed ring) + fill/stroke style ops,
 *  - wedge     → insertPath (closed center+arc ring; a full disc omits the
 *                center) + fill/stroke style ops, and
 *  - text      → insertTextFrame (a 1-line box at the label anchor),
 * with a setPluginMetadata writing the binding onto the FIRST created element
 * (the `$created` sentinel) so ONE undo removes the whole chart + its binding.
 * Each colour-bearing path's fill/stroke style ops are emitted IMMEDIATELY
 * after its insertPath, so each `$created` resolves to its own element. Each
 * text's string + anchor rides out in `texts` (phase-2 pour order).
 */
export function chartGeometryToMutations(
  geom: ChartGeometry,
  placement: ChartPlacement,
  binding: Binding,
): ChartLowerResult {
  const { pageId, bounds } = placement;
  const [top, left] = bounds;

  // (0) The colour swatches — minted once per distinct colour, FIRST so the
  // per-path frameFillColor/frameStrokeColor refs resolve. Not creating
  // children, so they leave the `$created` sentinel untouched.
  const ops: Mutation[] = swatchOps(geom);
  const texts: ChartTextLabel[] = [];

  // Whether a page element has been created yet (the binding rides the FIRST
  // one). `bound` flips true the moment we emit the binding so later creates
  // don't re-bind. `firstKind` records what the first created node IS so the
  // binding's `$created` placeholder carries the matching element kind.
  let bound = false;

  /** Append a created path/frame's style ops, then — if this is the FIRST
   *  created element — the binding (while `$created` still points at it). */
  const afterCreate = (kind: "polygon" | "textFrame", styles: Mutation[]) => {
    ops.push(...styles);
    if (!bound) {
      bound = true;
      ops.push({
        op: "setPluginMetadata",
        args: {
          elementId: { kind, id: "$created" },
          key: BINDING_KEY,
          value: JSON.stringify(binding),
        },
      });
    }
  };

  for (const prim of geom.prims) {
    switch (prim.kind) {
      case "rect":
        ops.push(insertPath(pageId, left, top, rectRing(prim), false));
        afterCreate("polygon", styleOps(prim.fill, prim.stroke, prim.strokeW));
        break;
      case "line":
        if (prim.pts.length >= 2) {
          ops.push(insertPath(pageId, left, top, prim.pts, true));
          // A polyline has no fill — only the series stroke.
          afterCreate("polygon", styleOps(null, prim.stroke, prim.strokeW));
        }
        break;
      case "polygon":
        if (prim.pts.length >= 2) {
          ops.push(insertPath(pageId, left, top, prim.pts, false));
          afterCreate(
            "polygon",
            styleOps(prim.fill, prim.stroke, prim.strokeW),
          );
        }
        break;
      case "wedge": {
        const ring = wedgeRing(prim);
        if (ring.length >= 3) {
          ops.push(insertPath(pageId, left, top, ring, false));
          // A wedge carries no stroke weight in the IR; a hairline default
          // applies when a stroke colour is present.
          afterCreate("polygon", styleOps(prim.fill, prim.stroke, 0));
        }
        break;
      }
      case "text": {
        // A small 1-line text box anchored at the label point. Width is a
        // generous fixed box (the host wraps/justifies); height ≈ the line.
        const ax = left + prim.x;
        const ay = top + prim.y;
        const h = prim.sizePt * 1.6;
        // Anchor the box around the label point per its alignment so the
        // text reads where the geometry placed it.
        const w = 80;
        const boxLeft =
          prim.anchor === "middle"
            ? ax - w / 2
            : prim.anchor === "end"
              ? ax - w
              : ax;
        const boxTop = ay - h;
        ops.push({
          op: "insertTextFrame",
          args: { pageId, bounds: [boxTop, boxLeft, boxTop + h, boxLeft + w] },
        });
        // A text frame carries no fill/stroke style; it may still be the
        // FIRST created element (a text-only chart) and thus the binding host.
        afterCreate("textFrame", []);
        texts.push({
          text: prim.s,
          at: [ax, ay],
          sizePt: prim.sizePt,
          anchor: prim.anchor,
        });
        break;
      }
    }
  }

  // If NOTHING was created (no primitives, or only degenerate ones) the batch
  // holds at most the swatch ops with no element to bind to — emit an empty
  // batch so the caller skips it (an empty chart binds nothing).
  if (!bound) {
    return { batch: { op: "batch", args: { ops: [] } }, texts };
  }

  return { batch: { op: "batch", args: { ops } }, texts };
}
