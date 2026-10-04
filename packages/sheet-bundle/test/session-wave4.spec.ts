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

// Wave 4 of the paged.sheet campaign, through the SESSION over the REAL
// engine and a fake document host that records every write:
//   · the placed table uses the styled page door (fills reach the page);
//   · placed tables refresh IN PLACE after an edit (and their binding
//     carries the workbook's content version, which was always 0);
//   · placement lands at the current selection;
//   · "Paginate into threaded frames" reaches the chain lowering;
//   · NOW/TODAY follow the host clock;
//   · a blank workbook, add/rename/delete sheet, rows/cols, iteration;
//   · CSV import.
// Same dual gate as engine-real.spec.ts: skipped without the artifact,
// FAILS under REQUIRE_REAL_ENGINE=1.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  BundleHost,
  ElementId,
  FrameChainLink,
  Mutation,
  MutationOutcome,
} from "@paged-media/plugin-api";
import { parseBinding } from "@paged-media/sheet-host-model";

import { createElement } from "react";
import { act, create, type ReactTestRenderer } from "react-test-renderer";

import { bootEmptyEngine, wrapEngine, type SheetWasmEngine } from "../src/engine";
import { makeWorkbookPanel } from "../src/panels/workbook-panel";
import { createWorkbookSession, REFRESH_DEBOUNCE_MS } from "../src/session";

const HERE = dirname(fileURLToPath(import.meta.url));
const WASM = join(HERE, "..", "bin", "sheet_js_bg.wasm");
const built = existsSync(WASM);
const corpus = (name: string) =>
  new Uint8Array(readFileSync(join(HERE, "..", "..", "..", "corpus/xlsx-corpus", name)));

if (process.env.REQUIRE_REAL_ENGINE === "1" && !built) {
  describe("session wave 4 (wasm artifact) — REQUIRED", () => {
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
        return opts.chain && storyId === opts.chain.storyId ? opts.chain.links : [];
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
          for (const o of m.args.ops) {
            if (o.op === "insertTextFrame") {
              const id = created("textFrame");
              first ??= id;
              stories.push(`Story/s${seq}`);
            } else if (o.op === "insertPath") {
              first ??= created("polygon");
            }
          }
          return { applied: true, createdId: first, pageIds: ["Page/u1"] };
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

const ops = (ms: Mutation[]): Mutation[] =>
  ms.flatMap((m) => (m.op === "batch" ? m.args.ops : [m]));

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe.skipIf(!built)("placed tables: styled door + refresh in place [sheet.lower.page]", () => {
  it("the placed table carries the workbook's conditional fills", async () => {
    const { host, mutations } = fakeDoc();
    const session = createWorkbookSession(host);
    await session.import(corpus("08-condfmt.xlsx"), "08-condfmt.xlsx");
    session.setRange("A1:E5");
    expect(await session.lowerSelection()).not.toBeNull();
    const fills = ops(mutations).filter(
      (m) => m.op === "setElementProperty" && m.args.path === "cellFillColor",
    );
    // Column A rows 1/3/5 match `> 5` → the dxf yellow (before Wave 4 the
    // page read the key-0 door and NO fill reached the document).
    expect(fills.length).toBeGreaterThanOrEqual(3);
    expect(
      ops(mutations).some((m) => m.op === "createSwatch"),
    ).toBe(true);
    session.dispose();
  });

  it("an edit re-pours only the changed cell and re-stamps the binding version", async () => {
    vi.useFakeTimers();
    const { host, mutations } = fakeDoc();
    const session = createWorkbookSession(host);
    await session.newWorkbook();
    session.editCell(0, 0, 0, "1");
    session.editCell(0, 1, 0, "2");
    session.editCell(0, 2, 0, "=A1+A2");
    session.setRange("A1:A3");
    const frameId = await session.lowerSelection();
    expect(frameId).not.toBeNull();
    const tablesBefore = mutations.filter((m) => m.op === "insertTable").length;
    const mark = mutations.length;

    session.editCell(0, 0, 0, "10");
    await vi.advanceTimersByTimeAsync(REFRESH_DEBOUNCE_MS + 50);
    await session.refreshPlacements();

    const after = mutations.slice(mark);
    expect(after.filter((m) => m.op === "insertTable")).toHaveLength(0);
    expect(mutations.filter((m) => m.op === "insertTable")).toHaveLength(tablesBefore);
    const texts = after.filter((m) => m.op === "insertText") as Array<{
      args: { text: string; cell: { row: number } };
    }>;
    // A1 → 10 and A3 (=A1+A2) → 12; A2 untouched.
    expect(texts.map((t) => [t.args.cell.row, t.args.text])).toEqual([
      [0, "10"],
      [2, "12"],
    ]);
    const stamp = after.find((m) => m.op === "setPluginMetadata") as
      | { args: { value: string } }
      | undefined;
    const binding = parseBinding(JSON.parse(stamp!.args.value));
    expect(binding?.data.contentVersion).toBe(session.contentVersion());
    expect(session.contentVersion()).toBeGreaterThan(0);
    session.dispose();
  });

  it("lands at the current selection, not 24 pt from the page origin", async () => {
    const { host, mutations } = fakeDoc({
      selection: [{ kind: "textFrame", id: "sel" }],
      selectionGeom: { bounds: [100, 50, 200, 250], t: [1, 0, 0, 1, 10, 20] },
    });
    const session = createWorkbookSession(host);
    await session.newWorkbook();
    session.editCell(0, 0, 0, "x");
    session.setRange("A1");
    await session.lowerSelection();
    const frame = ops(mutations).find((m) => m.op === "insertTextFrame") as {
      args: { bounds: number[] };
    };
    expect(frame.args.bounds.slice(0, 2)).toEqual([120, 60]);
    session.dispose();
  });
});

describe.skipIf(!built)("placed charts refresh after edits [sheet.chart.engine]", () => {
  it("an edit replaces the chart's elements (deleted, re-lowered in place)", async () => {
    vi.useFakeTimers();
    const { host, mutations } = fakeDoc();
    const session = createWorkbookSession(host);
    await session.import(corpus("09-chart.xlsx"), "09-chart.xlsx");
    const charts = session.listCharts();
    expect(charts.length).toBeGreaterThan(0);
    expect(await session.lowerChart(charts[0].index)).toBe(true);
    const firstBatch = mutations.length;
    const created = ops(mutations).filter((m) => m.op === "insertPath").length;
    expect(created).toBeGreaterThan(0);

    // An edit OUTSIDE the chart's series redraws nothing.
    session.editCell(0, 49, 25, "unrelated");
    await vi.advanceTimersByTimeAsync(REFRESH_DEBOUNCE_MS + 50);
    await session.refreshPlacements();
    expect(ops(mutations.slice(firstBatch)).filter((m) => m.op === "deleteFrame")).toHaveLength(0);

    // An edit to a series value (B2) replaces the chart.
    session.editCell(0, 1, 1, "999");
    await vi.advanceTimersByTimeAsync(REFRESH_DEBOUNCE_MS + 50);
    await session.refreshPlacements();

    const after = mutations.slice(firstBatch);
    const deletes = ops(after).filter((m) => m.op === "deleteFrame");
    expect(deletes.length).toBeGreaterThanOrEqual(created);
    expect(ops(after).filter((m) => m.op === "insertPath").length).toBe(created);
    session.dispose();
  });
});

describe.skipIf(!built)("paginate into threaded frames [sheet.lower.paginate]", () => {
  it("finds the selected frame's chain and lowers into the chain's story", async () => {
    const { host, mutations } = fakeDoc({
      selection: [{ kind: "textFrame", id: "f1" }],
      chain: {
        storyId: "Story/chain",
        links: [
          { frameId: "f0", next: "f1", overflow: false },
          { frameId: "f1", next: null, overflow: false },
        ],
      },
    });
    const session = createWorkbookSession(host);
    await session.newWorkbook();
    for (let r = 0; r < 4; r++) session.editCell(0, r, 0, `r${r}`);
    session.setRange("A1:A4");
    const r = await session.paginateSelection();
    expect(r).toEqual({ ok: true });
    const inserts = mutations.filter((m) => m.op === "insertTable") as Array<{
      args: { storyId: string };
    }>;
    expect(inserts.length).toBeGreaterThanOrEqual(1);
    expect(inserts.every((m) => m.args.storyId === "Story/chain")).toBe(true);
    session.dispose();
  });
});

describe.skipIf(!built)("NOW/TODAY follow the host clock [sheet.calc.engine]", () => {
  it("TODAY() is the host's local date, not serial 0", async () => {
    // Initialise the wasm module (the boot path does), then build an engine
    // over it with a pinned clock.
    (await bootEmptyEngine()).dispose();
    // A fresh engine on a pinned clock: 2026-10-04T12:00Z, UTC.
    const mod = (await import("../bin/sheet_js.js")) as unknown as {
      SheetEngine: new () => SheetWasmEngine;
    };
    const engine = wrapEngine(new mod.SheetEngine(), {
      nowMs: () => Date.UTC(2026, 9, 4, 12),
      tzOffsetMin: () => 0,
    });
    engine.setCell(0, 0, 0, "=TODAY()");
    expect(engine.getCellDisplay(0, 0, 0)).toBe("46299");
    engine.dispose();
  });
});

describe.skipIf(!built)("workbook structure [sheet.workbook.sheets]", () => {
  it("new blank workbook; add, rename and delete sheets", async () => {
    const { host } = fakeDoc();
    const session = createWorkbookSession(host);
    await session.newWorkbook();
    expect(session.state().fileName).toBe("Book1.xlsx");
    expect(session.addSheet()).toEqual({ ok: true });
    expect(session.state().activeSheet).toBe(1);
    expect(session.renameSheet(1, "Data")).toEqual({ ok: true });
    expect(session.renameSheet(1, "bad/name").ok).toBe(false);
    session.editCell(0, 0, 0, "=Data!A1+1");
    expect(session.state().engine!.getCellInput(0, 0, 0)).toBe("=Data!A1+1");
    expect(session.deleteSheet(1)).toEqual({ ok: true });
    expect(session.state().engine!.listSheets().map((s) => s.name)).toEqual(["Sheet1"]);
    expect(session.state().engine!.getCellDisplay(0, 0, 0)).toBe("#REF!");
    session.dispose();
  });
});

describe.skipIf(!built)("insert/delete rows and columns [sheet.calc.rewrite.structural-door]", () => {
  it("inserts at the grid selection and rewrites references", async () => {
    const { host } = fakeDoc();
    const session = createWorkbookSession(host);
    await session.newWorkbook();
    for (const [r, v] of [[0, "1"], [1, "2"], [2, "3"]] as const) session.editCell(0, r, 0, v);
    session.editCell(0, 0, 1, "=SUM(A1:A3)");
    expect(session.structuralEdit("insertRows").ok).toBe(false); // no selection
    session.setGridSelection(1, 0, 2, 1);
    expect(session.structuralEdit("insertRows")).toEqual({ ok: true });
    const e = session.state().engine!;
    expect(e.getCellInput(0, 0, 1)).toBe("=SUM(A1:A5)");
    expect(e.getCellDisplay(0, 4, 0)).toBe("3");
    session.setGridSelection(0, 0, 1, 1);
    expect(session.structuralEdit("deleteCols")).toEqual({ ok: true });
    expect(e.getCellDisplay(0, 0, 0)).toBe("#REF!");
    session.dispose();
  });

  it("refuses on a sheet whose preserved content addresses its cells", async () => {
    const { host } = fakeDoc();
    const session = createWorkbookSession(host);
    await session.import(corpus("12-datavalidation.xlsx"), "dv.xlsx");
    session.setGridSelection(0, 0, 1, 1);
    const r = session.structuralEdit("insertRows");
    expect(r.ok).toBe(false);
    expect(!r.ok && r.message).toContain("dataValidations");
    session.dispose();
  });

  it("iteration toggles through the session [sheet.calc.iterative]", async () => {
    const { host } = fakeDoc();
    const session = createWorkbookSession(host);
    await session.newWorkbook();
    expect(session.calcSettings()?.iterative).toBe(false);
    session.editCell(0, 0, 0, "=B1/2+1");
    session.editCell(0, 0, 1, "=A1");
    expect(session.setIterative(true)).toEqual({ ok: true });
    expect(session.calcSettings()?.iterative).toBe(true);
    expect(Number(session.state().engine!.getCellDisplay(0, 0, 0))).toBeCloseTo(2, 2);
    session.dispose();
  });
});

describe.skipIf(!built)("CSV / TSV import [sheet.import.csv]", () => {
  it("a German CSV becomes a typed one-sheet workbook", async () => {
    vi.stubGlobal("navigator", { language: "de-DE" });
    const { host } = fakeDoc();
    const session = createWorkbookSession(host);
    await session.importCsv("Region;Umsatz;Datum\nNord;1.234,50;04.10.2026\n", "umsatz.csv");
    const e = session.state().engine!;
    expect(session.state().fileName).toBe("umsatz.xlsx");
    expect(e.listSheets()[0].name).toBe("umsatz");
    expect(e.getCellInput(0, 1, 1)).toBe("1234.5");
    expect(e.getCellDisplay(0, 1, 2)).toBe("04.10.2026");
    session.dispose();
  });

  it("a .tsv splits on tabs", async () => {
    const { host } = fakeDoc();
    const session = createWorkbookSession(host);
    await session.importCsv("a\tb\n1\t2\n", "t.tsv");
    expect(session.state().engine!.getCellInput(0, 1, 1)).toBe("2");
    session.dispose();
  });
});

describe.skipIf(!built)("the workbook panel reaches the Wave 4 verbs [sheet.workbook.sheets]", () => {
  const byData = (tree: ReactTestRenderer, key: string) =>
    tree.root.findAll((n) => n.props != null && n.props[key] !== undefined && n.type === "button");

  it("New blank / Add sheet / rows buttons drive the session and report", async () => {
    const { host } = fakeDoc();
    (host as { supports: (f: string) => boolean }).supports = () => false;
    const session = createWorkbookSession(host);
    const Panel = makeWorkbookPanel(host, session);
    let tree!: ReactTestRenderer;
    act(() => {
      tree = create(createElement(Panel));
    });
    await act(async () => {
      byData(tree, "data-sheet-new")[0].props.onClick();
      for (let i = 0; i < 100 && session.state().fileName === null; i++) {
        await new Promise((r) => setTimeout(r, 5));
      }
    });
    expect(session.state().fileName).toBe("Book1.xlsx");
    act(() => byData(tree, "data-sheet-add")[0].props.onClick());
    expect(session.state().engine!.listSheets()).toHaveLength(2);
    // No grid selection: the verb says why instead of doing nothing.
    act(() => byData(tree, "data-sheet-insert-rows")[0].props.onClick());
    const msg = tree.root.findAll((n) => n.props?.["data-sheet-struct-msg"] !== undefined);
    expect(JSON.stringify(msg[0].children)).toContain("select the rows or columns");
    expect(byData(tree, "data-sheet-paginate")).toHaveLength(1);
    act(() => tree.unmount());
    session.dispose();
  });
});
