// sheet.scene-layer.in-frame (K-1): the session renders the windowed
// GridScene INTO a frame via host.contribute.sceneLayer() (C-1), and a
// FRAME-CONTENT-space pointer selects the cell it falls in + re-renders the
// grid with the selection chrome. No DOM, no wasm: a fake engine returns a
// fixed GridScene and a fake host captures every scene-layer submit so we
// can assert what core would compose.

import { describe, expect, it } from "vitest";

import type { BundleHost, SceneLayer } from "@paged-media/plugin-api";
import type { GridScene } from "@paged-media/sheet-host-model";

import { createWorkbookSession, type SheetEngine } from "../src";

// A 2×2 viewport: cols 40pt (x 0/40/80), rows 20pt (y 0/20/40).
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
    styles: [
      {
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
      },
    ],
    gridlines: { h: [], v: [] },
    selection: null,
  };
}

function fakeEngine() {
  const setSelCalls: Array<[number, number, number, number, number]> = [];
  const engine: SheetEngine = {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: (sheet, row, col, input) => ({
      changed: [{ sheet, row, col, display: input }],
    }),
    getCellDisplay: () => "",
    getRangeLowered: () => ({ cols: [], rows: [], rules: { h: [], v: [] }, merges: [] }),
    paginate: () => [],
    // The fake ignores the window args and always returns the 2×2 scene; the
    // session overlays its own selection onto it via computeGridScene.
    getGridScene: () => scene2x2(),
    setGridSelection(sheet, anchorRow, anchorCol, rows, cols) {
      setSelCalls.push([sheet, anchorRow, anchorCol, rows, cols]);
    },
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 2, cols: 2 }],
    listCharts: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
  return { engine, setSelCalls };
}

/** A host that wires the scene-layer channel + a frame geometry read, and
 *  CAPTURES every `submit(elementId, layer)`. */
function fakeHost() {
  const submits: Array<{ elementId: string; layer: SceneLayer }> = [];
  let cleared: string | null = null;
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) => f === "rendering.sceneLayer@1",
    contribute: {
      sceneLayer: () => ({
        async submit(elementId: string, layer: SceneLayer) {
          submits.push({ elementId, layer });
        },
        async clear(elementId: string) {
          cleared = elementId;
        },
        dispose() {},
      }),
    },
    document: {
      // bounds = [top, left, bottom, right] → an 80×40 content box.
      elementGeometry: async () => [{ bounds: [0, 0, 40, 80] }],
    },
  } as unknown as BundleHost;
  return { host, submits, cleared: () => cleared };
}

/** A booted+loaded session over the fakes (bypassing the wasm boot). */
function bootedSession(host: BundleHost, engine: SheetEngine) {
  const session = createWorkbookSession(host);
  const st = session.state();
  st.engine = engine;
  st.activeSheet = 0;
  st.fileName = "demo.xlsx";
  st.selectedRange = "A1:B2";
  return session;
}

describe("sheet_scene_layer_in_frame: K-1 click-to-select", () => {
  it("renders the grid in-frame, then selects the cell under a content point", async () => {
    const { engine, setSelCalls } = fakeEngine();
    const { host, submits } = fakeHost();
    const session = bootedSession(host, engine);

    const shown = await session.showGridInFrame("frame-1");
    expect(shown).toBe(true);
    expect(submits).toHaveLength(1);
    expect(submits[0].elementId).toBe("frame-1");
    // The first render carries no selection (no fillPath wash).
    expect(submits[0].layer.items.some((i) => i.kind === "fillPath")).toBe(false);

    // A content-space point in col1 (x∈[40,80)) row0 (y∈[0,20)).
    const hit = session.selectCellInFrame(50, 10);
    expect(hit).toBe(true);
    // The cell was selected (engine told + session state).
    expect(setSelCalls).toContainEqual([0, 0, 1, 1, 1]);
    // A re-render was submitted (now WITH the selection wash + stroke).
    expect(submits).toHaveLength(2);
    const last = submits[1].layer.items.slice(-2);
    expect(last.map((i) => i.kind)).toEqual(["fillPath", "strokePath"]);
  });

  it("returns false (no extra submit) for a point outside the windowed cells", async () => {
    const { engine } = fakeEngine();
    const { host, submits } = fakeHost();
    const session = bootedSession(host, engine);

    await session.showGridInFrame("frame-1");
    expect(submits).toHaveLength(1);

    const hit = session.selectCellInFrame(999, 999);
    expect(hit).toBe(false);
    expect(submits).toHaveLength(1); // no re-render
  });

  it("returns false before any in-frame grid is shown (nothing to hit)", () => {
    const { engine } = fakeEngine();
    const { host } = fakeHost();
    const session = bootedSession(host, engine);
    expect(session.selectCellInFrame(10, 10)).toBe(false);
  });
});
