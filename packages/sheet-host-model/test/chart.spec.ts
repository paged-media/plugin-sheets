// sheet.chart.lower.paged-draw — the chart geometry → paged.draw
// translator (spec §8.4 / §2.1). PURE: chart-geometry IR in, native
// paged.draw wire ops out (insertPath / insertTextFrame), the binding on
// the $created sentinel. No chart semantics here (decided in Rust).

import { describe, expect, it } from "vitest";

import {
  chartGeometryToMutations,
  makeBinding,
  type Binding,
  type ChartGeometry,
  type ChartPlacement,
} from "../src";

const placement: ChartPlacement = {
  pageId: "Page/u1",
  // [top, left, bottom, right] — origin (10, 20) page-local.
  bounds: [10, 20, 210, 320],
};
const binding: Binding = makeBinding("Sheet1", "A1:B4", 3);

/** Pull the ops array out of a batch mutation. */
function batchOps(geom: ChartGeometry): Array<{ op: string; args: unknown }> {
  const { batch } = chartGeometryToMutations(geom, placement, binding);
  expect(batch.op).toBe("batch");
  return (batch as { args: { ops: Array<{ op: string; args: unknown }> } }).args
    .ops;
}

/** The ops with every leading createSwatch stripped — the geometry/style
 *  portion of the batch, after the swatch-coherence prelude. */
function bodyOps(geom: ChartGeometry): Array<{ op: string; args: unknown }> {
  return batchOps(geom).filter((o) => o.op !== "createSwatch");
}

/** The createSwatch ops in the batch. */
function swatchCreateOps(
  geom: ChartGeometry,
): Array<{ op: string; args: { spec: Record<string, unknown> } }> {
  return batchOps(geom).filter((o) => o.op === "createSwatch") as Array<{
    op: string;
    args: { spec: Record<string, unknown> };
  }>;
}

describe("sheet_chart_lower_paged_draw: rect → insertPath", () => {
  it("lowers a rect to a closed 4-corner insertPath, offset by the frame", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 80,
      prims: [
        {
          kind: "rect",
          x: 5,
          y: 6,
          w: 10,
          h: 20,
          fill: "#4E79A7",
          stroke: null,
          strokeW: 0,
        },
      ],
    };
    // Strip the swatch prelude; the body is insertPath + the fill style op +
    // the binding metadata.
    const ops = bodyOps(geom);
    expect(ops.map((o) => o.op)).toEqual([
      "insertPath",
      "setElementProperty",
      "setPluginMetadata",
    ]);
    const path = ops[0] as {
      args: { pageId: string; anchors: Array<{ anchor: [number, number] }>; open: boolean };
    };
    expect(path.args.pageId).toBe("Page/u1");
    expect(path.args.open).toBe(false); // a rect is closed
    // 4 corners, each offset by [left=20, top=10].
    expect(path.args.anchors.map((a) => a.anchor)).toEqual([
      [25, 16],
      [35, 16],
      [35, 36],
      [25, 36],
    ]);
    // Corner anchors have collapsed handles (straight segments).
    const a0 = path.args.anchors[0] as {
      anchor: [number, number];
      left: [number, number];
      right: [number, number];
    };
    expect(a0.left).toEqual(a0.anchor);
    expect(a0.right).toEqual(a0.anchor);
  });
});

describe("sheet_chart_lower_paged_draw: line + polygon", () => {
  it("lowers a polyline to an OPEN insertPath and a polygon to a CLOSED one", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 80,
      prims: [
        {
          kind: "line",
          pts: [
            [0, 0],
            [10, 10],
            [20, 5],
          ],
          stroke: "#E15759",
          strokeW: 1.5,
        },
        {
          kind: "polygon",
          pts: [
            [0, 0],
            [10, 0],
            [5, 10],
          ],
          fill: "#76B7B2",
          stroke: null,
          strokeW: 0,
        },
      ],
    };
    // Body (after swatches): line + its stroke style ops, then polygon + its
    // fill style op, then the binding.
    const ops = bodyOps(geom);
    expect(ops.map((o) => o.op)).toEqual([
      "insertPath", // the polyline
      "setElementProperty", // its stroke colour
      "setElementProperty", // its stroke weight (1.5 > 0)
      "setPluginMetadata", // binding on the FIRST created element
      "insertPath", // the polygon
      "setElementProperty", // its fill colour
    ]);
    expect((ops[0] as { args: { open: boolean } }).args.open).toBe(true);
    expect((ops[4] as { args: { open: boolean } }).args.open).toBe(false);
    // The line's first vertex is offset by the frame origin.
    const line = ops[0] as {
      args: { anchors: Array<{ anchor: [number, number] }> };
    };
    expect(line.args.anchors[0].anchor).toEqual([20, 10]);
    expect(line.args.anchors.length).toBe(3);
  });

  it("drops a degenerate 1-point line (needs >= 2 points)", () => {
    const geom: ChartGeometry = {
      widthPt: 50,
      heightPt: 50,
      prims: [{ kind: "line", pts: [[0, 0]], stroke: "#000", strokeW: 1 }],
    };
    // A lone 1-point line yields no path, so nothing is created and there is
    // no element to bind to → an empty batch.
    const ops = batchOps(geom);
    expect(ops).toEqual([]);
  });
});

describe("sheet_chart_lower_paged_draw: wedge → arc-flattened insertPath", () => {
  it("flattens a pie slice into a closed center+arc ring", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 100,
      prims: [
        {
          kind: "wedge",
          cx: 50,
          cy: 50,
          r: 40,
          startDeg: 0,
          endDeg: 90,
          fill: "#4E79A7",
          stroke: "#888",
        },
      ],
    };
    const ops = bodyOps(geom);
    expect(ops[0].op).toBe("insertPath");
    const path = ops[0] as {
      args: { anchors: Array<{ anchor: [number, number] }>; open: boolean };
    };
    expect(path.args.open).toBe(false);
    // First point is the center (offset by [20,10]): (50+20, 50+10) = (70,60).
    expect(path.args.anchors[0].anchor).toEqual([70, 60]);
    // A 90° slice at 6°/step => 15 arc points + the center = 17 anchors.
    expect(path.args.anchors.length).toBeGreaterThanOrEqual(16);
    // 0° is straight up: cx, cy - r → (50, 10) + origin = (70, 20).
    expect(path.args.anchors[1].anchor[0]).toBeCloseTo(70, 6);
    expect(path.args.anchors[1].anchor[1]).toBeCloseTo(20, 6);
  });

  it("a full 0..360 disc (donut hole) omits the center vertex", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 100,
      prims: [
        {
          kind: "wedge",
          cx: 50,
          cy: 50,
          r: 20,
          startDeg: 0,
          endDeg: 360,
          fill: "#FFFFFF",
          stroke: null,
        },
      ],
    };
    const path = bodyOps(geom)[0] as {
      args: { anchors: Array<{ anchor: [number, number] }> };
    };
    // The disc ring is all arc points (no center): the first point is ON the
    // circle (straight up), NOT at the center.
    expect(path.args.anchors[0].anchor[1]).toBeCloseTo(40, 6); // cy - r + top
  });
});

describe("sheet_chart_lower_paged_draw: text two-phase", () => {
  it("mints an insertTextFrame per label and rides the strings out for phase 2", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 80,
      prims: [
        { kind: "text", x: 50, y: 10, s: "Q1", sizePt: 10, anchor: "middle" },
        { kind: "text", x: 4, y: 40, s: "100", sizePt: 8, anchor: "end" },
      ],
    };
    const { batch, texts } = chartGeometryToMutations(geom, placement, binding);
    const ops = (batch as { args: { ops: Array<{ op: string }> } }).args.ops;
    // 2 text frames; the binding rides the FIRST created frame (so it sits
    // BETWEEN the two insertTextFrames, not at the end). No swatches/styles —
    // text carries no fill/stroke colour.
    expect(ops.map((o) => o.op)).toEqual([
      "insertTextFrame",
      "setPluginMetadata",
      "insertTextFrame",
    ]);
    // The phase-2 pour list mirrors batch order, with page-local anchors.
    expect(texts.map((t) => t.text)).toEqual(["Q1", "100"]);
    expect(texts[0].at).toEqual([70, 20]); // (50+20, 10+10)
    expect(texts[0].anchor).toBe("middle");
  });
});

describe("sheet_chart_lower_paged_draw: binding + determinism", () => {
  it("binds the chart on the FIRST created element via $created", () => {
    const geom: ChartGeometry = {
      widthPt: 50,
      heightPt: 50,
      prims: [
        {
          kind: "rect",
          x: 0,
          y: 0,
          w: 5,
          h: 5,
          fill: "#000",
          stroke: null,
          strokeW: 0,
        },
      ],
    };
    const ops = batchOps(geom);
    const meta = ops.find((o) => o.op === "setPluginMetadata") as {
      op: string;
      args: { elementId: { kind: string; id: string }; key: string; value: string };
    };
    expect(meta.op).toBe("setPluginMetadata");
    // A vector-first chart binds on the minted polygon (insertPath node).
    expect(meta.args.elementId).toEqual({ kind: "polygon", id: "$created" });
    expect(meta.args.key).toBe("x-paged:media.paged.sheet");
    expect(JSON.parse(meta.args.value)).toEqual(binding);
  });

  it("a text-only chart binds on a textFrame; an empty chart emits no ops", () => {
    const textOnly: ChartGeometry = {
      widthPt: 50,
      heightPt: 20,
      prims: [{ kind: "text", x: 25, y: 10, s: "T", sizePt: 10, anchor: "middle" }],
    };
    const ops = batchOps(textOnly);
    const meta = ops.find((o) => o.op === "setPluginMetadata") as {
      args: { elementId: { kind: string } };
    };
    expect(meta.args.elementId.kind).toBe("textFrame");

    const empty: ChartGeometry = { widthPt: 10, heightPt: 10, prims: [] };
    expect(batchOps(empty)).toEqual([]);
  });

  it("is deterministic — same geometry yields identical mutations", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 100,
      prims: [
        {
          kind: "wedge",
          cx: 50,
          cy: 50,
          r: 30,
          startDeg: 0,
          endDeg: 120,
          fill: "#4E79A7",
          stroke: null,
        },
      ],
    };
    const a = chartGeometryToMutations(geom, placement, binding);
    const b = chartGeometryToMutations(geom, placement, binding);
    expect(JSON.stringify(a)).toBe(JSON.stringify(b));
  });
});

// ── Regression: colour lowering (FINDING 1 — chart palette was dropped) ──────
//
// Pre-fix, every primitive lowered to bare geometry and the fill/stroke hex
// from the IR was read into the interfaces but NEVER emitted, so a lowered
// chart rendered uniform-default — the whole palette + pie-slice distinction
// silently lost. These pin that the colour now rides as a document swatch +
// a frameFillColor/frameStrokeColor ref on the path it belongs to.

describe("sheet_chart_lower_paged_draw: colour → swatch + colorRef (FINDING 1)", () => {
  /** Find the setElementProperty for a path right after its insertPath. */
  function styleFor(
    ops: Array<{ op: string; args: unknown }>,
    pathIdx: number,
    propPath: string,
  ): { type: string; value: unknown } | undefined {
    for (let i = pathIdx + 1; i < ops.length; i++) {
      if (ops[i].op === "insertPath" || ops[i].op === "insertTextFrame") break;
      if (ops[i].op !== "setElementProperty") continue;
      const a = ops[i].args as { path: string; value: { type: string; value: unknown } };
      if (a.path === propPath) return a.value;
    }
    return undefined;
  }

  it("a coloured bar lowers its fill to a document RGB swatch + a frameFillColor ref", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 80,
      prims: [
        {
          kind: "rect",
          x: 0,
          y: 0,
          w: 10,
          h: 40,
          fill: "#4E79A7", // tableau blue: R=78 G=121 B=167
          stroke: null,
          strokeW: 0,
        },
      ],
    };
    // A swatch is minted for the fill colour, RGB 0..255 channels.
    const swatches = swatchCreateOps(geom);
    expect(swatches).toHaveLength(1);
    expect(swatches[0].args.spec.space).toBe("RGB");
    expect(swatches[0].args.spec.value).toEqual([78, 121, 167]);
    const swatchId = swatches[0].args.spec.selfId as string;
    expect(swatchId).toMatch(/^Color\/u/);

    // The bar's fill is set via frameFillColor → that swatch, on its $created.
    const ops = bodyOps(geom);
    const pathIdx = ops.findIndex((o) => o.op === "insertPath");
    const fill = styleFor(ops, pathIdx, "frameFillColor");
    expect(fill).toEqual({ type: "colorRef", value: swatchId });
    // The fill op addresses the just-created polygon via the $created sentinel.
    const fillOp = ops.find(
      (o) =>
        o.op === "setElementProperty" &&
        (o.args as { path: string }).path === "frameFillColor",
    ) as { args: { elementId: { kind: string; id: string } } };
    expect(fillOp.args.elementId).toEqual({ kind: "polygon", id: "$created" });
  });

  it("a line series lowers its stroke colour AND weight (no fill)", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 80,
      prims: [
        {
          kind: "line",
          pts: [
            [0, 0],
            [10, 10],
          ],
          stroke: "#E15759", // R=225 G=87 B=89
          strokeW: 2,
        },
      ],
    };
    const swatches = swatchCreateOps(geom);
    expect(swatches.map((s) => s.args.spec.value)).toEqual([[225, 87, 89]]);
    const strokeId = swatches[0].args.spec.selfId as string;

    const ops = bodyOps(geom);
    const pathIdx = ops.findIndex((o) => o.op === "insertPath");
    // A polyline has NO fill, but DOES carry the stroke colour + weight.
    expect(styleFor(ops, pathIdx, "frameFillColor")).toBeUndefined();
    expect(styleFor(ops, pathIdx, "frameStrokeColor")).toEqual({
      type: "colorRef",
      value: strokeId,
    });
    expect(styleFor(ops, pathIdx, "frameStrokeWeight")).toEqual({
      type: "length",
      value: 2,
    });
  });

  it("a coloured wedge (pie slice) lowers its fill — slices stay distinct", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 100,
      prims: [
        {
          kind: "wedge",
          cx: 50,
          cy: 50,
          r: 40,
          startDeg: 0,
          endDeg: 120,
          fill: "#59A14F", // green slice
          stroke: null,
        },
        {
          kind: "wedge",
          cx: 50,
          cy: 50,
          r: 40,
          startDeg: 120,
          endDeg: 240,
          fill: "#EDC948", // yellow slice
          stroke: null,
        },
      ],
    };
    // Two distinct slice colours → two distinct swatches (the pie-slice
    // distinction the pre-fix lower dropped entirely).
    const swatches = swatchCreateOps(geom);
    expect(swatches).toHaveLength(2);
    const ids = swatches.map((s) => s.args.spec.selfId as string);
    expect(new Set(ids).size).toBe(2);

    const ops = bodyOps(geom);
    const pathIdxs = ops
      .map((o, i) => (o.op === "insertPath" ? i : -1))
      .filter((i) => i >= 0);
    expect(pathIdxs).toHaveLength(2);
    const fill0 = styleFor(ops, pathIdxs[0], "frameFillColor") as {
      value: string;
    };
    const fill1 = styleFor(ops, pathIdxs[1], "frameFillColor") as {
      value: string;
    };
    // Each slice references its OWN colour — they are not the same fill.
    expect(fill0.value).toBe(ids[0]);
    expect(fill1.value).toBe(ids[1]);
    expect(fill0.value).not.toBe(fill1.value);
  });

  it("reuses ONE swatch for a repeated colour (swatch-coherence, §8.4)", () => {
    const geom: ChartGeometry = {
      widthPt: 100,
      heightPt: 80,
      prims: [
        { kind: "rect", x: 0, y: 0, w: 5, h: 10, fill: "#4E79A7", stroke: null, strokeW: 0 },
        { kind: "rect", x: 10, y: 0, w: 5, h: 20, fill: "#4E79A7", stroke: null, strokeW: 0 },
      ],
    };
    // Same colour twice → ONE swatch, both bars reference it.
    const swatches = swatchCreateOps(geom);
    expect(swatches).toHaveLength(1);
    const id = swatches[0].args.spec.selfId as string;
    const fills = bodyOps(geom).filter(
      (o) =>
        o.op === "setElementProperty" &&
        (o.args as { path: string }).path === "frameFillColor",
    ) as Array<{ args: { value: { value: string } } }>;
    expect(fills).toHaveLength(2);
    expect(fills.every((f) => f.args.value.value === id)).toBe(true);
  });

  it("3-digit shorthand hex expands; a malformed colour degrades to no swatch", () => {
    const geom: ChartGeometry = {
      widthPt: 50,
      heightPt: 50,
      prims: [
        { kind: "rect", x: 0, y: 0, w: 5, h: 5, fill: "#abc", stroke: null, strokeW: 0 },
        // A malformed colour: no swatch, no fill op (falls back to default).
        { kind: "rect", x: 6, y: 0, w: 5, h: 5, fill: "not-a-color", stroke: null, strokeW: 0 },
      ],
    };
    const swatches = swatchCreateOps(geom);
    // Only the well-formed shorthand mints a swatch; #abc → AABBCC = 170,187,204.
    expect(swatches).toHaveLength(1);
    expect(swatches[0].args.spec.value).toEqual([170, 187, 204]);
    // The malformed-colour bar still creates its path (geometry never lost),
    // it just carries no fill style op.
    const ops = bodyOps(geom);
    const fillOps = ops.filter(
      (o) =>
        o.op === "setElementProperty" &&
        (o.args as { path: string }).path === "frameFillColor",
    );
    expect(fillOps).toHaveLength(1);
    expect(ops.filter((o) => o.op === "insertPath")).toHaveLength(2);
  });
});
