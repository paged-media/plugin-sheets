// Wave 5 — editing fundamentals through the SESSION over the REAL engine:
// range selection (drag, shift), the in-frame key map (arrows, Tab/Enter,
// F2, Delete, scroll), multi-cell copy/paste with formula re-addressing and
// TSV interop, the fill handle + Cmd+D/R, find-next, and every multi-cell
// op as ONE undo step. The pure navigation model (grid-nav.ts) is pinned
// first, without the engine.
//
// Real engine: skipped without the wasm artifact, FAILS under
// REQUIRE_REAL_ENGINE=1 (the engine-real.spec.ts dual gate).

import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { createElement } from "react";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { describe, expect, it } from "vitest";

import type { BundleHost, ClipboardPayload, SceneLayer } from "@paged-media/plugin-api";

import {
  advance,
  collapsed,
  createWorkbookSession,
  extendTo,
  fillTarget,
  gridKeyAction,
  moveBy,
  rectOf,
  scrollToShow,
  type WorkbookSession,
} from "../src";
import { makeGridPanel } from "../src/panels/grid-panel";

const HERE = dirname(fileURLToPath(import.meta.url));
const WASM = join(HERE, "..", "bin", "sheet_js_bg.wasm");
const built = existsSync(WASM);

if (process.env.REQUIRE_REAL_ENGINE === "1" && !built) {
  describe("editing (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing`);
    });
  });
}

// ─────────────────────────────────────────────── the pure navigation model

describe("grid navigation model [sheet.grid.selection]", () => {
  it("shift-extends from the anchor and keeps the active cell there", () => {
    const m = extendTo(collapsed({ row: 2, col: 1 }), { row: 0, col: 3 });
    expect(rectOf(m)).toEqual({ anchorRow: 0, anchorCol: 1, rows: 3, cols: 3 });
    expect(m.active).toEqual({ row: 2, col: 1 });
    // Shift-arrow moves the focus, not the anchor.
    const n = moveBy(m, 1, 0, true);
    expect(rectOf(n)).toEqual({ anchorRow: 1, anchorCol: 1, rows: 2, cols: 3 });
    // A plain arrow collapses onto the moved active cell; never above A1.
    expect(rectOf(moveBy(n, -5, 0, false))).toEqual({
      anchorRow: 0,
      anchorCol: 1,
      rows: 1,
      cols: 1,
    });
  });

  it("Tab/Enter walk inside a multi-cell selection and wrap", () => {
    let m = extendTo(collapsed({ row: 0, col: 0 }), { row: 1, col: 1 });
    m = advance(m, "right");
    expect(m.active).toEqual({ row: 0, col: 1 });
    m = advance(m, "right");
    expect(m.active).toEqual({ row: 1, col: 0 }); // wrapped to the next row
    m = advance(m, "down");
    expect(m.active).toEqual({ row: 0, col: 1 }); // column-major walk
    expect(rectOf(m)).toEqual({ anchorRow: 0, anchorCol: 0, rows: 2, cols: 2 });
    // A single cell just moves.
    expect(advance(collapsed({ row: 3, col: 3 }), "up").active).toEqual({ row: 2, col: 3 });
  });

  it("a fill-handle drag extends along the axis left by the most", () => {
    const src = { anchorRow: 0, anchorCol: 0, rows: 2, cols: 1 };
    expect(fillTarget(src, { row: 5, col: 1 })).toEqual({
      anchorRow: 0,
      anchorCol: 0,
      rows: 6,
      cols: 1,
    });
    expect(fillTarget(src, { row: 1, col: 4 })).toEqual({
      anchorRow: 0,
      anchorCol: 0,
      rows: 2,
      cols: 5,
    });
    expect(fillTarget(src, { row: 1, col: 0 })).toBeNull();
  });

  it("scrolls the window as little as possible to show a cell", () => {
    const vis = { rows: 10, cols: 4 };
    expect(scrollToShow({ firstRow: 0, firstCol: 0 }, vis, { row: 12, col: 1 })).toEqual({
      firstRow: 3,
      firstCol: 0,
    });
    expect(scrollToShow({ firstRow: 5, firstCol: 2 }, vis, { row: 1, col: 1 })).toEqual({
      firstRow: 1,
      firstCol: 1,
    });
  });
});

describe("grid key map [sheet.grid.keyboard]", () => {
  it("maps navigation, editing and command keys", () => {
    expect(gridKeyAction({ key: "ArrowDown", shiftKey: true }, null)).toEqual({
      kind: "move",
      dRow: 1,
      dCol: 0,
      extend: true,
    });
    expect(gridKeyAction({ key: "Tab", shiftKey: true }, null)).toEqual({
      kind: "advance",
      dir: "left",
    });
    expect(gridKeyAction({ key: "Enter" }, null)).toEqual({ kind: "advance", dir: "down" });
    expect(gridKeyAction({ key: "F2" }, null)).toEqual({ kind: "edit" });
    expect(gridKeyAction({ key: "Delete" }, null)).toEqual({ kind: "clear" });
    expect(gridKeyAction({ key: "Backspace" }, null)).toEqual({ kind: "clear" });
    expect(gridKeyAction({ key: "x" }, null)).toEqual({ kind: "type", ch: "x" });
    expect(gridKeyAction({ key: "c", metaKey: true }, null)).toEqual({ kind: "copy" });
    expect(gridKeyAction({ key: "v", ctrlKey: true }, null)).toEqual({ kind: "paste" });
    expect(gridKeyAction({ key: "d", metaKey: true }, null)).toEqual({ kind: "fill", dir: "down" });
    expect(gridKeyAction({ key: "z", metaKey: true, shiftKey: true }, null)).toEqual({
      kind: "redo",
    });
    // Mid-edit: Enter/Tab commit and move, arrows commit in Enter mode and
    // stay with the editor in F2 mode, Backspace edits.
    expect(gridKeyAction({ key: "Tab" }, "enter")).toEqual({ kind: "commit", then: "right" });
    expect(gridKeyAction({ key: "ArrowUp" }, "enter")).toEqual({
      kind: "commitMove",
      dRow: -1,
      dCol: 0,
    });
    expect(gridKeyAction({ key: "ArrowUp" }, "edit")).toEqual({ kind: "none" });
    expect(gridKeyAction({ key: "Backspace" }, "edit")).toEqual({ kind: "backspace" });
    expect(gridKeyAction({ key: "Escape" }, "enter")).toEqual({ kind: "cancel" });
  });
});

// ─────────────────────────────────────────────── the session, real engine

/** A host with a scene channel (captures submits), a frame geometry read
 *  (an 80×40 pt content box — four 40×20 cells… the engine decides) and an
 *  in-memory clipboard. */
function fakeHost(opts: { box?: [number, number, number, number] } = {}) {
  const submits: SceneLayer[] = [];
  const clip: { payload: ClipboardPayload | null } = { payload: null };
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) => f === "rendering.sceneLayer@1",
    contribute: {
      sceneLayer: () => ({
        async submit(_id: string, layer: SceneLayer) {
          submits.push(layer);
        },
        async clear() {},
        dispose() {},
      }),
    },
    document: {
      elementGeometry: async () => [{ bounds: opts.box ?? [0, 0, 200, 400] }],
      onDidChange: () => ({ dispose() {} }),
    },
    clipboard: {
      async read() {
        return clip.payload;
      },
      async write(p: ClipboardPayload) {
        clip.payload = p;
      },
    },
    shell: { openPanel() {} },
  } as unknown as BundleHost;
  return { host, submits, clip };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

/** A blank workbook seeded with `cells` (A1-free coordinates). */
async function open(
  cells: [number, number, string][],
  hostOpts?: { box?: [number, number, number, number] },
): Promise<{ s: WorkbookSession; fake: ReturnType<typeof fakeHost> }> {
  const fake = fakeHost(hostOpts);
  const s = createWorkbookSession(fake.host);
  await s.newWorkbook();
  for (const [r, c, v] of cells) expect(s.editCell(0, r, c, v)).toBe(true);
  return { s, fake };
}

const input = (s: WorkbookSession, r: number, c: number) => s.cellInputAt(r, c);
const display = (s: WorkbookSession, r: number, c: number) =>
  s.state().engine!.getCellDisplay(0, r, c);

/** Show the grid in a frame and return a content-space point at the
 *  centre of `(row, col)` in the last rendered window. */
async function inFrame(s: WorkbookSession) {
  expect(await s.showGridInFrame("frame-1")).toBe(true);
  await tick();
  return (row: number, col: number): [number, number] => {
    const sc = s.gridScene(0, 0, 400, 200)!;
    const vp = sc.viewport;
    const ci = col - vp.firstCol;
    const ri = row - vp.firstRow;
    return [
      (vp.xOffsets[ci] + vp.xOffsets[ci + 1]) / 2,
      (vp.yOffsets[ri] + vp.yOffsets[ri + 1]) / 2,
    ];
  };
}

describe.skipIf(!built)("range selection in-frame [sheet.grid.selection]", () => {
  it("drag selects a range; shift-click extends from the anchor", async () => {
    const { s } = await open([]);
    const at = await inFrame(s);
    expect(s.pointerDownInFrame(...at(1, 1))).toBe(true);
    expect(s.pointerMoveInFrame(...at(3, 2))).toBe(true);
    expect(s.pointerUpInFrame(...at(3, 2))).toBe(true);
    expect(s.state().gridSelection).toEqual({ anchorRow: 1, anchorCol: 1, rows: 3, cols: 2 });
    expect(s.activeCell()).toEqual({ row: 1, col: 1 });
    s.pointerUpInFrame(0, 0);
    // Shift-click from the same anchor.
    expect(s.pointerDownInFrame(...at(0, 0), { shift: true })).toBe(true);
    s.pointerUpInFrame(...at(0, 0));
    expect(s.state().gridSelection).toEqual({ anchorRow: 0, anchorCol: 0, rows: 2, cols: 2 });
  });
});

describe.skipIf(!built)("in-frame keys [sheet.grid.keyboard]", () => {
  it("arrows, shift-arrows, Tab and Enter move; typing then Enter commits and moves down", async () => {
    const { s } = await open([]);
    const at = await inFrame(s);
    s.pointerDownInFrame(...at(0, 0));
    s.pointerUpInFrame(...at(0, 0));
    expect(s.handleGridKey({ key: "ArrowRight" })).toBe(true);
    expect(s.handleGridKey({ key: "ArrowDown", shiftKey: true })).toBe(true);
    expect(s.state().gridSelection).toEqual({ anchorRow: 0, anchorCol: 1, rows: 2, cols: 1 });
    expect(s.handleGridKey({ key: "Enter" })).toBe(true); // walks inside
    expect(s.activeCell()).toEqual({ row: 1, col: 1 });
    for (const ch of "42") s.handleGridKey({ key: ch });
    expect(s.isCellEditing()).toBe(true);
    s.handleGridKey({ key: "Tab" }); // commit + walk on (wraps inside the 2×1)
    expect(input(s, 1, 1)).toBe("42");
    expect(s.activeCell()).toEqual({ row: 0, col: 1 });
    s.handleGridKey({ key: "ArrowLeft" });
    s.handleGridKey({ key: "7" });
    s.handleGridKey({ key: "ArrowDown" }); // Enter mode: an arrow commits and moves
    expect(input(s, 0, 0)).toBe("7");
    expect(s.activeCell()).toEqual({ row: 1, col: 0 });
  });

  it("F2 opens the cell's formula; Esc cancels; Delete clears the selection as one undo step", async () => {
    const { s } = await open([
      [0, 0, "1"],
      [1, 0, "2"],
      [2, 0, "=A1+A2"],
    ]);
    await inFrame(s);
    s.selectCell(2, 0);
    expect(s.handleGridKey({ key: "F2" })).toBe(true);
    expect(s.isCellEditing()).toBe(true);
    s.handleGridKey({ key: "+" });
    s.handleGridKey({ key: "ArrowLeft" }); // F2 mode: arrows stay with the editor
    expect(s.isCellEditing()).toBe(true);
    s.handleGridKey({ key: "1" });
    s.handleGridKey({ key: "Enter" });
    expect(input(s, 2, 0)).toBe("=A1+A2+1");
    expect(display(s, 2, 0)).toBe("4");
    // Esc drops an open edit.
    s.selectCell(0, 0);
    s.handleGridKey({ key: "9" });
    s.handleGridKey({ key: "Escape" });
    expect(input(s, 0, 0)).toBe("1");
    // Delete over A1:A2 clears both, one undo restores both.
    s.selectCell(0, 0);
    s.extendSelection(1, 0);
    expect(s.handleGridKey({ key: "Delete" })).toBe(true);
    expect([input(s, 0, 0), input(s, 1, 0)]).toEqual(["", ""]);
    expect(s.undoCellEdit()).toBe(true);
    expect([input(s, 0, 0), input(s, 1, 0)]).toEqual(["1", "2"]);
  });

  it("moving past the window scrolls the in-frame grid; scrollInFrame pans it", async () => {
    const { s, fake } = await open([[40, 0, "deep"]], { box: [0, 0, 60, 200] });
    await inFrame(s);
    s.selectCell(0, 0);
    for (let i = 0; i < 40; i++) s.handleGridKey({ key: "ArrowDown" });
    await tick();
    const texts = (l: SceneLayer) =>
      l.items.flatMap((i) => (i.kind === "text" ? [i.text] : []));
    expect(texts(fake.submits.at(-1)!)).toContain("deep");
    expect(s.scrollInFrame(-100, 0)).toBe(true); // clamps at row 1
    await tick();
    expect(texts(fake.submits.at(-1)!)).not.toContain("deep");
    expect(s.scrollInFrame(-1, 0)).toBe(false);
  });
});

describe.skipIf(!built)("copy / paste [sheet.edit.clipboard]", () => {
  it("our own copy pastes formulas re-addressed; one undo step", async () => {
    const { s, fake } = await open([
      [0, 0, "1"],
      [1, 0, "2"],
      [0, 1, "=A1*10"],
      [1, 1, "=A2*$A$1"],
    ]);
    s.selectCell(0, 1);
    s.extendSelection(1, 1);
    expect(await s.copySelection()).toEqual({ ok: true, rows: 2, cols: 1 });
    expect(fake.clip.payload?.text).toBe("10\n2");
    s.selectCell(3, 2);
    expect(await s.pasteAtSelection()).toEqual({ ok: true, rows: 2, cols: 1 });
    expect(input(s, 3, 2)).toBe("=B4*10");
    expect(input(s, 4, 2)).toBe("=B5*$A$1");
    expect(s.state().gridSelection).toEqual({ anchorRow: 3, anchorCol: 2, rows: 2, cols: 1 });
    expect(s.undoCellEdit()).toBe(true);
    expect([input(s, 3, 2), input(s, 4, 2)]).toEqual(["", ""]);
  });

  it("a one-cell copy tiles over a larger selection", async () => {
    const { s } = await open([
      [0, 0, "5"],
      [0, 1, "=A1+1"],
    ]);
    s.selectCell(0, 1);
    await s.copySelection();
    s.selectCell(1, 1);
    s.extendSelection(3, 1);
    expect(await s.pasteAtSelection()).toEqual({ ok: true, rows: 3, cols: 1 });
    expect([input(s, 1, 1), input(s, 2, 1), input(s, 3, 1)]).toEqual([
      "=A2+1",
      "=A3+1",
      "=A4+1",
    ]);
  });

  it("text from another app (TSV) pastes as values the engine types", async () => {
    const { s, fake } = await open([]);
    fake.clip.payload = { text: "Name\tQty\r\nApples\t12\r\n" };
    s.selectCell(0, 0);
    expect(await s.pasteAtSelection()).toEqual({ ok: true, rows: 2, cols: 2 });
    expect(input(s, 1, 1)).toBe("12");
    expect(display(s, 1, 0)).toBe("Apples");
  });
});

describe.skipIf(!built)("fill handle and fill down/right [sheet.edit.fill]", () => {
  it("dragging the fill handle in-frame continues a series; one undo step", async () => {
    const { s } = await open([
      [0, 0, "1"],
      [1, 0, "2"],
      [0, 1, "Mon"],
    ]);
    const at = await inFrame(s);
    s.selectCell(0, 0);
    s.extendSelection(1, 0);
    await tick();
    // The knob sits on the selection's bottom-right corner.
    const sc = s.gridScene(0, 0, 400, 200)!;
    const knob: [number, number] = [sc.viewport.xOffsets[1], sc.viewport.yOffsets[2]];
    expect(s.pointerDownInFrame(...knob)).toBe(true);
    expect(s.pointerMoveInFrame(...at(4, 0))).toBe(true);
    expect(s.pointerUpInFrame(...at(4, 0))).toBe(true);
    expect([2, 3, 4].map((r) => input(s, r, 0))).toEqual(["3", "4", "5"]);
    expect(s.state().gridSelection).toEqual({ anchorRow: 0, anchorCol: 0, rows: 5, cols: 1 });
    expect(s.undoCellEdit()).toBe(true);
    expect([2, 3, 4].map((r) => input(s, r, 0))).toEqual(["", "", ""]);
    // The weekday list through the session door.
    s.selectCell(0, 1);
    expect(s.fillSelectionTo({ anchorRow: 0, anchorCol: 1, rows: 3, cols: 1 }).ok).toBe(true);
    expect([input(s, 1, 1), input(s, 2, 1)]).toEqual(["Tue", "Wed"]);
  });

  it("Cmd+D copies the top row down (formulas re-addressed); Cmd+R copies right", async () => {
    const { s } = await open([
      [0, 0, "1"],
      [1, 0, "2"],
      [2, 0, "3"],
      [0, 1, "=A1*2"],
    ]);
    s.selectCell(0, 1);
    s.extendSelection(2, 1);
    expect(s.handleGridKey({ key: "d", metaKey: true })).toBe(true);
    expect([input(s, 1, 1), input(s, 2, 1)]).toEqual(["=A2*2", "=A3*2"]);
    expect(display(s, 2, 1)).toBe("6");
    // One cell: Cmd+R takes the cell to its left.
    s.selectCell(0, 2);
    expect(s.fillRight().ok).toBe(true);
    expect(input(s, 0, 2)).toBe("=B1*2");
    // Row 1 has nothing above it: refused, nothing written.
    s.selectCell(0, 3);
    expect(s.fillDown()).toEqual({ ok: false, message: "nothing above to fill from" });
  });
});

describe.skipIf(!built)("find in sheet [sheet.grid.find]", () => {
  it("find next selects the next match after the active cell, wraps, and scrolls in-frame", async () => {
    const { s, fake } = await open(
      [
        [0, 0, "apple"],
        [30, 1, "Apple pie"],
        [5, 0, "pear"],
      ],
      { box: [0, 0, 60, 200] },
    );
    await inFrame(s);
    s.selectCell(0, 0);
    expect(s.findNext("apple")).toMatchObject({ row: 30, col: 1 });
    expect(s.activeCell()).toEqual({ row: 30, col: 1 });
    await tick();
    const texts = fake.submits.at(-1)!.items.flatMap((i) => (i.kind === "text" ? [i.text] : []));
    expect(texts).toContain("Apple pie");
    expect(s.findAgain()).toMatchObject({ row: 0, col: 0 }); // wrapped
    expect(s.findNext("apple", undefined, true)).toMatchObject({ row: 30, col: 1 });
    expect(s.findNext("zzz")).toBeNull();
    // Cmd+F asks for the find field.
    const before = s.state().findRequest;
    expect(s.handleGridKey({ key: "f", metaKey: true })).toBe(true);
    expect(s.state().findRequest).toBe(before + 1);
  });
});

// ─────────────────────────────────────────────── the grid panel, real engine

function byData(tree: ReactTestRenderer, key: string) {
  return tree.root.findAll((n) => n.props != null && n.props[key] !== undefined);
}

function keyEvent(key: string, mods: { shiftKey?: boolean; metaKey?: boolean } = {}) {
  let stopped = false;
  const target = {};
  return {
    ev: {
      key,
      ...mods,
      target,
      currentTarget: target,
      preventDefault() {},
      stopPropagation() {
        stopped = true;
      },
    } as unknown as React.KeyboardEvent<HTMLDivElement>,
    stopped: () => stopped,
  };
}

describe.skipIf(!built)("grid panel keys, find and fill [sheet.grid.keyboard]", () => {
  it("the focused grid moves with arrows, types into the editor, and owns Cmd+Z", async () => {
    const { s, fake } = await open([[0, 0, "1"]]);
    let tree!: ReactTestRenderer;
    act(() => {
      tree = create(createElement(makeGridPanel(fake.host, s)));
    });
    s.selectCell(0, 0);
    const surface = () => byData(tree, "data-grid-surface")[0];
    act(() => surface().props.onKeyDown(keyEvent("ArrowDown", { shiftKey: true }).ev));
    expect(s.state().gridSelection).toEqual({ anchorRow: 0, anchorCol: 0, rows: 2, cols: 1 });
    // Typing opens the panel's cell editor on the ACTIVE cell.
    act(() => surface().props.onKeyDown(keyEvent("9").ev));
    const editor = byData(tree, "data-grid-editor")[0];
    expect(editor.props.value).toBe("9");
    act(() =>
      editor.props.onKeyDown({ key: "Enter", shiftKey: false, preventDefault() {} }),
    );
    expect(input(s, 0, 0)).toBe("9");
    // Cmd+Z on the focused grid is the sheet's undo, and stops there (the
    // host's document undo keybinding never sees it).
    const z = keyEvent("z", { metaKey: true });
    act(() => surface().props.onKeyDown(z.ev));
    expect(input(s, 0, 0)).toBe("1");
    expect(z.stopped()).toBe(true);
    act(() => tree.unmount());
  });

  it("the find field selects the next match; Delete clears from the panel", async () => {
    const { s, fake } = await open([
      [0, 0, "x"],
      [4, 2, "needle"],
    ]);
    let tree!: ReactTestRenderer;
    act(() => {
      tree = create(createElement(makeGridPanel(fake.host, s)));
    });
    s.selectCell(0, 0);
    const field = () => byData(tree, "data-find-input")[0];
    act(() => field().props.onChange({ target: { value: "need" } }));
    act(() => field().props.onKeyDown({ key: "Enter", shiftKey: false, preventDefault() {} }));
    expect(s.activeCell()).toEqual({ row: 4, col: 2 });
    expect(JSON.stringify(byData(tree, "data-find-status")[0].props.children)).toContain("C5");
    act(() => byData(tree, "data-grid-surface")[0].props.onKeyDown(keyEvent("Delete").ev));
    expect(input(s, 4, 2)).toBe("");
    act(() => tree.unmount());
  });
});
