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
    const ops = batchOps(geom);
    // insertPath + the binding metadata = 2 ops.
    expect(ops.map((o) => o.op)).toEqual(["insertPath", "setPluginMetadata"]);
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
    const ops = batchOps(geom);
    expect(ops.map((o) => o.op)).toEqual([
      "insertPath",
      "insertPath",
      "setPluginMetadata",
    ]);
    expect((ops[0] as { args: { open: boolean } }).args.open).toBe(true);
    expect((ops[1] as { args: { open: boolean } }).args.open).toBe(false);
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
    const ops = batchOps(geom);
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
    const path = batchOps(geom)[0] as {
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
    // 2 text frames + the binding.
    expect(ops.map((o) => o.op)).toEqual([
      "insertTextFrame",
      "insertTextFrame",
      "setPluginMetadata",
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
    const meta = ops[ops.length - 1] as {
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
    const meta = ops[ops.length - 1] as {
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
