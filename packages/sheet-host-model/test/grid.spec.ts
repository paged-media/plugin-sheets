// sheet.grid.* (TS mirror) — the PURE grid-scene geometry translation:
// gridSceneToSvg (text positions, gridline coords, selection rect, style
// fills/borders), the click→cell hit-test (hitCell), the selection-rect
// clamp, and the cell-editor rect. All pure data in, geometry out — zero
// spreadsheet semantics (the engine windows the scene in Rust; this is
// rendering geometry only).

import { describe, expect, it } from "vitest";

import {
  DEFAULT_GRID_SVG_OPTIONS,
  cellEditorRect,
  cssColorToScenePaint,
  gridSceneToSceneLayer,
  gridSceneToSvg,
  hitCell,
  selectionRect,
  viewportHeightPt,
  viewportWidthPt,
  type GridScene,
  type LoweredStyle,
} from "../src";

/** The default key-0 style mirror (matches the Rust IR-v2 default). */
function defaultStyle(): LoweredStyle {
  return {
    key: 0,
    bold: false,
    italic: false,
    fontSizePt: null,
    fontName: null,
    fillRgb: null,
    textRgb: null,
    borderTop: false,
    borderRight: false,
    borderBottom: false,
    borderLeft: false,
  };
}

// A 2-col × 2-row viewport: cols 40pt wide, rows 20pt tall, gridlines at
// every boundary. One left-aligned text cell, one right-aligned number.
function scene2x2(): GridScene {
  return {
    viewport: {
      firstRow: 0,
      firstCol: 0,
      rows: 2,
      cols: 2,
      xOffsets: [0, 40, 80],
      yOffsets: [0, 20, 40],
    },
    cells: [
      { row: 0, col: 0, text: "Name", align: "left", styleKey: 0 },
      { row: 0, col: 1, text: "100", align: "right", styleKey: 0 },
    ],
    styles: [defaultStyle()],
    gridlines: {
      h: [
        { at: 0, from: 0, to: 80 },
        { at: 20, from: 0, to: 80 },
        { at: 40, from: 0, to: 80 },
      ],
      v: [
        { at: 0, from: 0, to: 40 },
        { at: 40, from: 0, to: 40 },
        { at: 80, from: 0, to: 40 },
      ],
    },
    selection: null,
  };
}

describe("sheet_grid_scene_to_svg: geometry", () => {
  it("sizes the SVG viewBox to the viewport's content-space extent", () => {
    const scene = scene2x2();
    expect(viewportWidthPt(scene)).toBe(80);
    expect(viewportHeightPt(scene)).toBe(40);
    const svg = gridSceneToSvg(scene);
    expect(svg).toContain('viewBox="0 0 80 40"');
    expect(svg).toContain('width="80"');
    expect(svg).toContain('height="40"');
    expect(svg.startsWith("<svg")).toBe(true);
  });

  it("places a left-aligned cell text at col leading edge + pad, row top + baseline", () => {
    const scene = scene2x2();
    const o = DEFAULT_GRID_SVG_OPTIONS;
    const svg = gridSceneToSvg(scene);
    // "Name" at col 0 (x0=0): x = 0 + pad; y = 0 + baseline; anchor start.
    expect(svg).toContain(
      `<text x="${o.pad}" y="${o.baseline}" font-size="${o.fontSizePt}" ` +
        `font-family="${o.fontFamily}" fill="${o.textColor}" ` +
        `text-anchor="start">Name</text>`,
    );
  });

  it("places a right-aligned cell text at col trailing edge - pad with end anchor", () => {
    const scene = scene2x2();
    const o = DEFAULT_GRID_SVG_OPTIONS;
    const svg = gridSceneToSvg(scene);
    // "100" at col 1 (x1=80): x = 80 - pad; anchor end.
    expect(svg).toContain(`x="${80 - o.pad}"`);
    expect(svg).toContain('text-anchor="end">100</text>');
  });

  it("centers a center-aligned cell at the column midpoint with middle anchor", () => {
    const scene = scene2x2();
    scene.cells = [{ row: 0, col: 0, text: "X", align: "center", styleKey: 0 }];
    const svg = gridSceneToSvg(scene);
    // col 0 span [0,40] → midpoint 20.
    expect(svg).toContain('x="20"');
    expect(svg).toContain('text-anchor="middle">X</text>');
  });

  it("emits one gridline per h/v rule at its coordinate", () => {
    const scene = scene2x2();
    const svg = gridSceneToSvg(scene);
    // h-rule at y=20 spans x 0..80.
    expect(svg).toContain('<line x1="0" y1="20" x2="80" y2="20"');
    // v-rule at x=40 spans y 0..40.
    expect(svg).toContain('<line x1="40" y1="0" x2="40" y2="40"');
    // 3 h + 3 v = 6 gridlines, no cell borders/selection in this fixture.
    expect((svg.match(/<line /g) ?? []).length).toBe(6);
  });

  it("respects the include-gridlines toggle via empty rule sets", () => {
    const scene = scene2x2();
    scene.gridlines = { h: [], v: [] };
    const svg = gridSceneToSvg(scene);
    expect(svg).not.toContain("<line ");
  });

  it("draws a style fill rect under the cell from the styles table", () => {
    const scene = scene2x2();
    scene.styles = [{ ...defaultStyle(), key: 1, fillRgb: "#ffeeaa" }];
    scene.cells = [{ row: 0, col: 0, text: "C", align: "left", styleKey: 1 }];
    const svg = gridSceneToSvg(scene);
    // fill rect over col0/row0 span [0,40]×[0,20].
    expect(svg).toContain(
      '<rect x="0" y="0" width="40" height="20" fill="#ffeeaa"/>',
    );
    // The fill precedes the text in the layer order.
    expect(svg.indexOf("fill=\"#ffeeaa\"")).toBeLessThan(svg.indexOf(">C</text>"));
  });

  it("draws cell borders from style flags", () => {
    const scene = scene2x2();
    scene.styles = [
      { ...defaultStyle(), key: 2, borderTop: true, borderLeft: true },
    ];
    scene.cells = [{ row: 0, col: 0, text: "", align: "left", styleKey: 2 }];
    const svg = gridSceneToSvg(scene);
    // top border of col0/row0: y=0 from x0..x1.
    expect(svg).toContain('<line x1="0" y1="0" x2="40" y2="0"');
    // left border: x=0 from y0..y1.
    expect(svg).toContain('<line x1="0" y1="0" x2="0" y2="20"');
  });

  it("applies bold/italic/size/colour from the resolved style", () => {
    const scene = scene2x2();
    scene.styles = [
      {
        ...defaultStyle(),
        key: 3,
        bold: true,
        italic: true,
        fontSizePt: 14,
        textRgb: "#cc0000",
        fontName: "Georgia",
      },
    ];
    scene.cells = [{ row: 0, col: 0, text: "B", align: "left", styleKey: 3 }];
    const svg = gridSceneToSvg(scene);
    expect(svg).toContain('font-size="14"');
    expect(svg).toContain('font-family="Georgia"');
    expect(svg).toContain('fill="#cc0000"');
    expect(svg).toContain('font-weight="700"');
    expect(svg).toContain('font-style="italic"');
  });

  it("XML-escapes formatted cell text (no raw markup leaks)", () => {
    const scene = scene2x2();
    scene.cells = [
      { row: 0, col: 0, text: "<a & \"b\">", align: "left", styleKey: 0 },
    ];
    const svg = gridSceneToSvg(scene);
    expect(svg).toContain("&lt;a &amp; &quot;b&quot;&gt;");
    expect(svg).not.toContain("<a &");
  });

  it("draws the selection rectangle (fill + stroke) over the anchored span", () => {
    const scene = scene2x2();
    scene.selection = { anchorRow: 0, anchorCol: 1, rows: 1, cols: 1 };
    const o = DEFAULT_GRID_SVG_OPTIONS;
    const svg = gridSceneToSvg(scene);
    // col1/row0 span [40,80]×[0,20].
    expect(svg).toContain(
      `<rect x="40" y="0" width="40" height="20" ` +
        `fill="${o.selectionFill}" stroke="${o.selectionColor}" ` +
        `stroke-width="${o.selectionWidth}"/>`,
    );
  });

  it("honours colour option overrides (the token layer)", () => {
    const scene = scene2x2();
    const svg = gridSceneToSvg(scene, {
      gridColor: "var(--pg-border)",
      textColor: "var(--pg-fg)",
    });
    expect(svg).toContain('stroke="var(--pg-border)"');
    expect(svg).toContain('fill="var(--pg-fg)"');
  });
});

// sheet.grid.freeze.render — the frozen-pane split is drawn as a heavier rule
// at the frozen band edge (spec §8.1) when the viewport shows the sheet origin.
describe("sheet_grid_scene_freeze: split rendering", () => {
  function frozenScene(): GridScene {
    const s = scene2x2();
    // Freeze the first column (40pt band) + the first row (20pt band).
    s.freeze = { rows: 1, cols: 1, frozenWidthPt: 40, frozenHeightPt: 20 };
    return s;
  }

  it("draws a horizontal + vertical split rule at the frozen band edges", () => {
    const svg = gridSceneToSvg(frozenScene());
    const o = DEFAULT_GRID_SVG_OPTIONS;
    // Horizontal split at y = frozenHeightPt (20), spanning the width (80).
    expect(svg).toContain(
      `<line x1="0" y1="20" x2="80" y2="20" ` +
        `stroke="${o.freezeColor}" stroke-width="${o.freezeWidth}"/>`,
    );
    // Vertical split at x = frozenWidthPt (40), spanning the height (40).
    expect(svg).toContain(
      `<line x1="40" y1="0" x2="40" y2="40" ` +
        `stroke="${o.freezeColor}" stroke-width="${o.freezeWidth}"/>`,
    );
  });

  it("draws no split when nothing is frozen", () => {
    const svg = gridSceneToSvg(scene2x2());
    expect(svg).not.toContain(DEFAULT_GRID_SVG_OPTIONS.freezeColor);
  });

  it("omits the split when scrolled past the origin (band off-screen)", () => {
    const s = frozenScene();
    s.viewport.firstRow = 5;
    s.viewport.firstCol = 5;
    const svg = gridSceneToSvg(s);
    expect(svg).not.toContain(DEFAULT_GRID_SVG_OPTIONS.freezeColor);
  });
});

// sheet.grid.comments.indicator — a commented cell gets a corner-triangle
// marker at its top-right corner (preserve-first display; spec §10.2).
describe("sheet_grid_scene_comments: indicator rendering", () => {
  it("draws a corner triangle at each comment marker", () => {
    const s = scene2x2();
    // A1's top-right corner is (40, 0) in this 2x2 fixture.
    s.comments = [{ row: 0, col: 0, x: 40, y: 0 }];
    const svg = gridSceneToSvg(s);
    const o = DEFAULT_GRID_SVG_OPTIONS;
    // Triangle hangs down-and-left from the corner (size = commentSize).
    expect(svg).toContain(
      `<polygon points="${40 - o.commentSize},0 40,0 40,${o.commentSize}" ` +
        `fill="${o.commentColor}"/>`,
    );
  });

  it("draws no marker when no cell carries a comment", () => {
    const svg = gridSceneToSvg(scene2x2());
    expect(svg).not.toContain(DEFAULT_GRID_SVG_OPTIONS.commentColor);
  });
});

describe("sheet_grid_scene_databars: data-bar rect rendering", () => {
  it("draws a filled rect for each visible data bar with the rule colour + opacity", () => {
    const s = scene2x2();
    // A2 (row1,col0) bar: engine-given inset rect within the cell box.
    s.databars = [
      { row: 1, col: 0, x: 1.5, y: 21.5, w: 30, h: 17, fillFraction: 0.75, fill: "#638EC6" },
    ];
    const svg = gridSceneToSvg(s);
    const o = DEFAULT_GRID_SVG_OPTIONS;
    expect(svg).toContain(
      `<rect x="1.5" y="21.5" width="30" height="17" ` +
        `fill="#638EC6" fill-opacity="${o.dataBarOpacity}"/>`,
    );
  });

  it("draws the bar UNDER the cell text (the value reads over the bar)", () => {
    const s = scene2x2();
    s.cells = [{ row: 0, col: 0, text: "V", align: "left", styleKey: 0 }];
    s.databars = [
      { row: 0, col: 0, x: 1.5, y: 1.5, w: 20, h: 17, fillFraction: 0.5, fill: "#638EC6" },
    ];
    const svg = gridSceneToSvg(s);
    expect(svg.indexOf('fill="#638EC6"')).toBeLessThan(svg.indexOf(">V</text>"));
  });

  it("skips a zero-width bar and an off-window bar", () => {
    const s = scene2x2();
    s.databars = [
      { row: 0, col: 0, x: 1.5, y: 1.5, w: 0, h: 17, fillFraction: 0, fill: "#638EC6" },
      { row: 99, col: 0, x: 1.5, y: 1.5, w: 10, h: 17, fillFraction: 0.5, fill: "#638EC6" },
    ];
    const svg = gridSceneToSvg(s);
    expect(svg).not.toContain("#638EC6");
  });

  it("draws nothing when the scene carries no data bars", () => {
    const svg = gridSceneToSvg(scene2x2());
    expect(svg).not.toContain("fill-opacity");
  });
});

describe("sheet_grid_selection: rect clamp", () => {
  it("returns null when there is no selection", () => {
    expect(selectionRect(scene2x2())).toBeNull();
  });

  it("computes the viewport-local pt rect for an in-window span", () => {
    const scene = scene2x2();
    scene.selection = { anchorRow: 0, anchorCol: 0, rows: 2, cols: 1 };
    expect(selectionRect(scene)).toEqual([0, 0, 40, 40]);
  });

  it("clamps a span that overflows the visible tracks", () => {
    const scene = scene2x2();
    // 5×5 span anchored at origin — clamps to the 2×2 window.
    scene.selection = { anchorRow: 0, anchorCol: 0, rows: 5, cols: 5 };
    expect(selectionRect(scene)).toEqual([0, 0, 80, 40]);
  });

  it("returns null for a selection entirely outside the window", () => {
    const scene = scene2x2();
    scene.selection = { anchorRow: 10, anchorCol: 10, rows: 1, cols: 1 };
    expect(selectionRect(scene)).toBeNull();
  });

  it("respects a scrolled viewport origin (firstRow/firstCol offset)", () => {
    const scene = scene2x2();
    scene.viewport.firstRow = 5;
    scene.viewport.firstCol = 3;
    // Absolute (5,4) → band (0,1) → col1/row0 span [40,80]×[0,20].
    scene.selection = { anchorRow: 5, anchorCol: 4, rows: 1, cols: 1 };
    expect(selectionRect(scene)).toEqual([40, 0, 40, 20]);
  });
});

describe("sheet_grid_panel_edit_contract: hit-test + editor rect", () => {
  it("maps a click point to the absolute (row,col) it falls in", () => {
    const scene = scene2x2();
    expect(hitCell(scene, 10, 10)).toEqual({ row: 0, col: 0 });
    expect(hitCell(scene, 50, 10)).toEqual({ row: 0, col: 1 });
    expect(hitCell(scene, 10, 30)).toEqual({ row: 1, col: 0 });
    expect(hitCell(scene, 79.9, 39.9)).toEqual({ row: 1, col: 1 });
  });

  it("returns the cell at a track boundary to the band it opens (half-open)", () => {
    const scene = scene2x2();
    // x=40 is the leading edge of col1 (bands are [lo, hi)).
    expect(hitCell(scene, 40, 0)).toEqual({ row: 0, col: 1 });
  });

  it("returns null outside the windowed tracks", () => {
    const scene = scene2x2();
    expect(hitCell(scene, -1, 10)).toBeNull();
    expect(hitCell(scene, 10, -1)).toBeNull();
    expect(hitCell(scene, 80, 10)).toBeNull(); // past trailing x edge
    expect(hitCell(scene, 10, 40)).toBeNull(); // past trailing y edge
  });

  it("offsets the absolute coordinate by the scrolled viewport origin", () => {
    const scene = scene2x2();
    scene.viewport.firstRow = 100;
    scene.viewport.firstCol = 7;
    expect(hitCell(scene, 50, 30)).toEqual({ row: 101, col: 8 });
  });

  it("computes the cell-editor overlay rect for an in-window cell", () => {
    const scene = scene2x2();
    expect(cellEditorRect(scene, 0, 1)).toEqual([40, 0, 40, 20]);
    expect(cellEditorRect(scene, 1, 0)).toEqual([0, 20, 40, 20]);
  });

  it("returns null for a cell outside the window", () => {
    const scene = scene2x2();
    expect(cellEditorRect(scene, 9, 9)).toBeNull();
  });

  it("editor rect honours a scrolled origin", () => {
    const scene = scene2x2();
    scene.viewport.firstRow = 5;
    scene.viewport.firstCol = 3;
    // Absolute (6,4) → band (1,1) → col1/row1 span [40,80]×[20,40].
    expect(cellEditorRect(scene, 6, 4)).toEqual([40, 20, 40, 20]);
  });
});

describe("sheet_grid_scene_to_scene_layer: in-frame C-1 lowering", () => {
  it("lowers fills + gridlines + text to SceneItems (fills→lines→text order)", () => {
    const scene = scene2x2(); // 2 text cells, no fills, 3h+3v gridlines
    const layer = gridSceneToSceneLayer(scene);
    const kinds = layer.items.map((i) => i.kind);
    // 0 fills (default style has no fillRgb) + 6 strokes + 2 text.
    expect(kinds.filter((k) => k === "fillPath").length).toBe(0);
    expect(kinds.filter((k) => k === "strokePath").length).toBe(6);
    expect(kinds.filter((k) => k === "text").length).toBe(2);
    // A gridline is a 2-point stroke at the rule's coordinates (h at y=20).
    const hLine = layer.items.find(
      (i) => i.kind === "strokePath" && i.path[0].op === "moveTo" && i.path[0].y === 20,
    );
    expect(hLine).toBeDefined();
  });

  it("positions cell text at the cell's leading edge + baseline", () => {
    const scene = scene2x2();
    const layer = gridSceneToSceneLayer(scene);
    const name = layer.items.find((i) => i.kind === "text" && i.text === "Name");
    expect(name).toBeDefined();
    if (name && name.kind === "text") {
      // col0 leading edge (0) + pad; row0 top (0) + baseline.
      expect(name.x).toBe(DEFAULT_GRID_SVG_OPTIONS.pad);
      expect(name.y).toBe(DEFAULT_GRID_SVG_OPTIONS.baseline);
    }
  });

  it("lowers a style fill to a FillPath rect over the cell", () => {
    const scene = scene2x2();
    scene.styles = [{ ...defaultStyle(), key: 1, fillRgb: "#ffeeaa" }];
    scene.cells = [{ row: 0, col: 0, text: "C", align: "left", styleKey: 1 }];
    const layer = gridSceneToSceneLayer(scene);
    const fill = layer.items.find((i) => i.kind === "fillPath");
    expect(fill).toBeDefined();
    if (fill && fill.kind === "fillPath") {
      // rect over [0,40]×[0,20]: moveTo(0,0) → lineTo(40,0) → (40,20) → (0,20).
      expect(fill.path[0]).toEqual({ op: "moveTo", x: 0, y: 0 });
      expect(fill.path[1]).toEqual({ op: "lineTo", x: 40, y: 0 });
      expect(fill.path[2]).toEqual({ op: "lineTo", x: 40, y: 20 });
      // #ffeeaa → sRGB (1.0, ~0.933, ~0.667).
      expect(fill.paint.r).toBeCloseTo(1.0, 3);
      expect(fill.paint.g).toBeCloseTo(0xee / 255, 3);
      expect(fill.paint.b).toBeCloseTo(0xaa / 255, 3);
      expect(fill.paint.a).toBe(1);
    }
  });

  it("parses css colours (hex short/long, rgb, rgba) and falls back to black", () => {
    expect(cssColorToScenePaint("#fff")).toEqual({ r: 1, g: 1, b: 1, a: 1 });
    expect(cssColorToScenePaint("#000000")).toEqual({ r: 0, g: 0, b: 0, a: 1 });
    const rgba = cssColorToScenePaint("rgba(255, 0, 0, 0.5)");
    expect(rgba.r).toBe(1);
    expect(rgba.a).toBe(0.5);
    // A token / unparseable value → opaque black (never silently invisible).
    expect(cssColorToScenePaint("var(--pg-border)")).toEqual({
      r: 0,
      g: 0,
      b: 0,
      a: 1,
    });
  });

  it("lowers the selection to a fill wash + stroke over the anchored cell (K-1)", () => {
    const scene = scene2x2();
    scene.selection = { anchorRow: 0, anchorCol: 1, rows: 1, cols: 1 };
    const layer = gridSceneToSceneLayer(scene);
    // selectionRect for col1 = [40, 0, 40, 20]; the wash + stroke are the
    // LAST two items (drawn over cell content).
    const sel = layer.items.slice(-2);
    expect(sel.map((i) => i.kind)).toEqual(["fillPath", "strokePath"]);
    const [wash, stroke] = sel;
    if (wash.kind === "fillPath" && stroke.kind === "strokePath") {
      // rect over [40,80]×[0,20].
      expect(wash.path[0]).toEqual({ op: "moveTo", x: 40, y: 0 });
      expect(wash.path[2]).toEqual({ op: "lineTo", x: 80, y: 20 });
      // The wash carries the translucent selectionFill alpha (0.12).
      expect(wash.paint.a).toBeCloseTo(0.12, 3);
      expect(stroke.width).toBe(DEFAULT_GRID_SVG_OPTIONS.selectionWidth);
    }
  });

  it("emits no selection items when the scene carries no selection", () => {
    const layer = gridSceneToSceneLayer(scene2x2());
    // 6 strokes (gridlines) + 2 text, 0 fills — no selection wash/stroke.
    expect(layer.items.filter((i) => i.kind === "fillPath").length).toBe(0);
    expect(layer.items.filter((i) => i.kind === "strokePath").length).toBe(6);
  });

  it("lowers a data bar to a FillPath rect with the rule colour + opacity, under the text", () => {
    const scene = scene2x2();
    scene.cells = [{ row: 0, col: 0, text: "V", align: "left", styleKey: 0 }];
    scene.databars = [
      { row: 0, col: 0, x: 1.5, y: 1.5, w: 20, h: 17, fillFraction: 0.5, fill: "#638EC6" },
    ];
    const layer = gridSceneToSceneLayer(scene);
    const barIdx = layer.items.findIndex((i) => i.kind === "fillPath");
    const textIdx = layer.items.findIndex((i) => i.kind === "text" && i.text === "V");
    expect(barIdx).toBeGreaterThanOrEqual(0);
    // The bar is drawn before (under) the text.
    expect(barIdx).toBeLessThan(textIdx);
    const bar = layer.items[barIdx];
    if (bar.kind === "fillPath") {
      // rect over [1.5, 21.5]×[1.5, 18.5].
      expect(bar.path[0]).toEqual({ op: "moveTo", x: 1.5, y: 1.5 });
      expect(bar.path[2]).toEqual({ op: "lineTo", x: 21.5, y: 18.5 });
      // #638EC6 → sRGB, alpha = the data-bar opacity wash.
      expect(bar.paint.r).toBeCloseTo(0x63 / 255, 3);
      expect(bar.paint.a).toBeCloseTo(DEFAULT_GRID_SVG_OPTIONS.dataBarOpacity, 3);
    }
  });
});
