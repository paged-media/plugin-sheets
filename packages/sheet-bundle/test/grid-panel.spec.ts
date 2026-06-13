// sheet.grid.panel-edit-contract (the bundle-side panel): the interim
// sheets-mode grid panel (spec §8.1, S-02) renders the engine-windowed
// GridScene as an SVG, click-selects a cell (→ session.setGridSelection →
// the next scene carries the selection chrome), and commits a cell edit
// (double-click → editor → Enter → session.editCell → engine.setCell).
//
// No DOM: react-test-renderer renders the component to a JS test tree; we
// find nodes by their data-* props and invoke their handlers with hand-
// built events. A fake engine returns a hand-built GridScene over the real
// session (so the session's gridScene overlay + editCell write-path are
// exercised), and a fake host records nothing host-side beyond logging.

import { createElement } from "react";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { describe, expect, it } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";
import type { GridScene } from "@paged-media/sheet-host-model";

import {
  createWorkbookSession,
  makeGridPanel,
  type SheetEngine,
} from "../src";

// A 2×2 viewport scene (cols 40pt, rows 20pt) with two populated cells —
// the same shape the host-model grid.spec uses, here returned by the engine.
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

/** A fake engine: one sheet, a fixed GridScene window, and a recording
 *  setCell / setGridSelection so the panel→session→engine path is visible. */
function fakeEngine() {
  const setCellCalls: Array<[number, number, number, string]> = [];
  const setSelCalls: Array<[number, number, number, number, number]> = [];
  const engine: SheetEngine = {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell(sheet, row, col, input) {
      setCellCalls.push([sheet, row, col, input]);
      return { changed: [{ sheet, row, col, display: input }] };
    },
    getCellDisplay: () => "",
    getCellInput: () => "",
    sortRange: () => ({ changed: [], edits: [] }),
    findAll: () => [],
    replaceAll: () => ({ occurrences: 0, changed: [], edits: [], skipped: [] }),
    getRangeLowered: () => ({
      cols: [],
      rows: [],
      rules: { h: [], v: [] },
      merges: [],
    }),
    getRangeValues: () => [],
    paginate: () => [],
    getGridScene: () => scene2x2(),
    setGridSelection(sheet, anchorRow, anchorCol, rows, cols) {
      setSelCalls.push([sheet, anchorRow, anchorCol, rows, cols]);
    },
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 2, cols: 2 }],
    listCharts: () => [],
    listFreezePanes: () => [],
    listDataValidations: () => [],
    listComments: () => [],
    listFunctions: () => [
      { name: "SUM", family: "math", minArgs: 1, maxArgs: null },
      { name: "SUMIF", family: "math", minArgs: 2, maxArgs: 3 },
    ],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
  return { engine, setCellCalls, setSelCalls };
}

/** A minimal fake host — only log is touched by the grid panel. */
function fakeHost(): BundleHost {
  return {
    log: { debug() {}, info() {}, warn() {}, error() {} },
  } as unknown as BundleHost;
}

/** A mouse event whose currentTarget reports a zero-origin rect, so the
 *  panel's pixel→pt mapping is the identity (PX_PER_PT === 1). */
function clickAt(clientX: number, clientY: number) {
  return {
    clientX,
    clientY,
    preventDefault() {},
    currentTarget: {
      getBoundingClientRect: () => ({ left: 0, top: 0 }),
    },
  } as unknown as React.MouseEvent<SVGSVGElement>;
}

/** Walk the test tree collecting nodes carrying the given data-* prop. */
function byData(
  tree: ReactTestRenderer,
  key: string,
  value?: string,
): import("react-test-renderer").ReactTestInstance[] {
  return tree.root.findAll(
    (n) =>
      n.props != null &&
      n.props[key] !== undefined &&
      (value === undefined || n.props[key] === value),
  );
}

/** Mount the grid panel over a session whose engine is forced to the fake
 *  (bypassing the wasm boot — we test the panel, not the artifact). */
function mountPanel() {
  const { engine, setCellCalls, setSelCalls } = fakeEngine();
  const host = fakeHost();
  const session = createWorkbookSession(host);
  // Force the in-memory session into a booted+loaded state directly.
  const st = session.state();
  st.engine = engine;
  st.activeSheet = 0;
  st.fileName = "demo.xlsx";
  st.selectedRange = "A1:B2";

  const Panel = makeGridPanel(host, session);
  let tree!: ReactTestRenderer;
  act(() => {
    tree = create(createElement(Panel));
  });
  return { tree, session, setCellCalls, setSelCalls };
}

describe("sheet_grid_panel_edit_contract: render", () => {
  it("renders the engine-windowed scene as an SVG (cells + gridlines)", () => {
    const { tree } = mountPanel();
    const svgRoots = byData(tree, "data-grid-svg-root");
    expect(svgRoots).toHaveLength(1);
    const inner = svgRoots[0].props.dangerouslySetInnerHTML.__html as string;
    // The formatted cell text the engine produced is painted.
    expect(inner).toContain(">Name</text>");
    expect(inner).toContain(">100</text>");
    // Gridlines from the scene's rule sets.
    expect(inner).toContain("<line ");
    // The surface is present and sized to the viewport extent (80×40 pt).
    const svgProps = svgRoots[0].props;
    expect(svgProps.width).toBe(80);
    expect(svgProps.height).toBe(40);
  });

  it("shows the honest interim-grid notice (S-02 not in-frame yet)", () => {
    const { tree } = mountPanel();
    const notice = byData(tree, "data-grid-honesty");
    expect(notice).toHaveLength(1);
    const text = JSON.stringify(notice[0].props.children);
    expect(text).toContain("Interim panel grid");
  });

  it("renders the honest empty state before a workbook is loaded", () => {
    const host = fakeHost();
    const session = createWorkbookSession(host);
    const Panel = makeGridPanel(host, session);
    let tree!: ReactTestRenderer;
    act(() => {
      tree = create(createElement(Panel));
    });
    // No grid surface, just the import prompt.
    expect(byData(tree, "data-grid-svg-root")).toHaveLength(0);
    expect(byData(tree, "data-sheet-panel", "grid")).toHaveLength(1);
  });
});

describe("sheet_grid_panel_edit_contract: select", () => {
  it("click on a cell records the selection via the session", () => {
    const { tree, session, setSelCalls } = mountPanel();
    const svg = byData(tree, "data-grid-svg-root")[0];
    // Click in col 1 / row 0 (x in [40,80), y in [0,20)).
    act(() => {
      svg.props.onClick(clickAt(50, 10));
    });
    // The session forwarded the selection to the engine…
    expect(setSelCalls).toContainEqual([0, 0, 1, 1, 1]);
    // …and holds it in state so the next scene carries the chrome.
    expect(session.state().gridSelection).toEqual({
      anchorRow: 0,
      anchorCol: 1,
      rows: 1,
      cols: 1,
    });
  });

  it("the re-rendered scene paints the selection rectangle", () => {
    const { tree, session } = mountPanel();
    const svg = byData(tree, "data-grid-svg-root")[0];
    act(() => {
      svg.props.onClick(clickAt(50, 10));
    });
    // After selection the session overlays it on the scene the panel paints.
    const scene = session.gridScene(0, 0, 480, 320);
    expect(scene?.selection).toEqual({
      anchorRow: 0,
      anchorCol: 1,
      rows: 1,
      cols: 1,
    });
  });
});

describe("sheet_grid_panel_edit_contract: edit-commit", () => {
  it("double-click opens the editor seeded with the cell text", () => {
    const { tree } = mountPanel();
    const svg = byData(tree, "data-grid-svg-root")[0];
    act(() => {
      svg.props.onDoubleClick(clickAt(50, 10)); // col1/row0 = "100"
    });
    const editors = byData(tree, "data-grid-editor");
    expect(editors).toHaveLength(1);
    expect(editors[0].props.value).toBe("100");
  });

  it("Enter commits the edited value through session.editCell → engine.setCell", () => {
    const { tree, setCellCalls } = mountPanel();
    const svg = byData(tree, "data-grid-svg-root")[0];
    act(() => {
      svg.props.onDoubleClick(clickAt(10, 10)); // col0/row0 = "Name"
    });
    let editor = byData(tree, "data-grid-editor")[0];
    // Type a new value, then press Enter.
    act(() => {
      editor.props.onChange({ target: { value: "Total" } });
    });
    editor = byData(tree, "data-grid-editor")[0];
    act(() => {
      editor.props.onKeyDown({ key: "Enter", preventDefault() {} });
    });
    // The engine received the commit at the clicked cell (sheet 0, r0, c0).
    expect(setCellCalls).toContainEqual([0, 0, 0, "Total"]);
    // The editor closes after commit.
    expect(byData(tree, "data-grid-editor")).toHaveLength(0);
  });

  it("Escape abandons the edit (no setCell)", () => {
    const { tree, setCellCalls } = mountPanel();
    const svg = byData(tree, "data-grid-svg-root")[0];
    act(() => {
      svg.props.onDoubleClick(clickAt(10, 10));
    });
    const editor = byData(tree, "data-grid-editor")[0];
    act(() => {
      editor.props.onKeyDown({ key: "Escape", preventDefault() {} });
    });
    expect(setCellCalls).toHaveLength(0);
    expect(byData(tree, "data-grid-editor")).toHaveLength(0);
  });

  it("a scroll control re-windows from a new origin", () => {
    const { tree } = mountPanel();
    const down = byData(tree, "data-grid-scroll", "down")[0];
    act(() => {
      down.props.onClick();
    });
    // The origin indicator advances one row.
    const origin = byData(tree, "data-grid-origin")[0];
    const text = JSON.stringify(origin.props.children);
    expect(text).toContain("1"); // R1
  });
});
