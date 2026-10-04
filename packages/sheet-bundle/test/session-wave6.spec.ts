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

// Wave 6 of the paged.sheet campaign — formatting & layout through the
// SESSION and the workbook panel over the REAL engine and a fake document
// host that records every write: cell styles (and what a placed table gets
// from them), borders, merges, sizes, the freeze toggle, defined names
// (define, place by name, delete), repeated header rows when paginating,
// and persistence of all of it through the xlsx bytes.
// Same dual gate as engine-real.spec.ts: skipped without the artifact,
// FAILS under REQUIRE_REAL_ENGINE=1.

import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { createElement } from "react";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { describe, expect, it } from "vitest";

import type {
  BundleHost,
  ElementId,
  FrameChainLink,
  Mutation,
  MutationOutcome,
} from "@paged-media/plugin-api";
import { parseBinding } from "@paged-media/sheet-host-model";

import { makeWorkbookPanel } from "../src/panels/workbook-panel";
import { createWorkbookSession, type WorkbookSession } from "../src/session";

const HERE = dirname(fileURLToPath(import.meta.url));
const WASM = join(HERE, "..", "bin", "sheet_js_bg.wasm");
const built = existsSync(WASM);

if (process.env.REQUIRE_REAL_ENGINE === "1" && !built) {
  describe("session wave 6 (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing`);
    });
  });
}

/** A fake document: frames mint stories, tables mint ids, a selection and
 *  its geometry, a frame chain, a scene tree that grows with each batch. */
function fakeDoc(opts: {
  selection?: ElementId[];
  selectionGeom?: { bounds: [number, number, number, number]; t?: number[] };
  chain?: { storyId: string; links: FrameChainLink[] };
} = {}) {
  const mutations: Mutation[] = [];
  const stories: string[] = opts.chain ? [opts.chain.storyId] : [];
  const frameOfStory = new Map<string, string>();
  const tree: { id: ElementId; kind: string; label: string }[] = [];
  let seq = 0;
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: () => false,
    document: {
      async meta() {
        return { activePage: "Page/u1" } as never;
      },
      async collection(name: string) {
        if (name === "stories") return stories.map((selfId) => ({ selfId })) as never;
        return [] as never;
      },
      async tree() {
        return [{ kind: "page", label: "p", children: [...tree] }] as never;
      },
      async frameChain(storyId: string) {
        if (opts.chain && storyId === opts.chain.storyId) return opts.chain.links;
        const frame = frameOfStory.get(storyId);
        return frame ? ([{ frameId: frame, next: null, overflow: false }] as never) : [];
      },
      async elementGeometry(ids: ElementId[]) {
        return ids.map((id) => ({
          id,
          pageId: "Page/u1",
          bounds: opts.selectionGeom?.bounds ?? [0, 0, 300, 200],
          itemTransform: opts.selectionGeom?.t ?? null,
        })) as never;
      },
      async mutate(m: Mutation): Promise<MutationOutcome> {
        mutations.push(m);
        const created = (kind: string): ElementId => {
          seq += 1;
          const id = { kind, id: `u${seq}` } as ElementId;
          tree.push({ id, kind, label: id.id as string });
          return id;
        };
        if (m.op === "batch") {
          let first: ElementId | null = null;
          // C-15 handles: `$h:NAME` in a storyId position is the story the
          // bound frame insert minted; the reply names the LAST mint when
          // the batch binds handles (core's contract), else the first.
          const handles = new Map<string, string>();
          let lastStory: string | null = null;
          let last: ElementId | null = null;
          for (const o of m.args.ops) {
            if (o.op === "insertTextFrame") {
              const id = created("textFrame");
              first ??= id;
              last = id;
              lastStory = `Story/s${seq}`;
              stories.push(lastStory);
              frameOfStory.set(lastStory, id.id as string);
            } else if (o.op === "insertPath") {
              first ??= created("polygon");
            } else if (o.op === "bindCreated" && lastStory) {
              handles.set(`$h:${o.args.handle}`, lastStory);
            } else if (o.op === "insertTable") {
              seq += 1;
              last = {
                kind: "table",
                id: { story_id: handles.get(o.args.storyId) ?? o.args.storyId, table_id: `t${seq}` },
              } as ElementId;
            }
          }
          return {
            applied: true,
            createdId: handles.size > 0 ? last : first,
            pageIds: ["Page/u1"],
          };
        }
        if (m.op === "insertTable") {
          seq += 1;
          return {
            applied: true,
            createdId: {
              kind: "table",
              id: { story_id: m.args.storyId, table_id: `t${seq}` },
            } as ElementId,
            pageIds: ["Page/u1"],
          };
        }
        return { applied: true, createdId: null, pageIds: ["Page/u1"] };
      },
      onDidChange() {
        return { dispose() {} };
      },
    },
    text: {
      async measureString() {
        return { advance: 30, ascender: 9, descender: -2 };
      },
    },
    selection: {
      get: () => opts.selection ?? [],
      async set(ids: ElementId[]) {
        return ids;
      },
    },
    shell: { openPanel() {} },
  } as unknown as BundleHost;
  return { host, mutations };
}

type Loose = { op: string; args: Record<string, unknown> & { path?: string; value?: unknown } };
const ops = (ms: Mutation[]): Loose[] =>
  ms.flatMap((m) => (m.op === "batch" ? m.args.ops : [m])) as unknown as Loose[];
const props = (ms: Mutation[], path: string) =>
  ops(ms).filter((o) => o.op === "setElementProperty" && o.args.path === path);

/** A blank workbook with a 3×2 block: a header row and two data rows. */
async function block(host: BundleHost): Promise<WorkbookSession> {
  const session = createWorkbookSession(host);
  await session.newWorkbook();
  session.editCell(0, 0, 0, "Region");
  session.editCell(0, 0, 1, "Units");
  session.editCell(0, 1, 0, "North");
  session.editCell(0, 1, 1, "1.5");
  session.editCell(0, 2, 0, "South");
  session.editCell(0, 2, 1, "2");
  return session;
}

describe.skipIf(!built)("cell styles through the session [sheet.format.cell-style]", () => {
  it("styles the grid selection and the placed table carries every facet", async () => {
    const { host, mutations } = fakeDoc();
    const session = await block(host);
    session.setGridSelection(0, 0, 1, 2); // A1:B1
    expect(session.formatTarget()).toMatchObject({ sheet: 0, range: "A1:B1", top: 0, right: 1 });
    expect(
      session.setStyle({
        bold: true,
        underline: true,
        fill: "#FFEE00",
        vAlign: "center",
        borderBottom: { style: "medium", color: "#FF0000" },
      }),
    ).toEqual({ ok: true });
    expect(session.styleAtTarget()).toMatchObject({
      bold: true,
      underline: true,
      fill: "#FFEE00",
      vAlign: "center",
      borderBottom: { style: "medium", color: "#FF0000" },
    });
    session.setGridSelection(1, 1, 2, 1); // B2:B3
    expect(session.setStyle({ numFmt: "0.00" })).toEqual({ ok: true });
    expect(session.state().engine!.getCellDisplay(0, 1, 1)).toBe("1.50");
    expect(session.setStyle({ fill: "not a colour" }).ok).toBe(false);

    session.setRange("A1:B3");
    expect(await session.lowerSelection()).not.toBeNull();
    expect(props(mutations, "cellFillColor").length).toBe(2);
    expect(props(mutations, "cellVerticalJustification")[0].args.value).toEqual({
      type: "text",
      value: "CenterAlign",
    });
    const weights = props(mutations, "cellBottomEdgeStrokeWeight").map(
      (o) => (o.args.value as { value: number }).value,
    );
    expect(weights).toContain(1.5);
    expect(props(mutations, "cellBottomEdgeStrokeColor")[0].args.value).toEqual({
      type: "colorRef",
      value: "Color/uPagedSheetCellStrokeFF0000",
    });
    expect(
      ops(mutations).some(
        (o) => o.op === "createSwatch" && JSON.stringify(o.args).includes("CellStrokeFF0000"),
      ),
    ).toBe(true);
    expect(
      ops(mutations).some(
        (o) => o.op === "setStyleProperty" && o.args.path === "characterUnderline",
      ),
    ).toBe(true);
    session.dispose();
  });

  it("borders: outline draws the block's edges only", async () => {
    const { host } = fakeDoc();
    const session = await block(host);
    session.setGridSelection(0, 0, 3, 2); // A1:B3
    expect(session.setBorders("outline", { style: "thin" })).toEqual({ ok: true });
    const e = session.state().engine!;
    const a1 = e.getStyle!(0, 0, 0);
    const b3 = e.getStyle!(0, 2, 1);
    const a2 = e.getStyle!(0, 1, 0);
    expect([a1.borderTop?.style, a1.borderLeft?.style, a1.borderBottom?.style]).toEqual([
      "thin",
      "thin",
      "none",
    ]);
    expect([b3.borderBottom?.style, b3.borderRight?.style]).toEqual(["thin", "thin"]);
    expect([a2.borderTop?.style, a2.borderLeft?.style, a2.borderRight?.style]).toEqual([
      "none",
      "thin",
      "none",
    ]);
    expect(session.setBorders("none", { style: "thin" })).toEqual({ ok: true });
    expect(e.getStyle!(0, 0, 0).borderTop?.style).toBe("none");
    session.dispose();
  });

  it("styles, merges, sizes and the freeze survive the xlsx bytes", async () => {
    const { host } = fakeDoc();
    const session = await block(host);
    session.setGridSelection(0, 0, 1, 2);
    session.setStyle({ bold: true, fill: "#DDEEFF", numFmt: "#,##0.00" });
    session.mergeTarget();
    session.setColumnWidth(20);
    session.setRowHeight(30);
    session.setGridSelection(1, 1, 1, 1); // B2: freeze row 1, col A
    expect(session.toggleFreeze()).toEqual({ ok: true });
    const saved = session.saveWorkbook()!;
    session.dispose();

    const { host: host2 } = fakeDoc();
    const again = createWorkbookSession(host2);
    await again.import(saved.bytes, saved.fileName);
    const e = again.state().engine!;
    expect(e.getStyle!(0, 0, 0)).toMatchObject({ bold: true, fill: "#DDEEFF", numFmt: "#,##0.00" });
    expect(again.layout()).toMatchObject({
      merges: ["A1:B1"],
      colWidths: [
        [0, 20],
        [1, 20],
      ],
      rowHeights: [[0, 30]],
      freezeRows: 1,
      freezeCols: 1,
    });
    again.dispose();
  });
});

describe.skipIf(!built)("merge / unmerge [sheet.layout.merge]", () => {
  it("merges the target, the placed table spans it, unmerge removes it", async () => {
    const { host, mutations } = fakeDoc();
    const session = await block(host);
    session.setGridSelection(0, 0, 1, 2); // A1:B1
    expect(session.mergeTarget()).toEqual({ ok: true });
    expect(session.state().engine!.getCellDisplay(0, 0, 1)).toBe("");
    expect(session.layout()!.merges).toEqual(["A1:B1"]);
    expect(session.mergeTarget().ok).toBe(false); // overlaps itself
    session.setRange("A1:B3");
    await session.lowerSelection();
    expect(ops(mutations).some((o) => o.op === "setCellSpan")).toBe(true);
    expect(session.unmergeTarget()).toEqual({ ok: true });
    expect(session.layout()!.merges).toEqual([]);
    expect(session.unmergeTarget().ok).toBe(false);
    session.dispose();
  });
});

describe.skipIf(!built)("column width / row height [sheet.layout.sizes]", () => {
  it("sets and resets sizes over the target's columns and rows", async () => {
    const { host } = fakeDoc();
    const session = await block(host);
    session.setGridSelection(1, 1, 2, 1); // B2:B3
    expect(session.setColumnWidth(12)).toEqual({ ok: true });
    expect(session.setRowHeight(24)).toEqual({ ok: true });
    expect(session.layout()).toMatchObject({
      colWidths: [[1, 12]],
      rowHeights: [
        [1, 24],
        [2, 24],
      ],
    });
    expect(session.setColumnWidth(999).ok).toBe(false);
    expect(session.setColumnWidth(null)).toEqual({ ok: true });
    expect(session.layout()!.colWidths).toEqual([]);
    session.dispose();
  });
});

describe.skipIf(!built)("freeze toggle [sheet.layout.freeze]", () => {
  it("freezes above/left of the selection, then unfreezes", async () => {
    const { host } = fakeDoc();
    const session = await block(host);
    expect(session.toggleFreeze()).toEqual({ ok: true }); // no selection: first row
    expect(session.layout()).toMatchObject({ freezeRows: 1, freezeCols: 0 });
    expect(session.state().engine!.listFreezePanes()).toEqual([{ sheet: 0, rows: 1, cols: 0 }]);
    expect(session.toggleFreeze()).toEqual({ ok: true });
    expect(session.layout()).toMatchObject({ freezeRows: 0, freezeCols: 0 });
    session.setGridSelection(2, 1, 1, 1); // B3
    session.toggleFreeze();
    expect(session.layout()).toMatchObject({ freezeRows: 2, freezeCols: 1 });
    session.dispose();
  });
});

describe.skipIf(!built)("defined names [sheet.names.define]", () => {
  it("define, place by name (the binding keeps the name), delete", async () => {
    const { host, mutations } = fakeDoc();
    const session = await block(host);
    expect(session.defineName("Sales", "A1:B3")).toEqual({ ok: true });
    expect(session.defineName("1bad", "A1").ok).toBe(false);
    expect(session.names()).toEqual([{ name: "Sales", refersTo: "Sheet1!$A$1:$B$3" }]);
    const frame = await session.placeName("Sales");
    expect(frame).not.toBeNull();
    const stamp = ops(mutations).find((o) => o.op === "setPluginMetadata");
    const binding = parseBinding(JSON.parse((stamp!.args as { value: string }).value));
    expect(binding?.data.range).toBe("Sales");
    expect(
      ops(mutations).filter((o) => o.op === "insertText").map((o) => (o.args as { text: string }).text),
    ).toContain("South");
    session.editCell(0, 4, 0, "=ROWS(Sales)");
    expect(session.state().engine!.getCellDisplay(0, 4, 0)).toBe("3");
    expect(session.deleteName("Sales")).toEqual({ ok: true });
    expect(session.names()).toEqual([]);
    expect(session.state().engine!.getCellDisplay(0, 4, 0)).toBe("#NAME?");
    expect(await session.placeName("Sales")).toBeNull();
    session.dispose();
  });
});

describe.skipIf(!built)("repeated header rows when paginating [sheet.lower.paginate]", () => {
  it("every frame of the chain starts with the header row", async () => {
    const { host, mutations } = fakeDoc({
      selection: [{ kind: "textFrame", id: "f1" }],
      selectionGeom: { bounds: [0, 0, 60, 200] }, // 4 rows of 15 pt per frame
      chain: {
        storyId: "Story/chain",
        links: [
          { frameId: "f0", next: "f1", overflow: false },
          { frameId: "f1", next: "f2", overflow: false },
          { frameId: "f2", next: null, overflow: false },
        ],
      },
    });
    const session = createWorkbookSession(host);
    await session.newWorkbook();
    session.editCell(0, 0, 0, "Head");
    for (let r = 1; r < 10; r++) session.editCell(0, r, 0, `r${r}`);
    session.setRange("A1:A10");
    expect(await session.paginateSelection({ repeatHeaderRows: 1 })).toEqual({ ok: true });
    const rowZero = new Map<string, string>();
    for (const o of ops(mutations)) {
      if (o.op !== "insertText") continue;
      const a = o.args as { text: string; cell?: { tableId: string; row: number } };
      if (a.cell && a.cell.row === 0) rowZero.set(a.cell.tableId, a.text);
    }
    expect(rowZero.size).toBeGreaterThanOrEqual(3);
    expect([...rowZero.values()].every((t) => t === "Head")).toBe(true);
    session.dispose();
  });
});

describe.skipIf(!built)("the workbook panel reaches the Wave 6 verbs [sheet.format.cell-style]", () => {
  const byData = (tree: ReactTestRenderer, key: string, value?: string) =>
    tree.root.findAll(
      (n) =>
        n.props != null &&
        n.props[key] !== undefined &&
        (value === undefined || n.props[key] === value) &&
        typeof n.type === "string",
    );

  it("format, layout and name controls drive the session and report", async () => {
    const { host } = fakeDoc();
    (host as { supports: (f: string) => boolean }).supports = () => false;
    const session = await block(host);
    const Panel = makeWorkbookPanel(host, session);
    let tree!: ReactTestRenderer;
    act(() => {
      tree = create(createElement(Panel));
    });
    act(() => session.setGridSelection(0, 0, 1, 2));
    act(() => byData(tree, "data-sheet-fmt-bold")[0].props.onClick());
    expect(session.state().engine!.getStyle!(0, 0, 1).bold).toBe(true);
    act(() => byData(tree, "data-sheet-fmt-numfmt")[0].props.onChange({ target: { value: "0%" } }));
    act(() => byData(tree, "data-sheet-fmt-numfmt-apply")[0].props.onClick());
    expect(session.state().engine!.getStyle!(0, 0, 0).numFmt).toBe("0%");
    act(() => byData(tree, "data-sheet-fmt-border", "outline")[0].props.onClick());
    expect(session.state().engine!.getStyle!(0, 0, 0).borderTop?.style).toBe("thin");
    act(() => byData(tree, "data-sheet-merge")[0].props.onClick());
    expect(session.layout()!.merges).toEqual(["A1:B1"]);
    act(() => byData(tree, "data-sheet-freeze")[0].props.onClick());
    expect(session.layout()!.freezeRows).toBe(0); // anchor row 0, col 0 → refused
    const msg = () =>
      JSON.stringify(byData(tree, "data-sheet-format-msg")[0]?.children ?? []);
    expect(msg()).toContain("select the cell below");
    act(() => byData(tree, "data-sheet-name-new")[0].props.onChange({ target: { value: "Top" } }));
    act(() => byData(tree, "data-sheet-name-define")[0].props.onClick());
    expect(session.names().map((n) => n.name)).toEqual(["Top"]);
    expect(byData(tree, "data-sheet-name-place", "Top")).toHaveLength(1);
    expect(byData(tree, "data-sheet-paginate-header-rows")).toHaveLength(1);
    act(() => tree.unmount());
    session.dispose();
  });
});
