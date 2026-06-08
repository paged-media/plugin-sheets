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

import type { Mutation, PageId } from "@paged-media/plugin-api";

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

// ── The translator ──────────────────────────────────────────────────────────

/**
 * Translate a chart's geometry IR + a resolved placement + the frame binding
 * into the phase-1 paged.draw batch and the phase-2 text labels. PURE: no
 * host import beyond wire TYPES, no chart semantics (the geometry was already
 * decided in Rust). Deterministic — same geometry => same mutations.
 *
 * The batch emits, in geometry order:
 *  - rect      → insertPath (closed 4-corner ring),
 *  - line      → insertPath (open polyline),
 *  - polygon   → insertPath (closed ring),
 *  - wedge     → insertPath (closed center+arc ring; a full disc omits the
 *                center), and
 *  - text      → insertTextFrame (a 1-line box at the label anchor),
 * then a setPluginMetadata writing the binding onto the FIRST created element
 * (the `$created` sentinel) so ONE undo removes the whole chart + its binding.
 * Each text's string + anchor rides out in `texts` (phase-2 pour order).
 */
export function chartGeometryToMutations(
  geom: ChartGeometry,
  placement: ChartPlacement,
  binding: Binding,
): ChartLowerResult {
  const { pageId, bounds } = placement;
  const [top, left] = bounds;

  const ops: Mutation[] = [];
  const texts: ChartTextLabel[] = [];

  for (const prim of geom.prims) {
    switch (prim.kind) {
      case "rect":
        ops.push(insertPath(pageId, left, top, rectRing(prim), false));
        break;
      case "line":
        if (prim.pts.length >= 2) {
          ops.push(insertPath(pageId, left, top, prim.pts, true));
        }
        break;
      case "polygon":
        if (prim.pts.length >= 2) {
          ops.push(insertPath(pageId, left, top, prim.pts, false));
        }
        break;
      case "wedge": {
        const ring = wedgeRing(prim);
        if (ring.length >= 3) {
          ops.push(insertPath(pageId, left, top, ring, false));
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

  // The binding rides on the FIRST created element (the `$created` sentinel
  // resolves to the first minted node of the batch). If the chart has NO
  // primitives there is nothing to bind to — the batch is empty (the caller
  // skips an empty chart).
  if (ops.length > 0) {
    ops.push({
      op: "setPluginMetadata",
      args: {
        elementId: firstCreatedTarget(geom),
        key: BINDING_KEY,
        value: JSON.stringify(binding),
      },
    });
  }

  return { batch: { op: "batch", args: { ops } }, texts };
}

/** The `$created` element target for the binding: the FIRST minted node's
 *  kind. A vector primitive mints a `polygon` (insertPath); a text-only chart
 *  (no vectors — e.g. an empty-data title) mints a `textFrame`. The host
 *  resolves `$created` to the just-minted node and the metadata gate verifies
 *  the key is this plugin's namespace (the lower-to-mutations precedent). */
function firstCreatedTarget(
  geom: ChartGeometry,
): { kind: "polygon"; id: string } | { kind: "textFrame"; id: string } {
  const firstVector = geom.prims.some(
    (p) =>
      p.kind === "rect" ||
      p.kind === "line" ||
      p.kind === "polygon" ||
      p.kind === "wedge",
  );
  return firstVector
    ? { kind: "polygon", id: "$created" }
    : { kind: "textFrame", id: "$created" };
}
