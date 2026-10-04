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

// PERF BUDGETS — paged.sheet's work counted at the host doors and the
// engine boundary (campaign Wave 1).
//
// THE RULES, and they are the whole point of this file:
//
//  1. A budget is a COUNT, never a duration.
//  2. A budget is the MEASURED value, pinned exactly — the bad ones
//     included, with the number it should become written beside it.
//  3. A budget is only ever LOWERED, in the same commit as the change that
//     earns it. One that fails UPWARD means the change made the bundle do
//     more work; raising the number is not the fix.
//  4. Every scenario sits beside a behaviour assertion on the same gesture,
//     and no budget stands on a rejected write (`rejected` is pinned 0).
//
// The harness: a REAL headless host (core 0.64 via `@paged-media/canvas-
// wasm`), the REAL sheet engine (bin/sheet_js_bg.wasm), the session the
// bundle runs, and `countingHost` / `countingEngine` (./counting-host.ts)
// counting every door call and every engine call into one tally. The one
// stand-in is the scene channel the headless host does not wire
// (`withSceneChannel`): it records what the in-frame grid submits.
//
// `PERF_SHOW=1 pnpm vitest run test/perf` prints every scenario's full
// work log — how a budget is found, and how a failing one is read.
//
// Real engine: skipped without the wasm artifact, FAILS under
// REQUIRE_REAL_ENGINE=1 (the engine-real.spec.ts dual gate).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createElement } from "react";
import { act, create, type ReactTestInstance, type ReactTestRenderer } from "react-test-renderer";

import type { BundleHost } from "@paged-media/plugin-api";
import type { HeadlessHost } from "@paged-media/plugin-sdk";

import {
  CHAIN_REFLOW_DEBOUNCE_MS,
  createWorkbookSession,
  lowerPaginatedToChain,
  makeGridPanel,
  makeWorkbookPanel,
  subscribeChainReflow,
  subscribeProviderInvalidation,
  type SheetEngine,
  type WorkbookSession,
} from "../../src";
import {
  BUDGET_TIMEOUT_MS,
  SHARED,
  countingHost,
  report,
  type WorkLog,
} from "./counting-host";
import { engineCounters, resetEngineCounters } from "./perf-engine";
import {
  ENGINE_BUILT,
  WASM,
  authorWorkbook,
  blankPageIdml,
  exportedTableCells,
  openHost,
  settle,
  sheetHost,
  testClipboard,
  withSceneChannel,
  type TestClipboard,
} from "./workload";

// Every engine the session boots is wrapped, so its calls land in SHARED
// as `engine.<method>`. With PERF_ENGINE=1 (and scripts/build-wasm-perf.sh
// run) it is the COUNTING engine, and PERF_SHOW lines carry its own work
// counts under "engine" — trended, not budgeted.
vi.mock("../../src/engine", async (importOriginal) => {
  const orig = await importOriginal<typeof import("../../src/engine")>();
  const { countingEngine, SHARED: tally } = await import("./counting-host");
  const { perfBoot } = await import("./perf-engine");
  const boot = async (fallback: () => Promise<SheetEngine>) =>
    countingEngine((await perfBoot(orig.wrapEngine)) ?? (await fallback()), tally);
  return {
    ...orig,
    bootEngine: () => boot(orig.bootEngine),
    bootEmptyEngine: () => boot(orig.bootEmptyEngine),
  };
});

if (process.env.REQUIRE_REAL_ENGINE === "1" && !ENGINE_BUILT) {
  describe("perf budgets (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing — run scripts/build-wasm.sh`);
    });
  });
}

/** Forget everything counted so far — doors, engine calls, and the
 *  counting engine's own counters when it is in use. */
function resetWork(): void {
  SHARED.work.reset();
  resetEngineCounters();
}

/** Snapshot + `PERF_SHOW` line (with the counting engine's numbers). */
function take(scenario: string, extra: Record<string, unknown> = {}): WorkLog {
  const engine = engineCounters();
  return report(scenario, SHARED.work.snapshot(), engine ? { ...extra, engine } : extra);
}

/** A budget: the doors a scenario names, pinned exactly. */
type Budget = Record<string, number>;

/** The numbers a scenario is judged by. Door counts by name, plus the
 *  derived totals. */
function measured(work: WorkLog): Budget {
  return {
    ...work.calls,
    "=mutations": work.mutations.length,
    "=engineCalls": work.engineCalls(),
    "=reads": work.reads(),
    "=bytesWritten": work.bytesWritten,
    "=sceneItems": work.sceneItems,
    "=rejected": work.rejected,
  };
}

/** Pin `budget` exactly: every named number must match, and every door
 *  the scenario called must be named (a NEW door is new work). */
function expectBudget(scenario: string, work: WorkLog, budget: Budget): void {
  const got = measured(work);
  const keys = new Set([...Object.keys(budget), ...Object.keys(work.calls)]);
  const actual: Budget = {};
  for (const k of keys) actual[k] = got[k] ?? 0;
  const want: Budget = {};
  for (const k of keys) want[k] = budget[k] ?? 0;
  expect(actual, `${scenario}: the work moved — higher is a regression (never raise the budget), lower is earned (lower it in this commit)`).toEqual(want);
}

describe.skipIf(!ENGINE_BUILT)("perf budgets — work counted at the doors", () => {
  vi.setConfig({ testTimeout: BUDGET_TIMEOUT_MS });

  let h: HeadlessHost;
  let raw: BundleHost;
  let clip: TestClipboard;
  let session: WorkbookSession | null = null;

  beforeEach(async () => {
    clip = testClipboard();
    h = await openHost(clip);
    await h.load(blankPageIdml());
    raw = sheetHost(h);
    resetWork();
  });
  afterEach(() => {
    session?.dispose();
    session = null;
    h?.dispose();
  });

  /** A session over the COUNTED host (with the recording scene channel),
   *  holding `bytes`. Setup work is forgotten before it returns. */
  let channel: ReturnType<typeof withSceneChannel>;
  async function open(bytes: Uint8Array, range: string): Promise<WorkbookSession> {
    channel = withSceneChannel(raw);
    const { host } = countingHost(channel.host, SHARED);
    const s = createWorkbookSession(host);
    session = s;
    await s.import(bytes, "perf.xlsx");
    s.setRange(range);
    await s.flushPersist();
    await settle();
    resetWork();
    return s;
  }

  const engineOf = (s: WorkbookSession): SheetEngine => {
    const e = s.state().engine;
    if (!e) throw new Error("no engine");
    return e;
  };

  // COVERS: lower.ts placement — ONE batch for the frame + binding + table
  // (C-15 handles: the table's storyId is `$h:f`), the frame read back off
  // the table's story chain, column measurement, then ONE batch for the
  // whole content (pour + decor + text styles; core takes text ops in a
  // batch since v0.61, one rebuild per batch since v0.63). Was 1003
  // mutates: one awaited round trip — and one rebuild — per cell.
  it("place a 50×20 range as a native table [sheet.lower.page]", async () => {
    const s = await open(
      await authorWorkbook(50, 20, (r, c) => `r${r}c${c}`),
      "A1:T50",
    );
    const frame = await s.lowerSelection();
    await settle();
    const work = take("place 50x20");
    // Behaviour: a frame landed, every cell's text was poured once, and
    // the document the core exports holds exactly that text, cell by cell.
    expect(frame).not.toBeNull();
    const childOps = (op: string) =>
      work.mutations.reduce((n, m) => n + (m.kinds[op] ?? 0), 0);
    expect(childOps("insertText")).toBe(1000);
    expect(childOps("insertTable")).toBe(1);
    const tables = await exportedTableCells(h);
    expect(tables).toHaveLength(1);
    expect(tables[0].size).toBe(1000);
    for (const [r, c] of [[0, 0], [0, 19], [49, 0], [49, 19], [17, 7]]) {
      expect(tables[0].get(`${r}:${c}`)).toBe(`r${r}c${c}`);
    }
    expectBudget("place 50x20", work, {
      "document.meta": 1,
      "document.frameChain": 1, // the frame, read back off the table's story
      "text.measureString": 20,
      // → 1 once core resolves a table handle in a tableId position
      // (core 0eff96b, unreleased): frame + table + content, one batch.
      "document.mutate": 2,
      "selection.set": 1,
      // Wave 4: the PAGE door (styles + conditional formatting) replaces the
      // key-0 door one for one; the placement reads the selection (an
      // in-memory read, no document round trip) to land the frame there.
      "engine.getRangePage": 1,
      "selection.get": 1,
      "engine.listSheets": 1,
      "document.collection": 1, // the swatch read before the fill mints
      "=mutations": 2,
      "=engineCalls": 2,
      "=reads": 3,
      "=bytesWritten": 0,
      "=sceneItems": 0,
      "=rejected": 0,
    });
    // Undo: the content is ONE step (all 1000 cells at once), the frame
    // and its table the other — two steps where it was 1003.
    await raw.document.undo();
    const emptied = await exportedTableCells(h);
    expect(emptied).toHaveLength(1);
    expect([...emptied[0].values()].every((t) => t === "")).toBe(true);
    await raw.document.undo();
    expect(await exportedTableCells(h)).toHaveLength(0);
  });

  // COVERS: the K-1 in-frame edit session (session.ts typeCellChar /
  // commitCellEdit / submitInFrameGrid). Wave 2: the asks for a submit
  // inside one animation frame (one microtask turn here — no rAF in the
  // headless host) collapse into ONE window + submit of the final state,
  // and the engine window is memoised until the next change signal. Was
  // 21 getGridScene / 21 submits / 5922 items. The scene contract replaces
  // a frame's whole layer (no per-item patch), so a submit still carries
  // the window — see the per-frame scenario below.
  it("type 20 chars into one cell in-frame, then commit [sheet.grid.inframe]", async () => {
    const s = await open(
      await authorWorkbook(30, 8, (r, c) => String(r * 8 + c)),
      "A1:H30",
    );
    const frame = await s.lowerSelection();
    expect(frame).not.toBeNull();
    expect(await s.showGridInFrame(frame!)).toBe(true);
    // Content-space point inside the first body cell.
    expect(s.selectCellInFrame(4, 4)).toBe(true);
    await settle();
    resetWork();
    channel.submits.length = 0;
    const text = "abcdefghijklmnopqrst";
    for (const ch of text) expect(s.typeCellChar(ch)).toBe(true);
    expect(s.commitCellEdit()).toBe(true);
    await s.flushPersist();
    await settle();
    const work = take("type 20 chars in-frame + commit");
    // Behaviour: the committed cell holds the typed text (A1 — the hit),
    // and the LAST submit shows it (the final state is what landed).
    expect(engineOf(s).getCellDisplay(0, 0, 0)).toBe(text);
    expect(channel.submits.at(-1)?.texts).toContain(text);
    expectBudget("type 20 chars in-frame + commit", work, {
      "engine.getGridScene": 1, // was 21: the whole window, per keystroke
      "sceneLayer.submit": 1, // was 21: coalesced per frame
      "supports": 3,
      "engine.getCellInput": 1,
      "engine.setCell": 1,
      "engine.saveXlsx": 1, // the persist after the commit
      "blob.write": 1,
      "parts.write": 2,
      "storage.set": 1,
      "=mutations": 0,
      "=engineCalls": 4,
      "=reads": 0,
      "=bytesWritten": 5293,
      "=sceneItems": 282, // was 5922 (21 × the full grid)
      "=rejected": 0,
    });
  });

  // COVERS: the same gesture with every keystroke in its OWN frame (the
  // real cadence of typing): each keystroke still submits — the layer is
  // replaced whole — but none re-windows the engine: the buffer is
  // overlaid on the memoised window. Only the commit (a change signal)
  // re-windows.
  it("type 20 chars in-frame, one keystroke per frame [sheet.grid.inframe]", async () => {
    const s = await open(
      await authorWorkbook(30, 8, (r, c) => String(r * 8 + c)),
      "A1:H30",
    );
    const frame = await s.lowerSelection();
    expect(await s.showGridInFrame(frame!)).toBe(true);
    expect(s.selectCellInFrame(4, 4)).toBe(true);
    await settle();
    resetWork();
    channel.submits.length = 0;
    const text = "abcdefghijklmnopqrst";
    for (const ch of text) {
      expect(s.typeCellChar(ch)).toBe(true);
      await settle();
      // Each frame shows the buffer as typed so far.
      expect(channel.submits.at(-1)?.texts).toContain(text.slice(0, text.indexOf(ch) + 1));
    }
    expect(s.commitCellEdit()).toBe(true);
    await s.flushPersist();
    await settle();
    const work = take("type 20 chars in-frame, per frame");
    expect(engineOf(s).getCellDisplay(0, 0, 0)).toBe(text);
    expect(channel.submits.at(-1)?.texts).toContain(text);
    expectBudget("type 20 chars in-frame, per frame", work, {
      "engine.getGridScene": 1, // the commit's re-window; keystrokes reuse the memo
      "sceneLayer.submit": 21, // one per frame — the contract replaces the whole layer
      "supports": 23,
      "engine.getCellInput": 1,
      "engine.setCell": 1,
      "engine.saveXlsx": 1,
      "blob.write": 1,
      "parts.write": 2,
      "storage.set": 1,
      "=mutations": 0,
      "=engineCalls": 4,
      "=reads": 0,
      "=bytesWritten": 5293,
      "=sceneItems": 5922,
      "=rejected": 0,
    });
  });

  // COVERS: session.sortRange → engine.sortRange (one wasm call), whose
  // Rust apply lane re-enters every moved cell and recalcs the SUM once per
  // cell (counted in sheet-js/tests/perf_budgets.rs), then the persist.
  it("sort 1000 rows under a SUM [sheet.edit.ops]", async () => {
    const s = await open(
      await authorWorkbook(1000, 1, (r) => String(1000 - r), [[0, 2, "=SUM(A1:A1000)"]]),
      "A1:A1000",
    );
    const r = s.sortRange(0, true, false);
    expect(r.ok).toBe(true);
    await s.flushPersist();
    await settle();
    const work = take("sort 1000 rows");
    const e = engineOf(s);
    expect(e.getCellDisplay(0, 0, 0)).toBe("1");
    expect(e.getCellDisplay(0, 999, 0)).toBe("1000");
    expect(e.getCellDisplay(0, 0, 2)).toBe("500500");
    expectBudget("sort 1000 rows", work, {
      "engine.sortRange": 1, // one call — the per-cell recalcs are inside it (sheet-js perf_budgets)
      "engine.saveXlsx": 1,
      "blob.write": 1,
      "parts.write": 2,
      "storage.set": 1,
      "supports": 2,
      "=mutations": 0,
      "=engineCalls": 2,
      "=reads": 0,
      "=bytesWritten": 18759,
      "=sceneItems": 0,
      "=rejected": 0,
    });
  });

  // COVERS: session.pasteAtSelection — the tabular payload re-typed through
  // editCell, one engine.setCell (one recalc, one result marshalled back)
  // per pasted cell. Batched: one setCells call.
  it("paste 100×10 at the selection [sheet.edit.ops]", async () => {
    const s = await open(
      await authorWorkbook(1, 1, () => "seed", [[100, 0, "=SUM(A1:A100)"]]),
      "A1:J101",
    );
    const rows = Array.from({ length: 100 }, (_, r) =>
      Array.from({ length: 10 }, (_, c) => String(r * 10 + c + 1)),
    );
    clip.payload = { tabular: { rows } } as never;
    s.setGridSelection(0, 0, 1, 1);
    resetWork();
    const r = await s.pasteAtSelection();
    expect(r).toEqual({ ok: true, rows: 100, cols: 10 });
    await s.flushPersist();
    await settle();
    const work = take("paste 100x10");
    const e = engineOf(s);
    expect(e.getCellDisplay(0, 99, 9)).toBe("1000");
    // A1..A100 = 1, 11, 21, … 991 → 49 600.
    expect(e.getCellDisplay(0, 100, 0)).toBe("49600");
    expectBudget("paste 100x10", work, {
      "clipboard.read": 1,
      "engine.getCellInput": 1000, // the undo journal's prior input, per cell → 1 range read
      "engine.setCell": 1000, // one recalc + one marshalled result per cell → 1 setCells
      "engine.saveXlsx": 1,
      "blob.write": 1,
      "parts.write": 2,
      "storage.set": 1,
      "supports": 2,
      "=mutations": 0,
      "=engineCalls": 2001,
      "=reads": 0,
      "=bytesWritten": 9817,
      "=sceneItems": 0,
      "=rejected": 0,
    });
  });

  // COVERS: live re-pagination (lower.ts subscribeChainReflow →
  // paginatePass). Wave 4: a burst of content-box reflows (a resize drag)
  // settles into ONE re-pagination, and that pass REFRESHES the chain's own
  // tables in place (rows realigned, only moved rows re-poured) — it used to
  // re-paginate per event and lower EVERY page again as a new table, with
  // the previous tables never removed (1291 mutations for 10 reflows).
  it("reflow a two-frame chain 10 times [sheet.lower.paginate]", async () => {
    const s = await open(
      await authorWorkbook(120, 3, (r, c) => `${r}:${c}`),
      "A1:C120",
    );
    // Two threaded frames on the page (raw host — setup, not counted).
    const before = new Set(
      (await raw.document.collection<{ selfId: string }>("stories")).map((x) => x.selfId),
    );
    const mk = async (bounds: [number, number, number, number]) => {
      const o = await raw.document.mutate({ op: "insertTextFrame", args: { pageId: "usp", bounds } });
      if (!o.applied || !o.createdId) throw new Error("insertTextFrame rejected");
      return (o.createdId as { id: string }).id;
    };
    const f1 = await mk([36, 36, 380, 576]);
    const f2 = await mk([400, 36, 756, 576]);
    expect((await raw.document.mutate({ op: "linkFrames", args: { from: f1, to: f2 } })).applied).toBe(true);
    const storyId = (await raw.document.collection<{ selfId: string }>("stories"))
      .map((x) => x.selfId)
      .find((id) => !before.has(id));
    expect(storyId).toBeDefined();

    const { host } = countingHost(withSceneChannel(raw).host, SHARED);
    const engine = engineOf(s);
    const first = await lowerPaginatedToChain(host, engine, 0, "A1:C120", storyId!);
    expect(first?.pages.length).toBe(2);
    const sub = subscribeChainReflow(host, engine, 0, "A1:C120", storyId!, {
      from: first,
    });
    await settle();
    resetWork();

    let reflows = 0;
    const seen = raw.document.onDidChange((e) => {
      if (e.reflow) reflows += 1;
    });
    // A resize drag: ten content-box changes back to back.
    for (let i = 1; i <= 10; i++) {
      const o = await raw.document.mutate({
        op: "resizeFrame",
        args: { frameId: f1, bounds: [36, 36, 380 - i * 10, 576] },
      });
      expect(o.applied).toBe(true);
    }
    await new Promise((r) => setTimeout(r, CHAIN_REFLOW_DEBOUNCE_MS + 50));
    await sub.idle(); // the debounced pass, awaited
    await settle();
    seen.dispose();
    const work = take("reflow x10", { reflows });
    // Behaviour: core reported each resize as a reflow; ONE re-pagination
    // ran; no table was added (the two tables were refreshed in place) and
    // the rows the shorter first frame lost moved into the second table.
    expect(reflows).toBe(10);
    expect(work.mutations.filter((m) => m.op === "insertTable").length).toBe(0);
    const pages = sub.current()!.pages;
    expect(pages).toHaveLength(2);
    expect(pages[0].content.rows.length).toBeLessThan(first!.pages[0].content.rows.length);
    sub.dispose();
    expectBudget("reflow x10", work, {
      // Wave 4: debounced to ONE pass; the pass refreshes the two tables in
      // place (rows realigned: the rows the first frame lost are deleted
      // there and inserted in the second, their text as one batch). Was
      // 10 / 10 / 10 / 20 / 10 / 60 / 10 / 1291 — every reflow re-lowered
      // every page as a NEW table and the old ones stayed.
      "document.frameChain": 1,
      "document.elementGeometry": 1,
      "text.measureString": 6,
      "engine.paginate": 1,
      "document.mutate": 5,
      "=mutations": 5,
      "=engineCalls": 1,
      "=reads": 2,
      "=bytesWritten": 0,
      "=sceneItems": 0,
      "=rejected": 0, // against the real core engine: every reshape op applied
    });
  });

  /** Render `Panel` and return its test tree (act-wrapped). */
  function render(Panel: () => ReturnType<ReturnType<typeof makeGridPanel>>): ReactTestRenderer {
    let tree!: ReactTestRenderer;
    act(() => {
      tree = create(createElement(Panel));
    });
    return tree;
  }
  const byData = (tree: ReactTestRenderer, attr: string): ReactTestInstance =>
    tree.root.find((n) => typeof n.type === "string" && n.props[attr] !== undefined);

  // COVERS: the grid panel's render path (grid-panel.tsx). Every render
  // re-windowed the grid (`getGridScene`), re-read the selected cell's input
  // (`getCellInput`) and re-serialised the SVG — and a formula-bar keystroke
  // is a render. Wave 2: the session memoises its panel reads until the next
  // change signal, and the SVG string is built once per scene.
  it("type 12 chars into the grid panel's formula bar [sheet.grid.panel-edit-contract]", async () => {
    const s = await open(
      await authorWorkbook(30, 8, (r, c) => String(r * 8 + c)),
      "A1:H30",
    );
    s.setGridSelection(2, 3, 1, 1);
    const tree = render(makeGridPanel(raw, s));
    resetWork();
    const input = byData(tree, "data-formula-input");
    const text = "=SUM(A1:A30)";
    for (let i = 1; i <= text.length; i++) {
      act(() => {
        input.props.onChange({
          target: { value: text.slice(0, i), selectionStart: i },
        });
      });
    }
    // Behaviour: the bar shows the draft.
    expect(byData(tree, "data-formula-input").props.value).toBe(text);
    // Enter commits the draft through the journaled lane.
    act(() => {
      byData(tree, "data-formula-input").props.onKeyDown({
        key: "Enter",
        preventDefault() {},
      });
    });
    const work = take("grid panel: 12 formula-bar keystrokes + Enter");
    expect(engineOf(s).getCellInput(0, 2, 3)).toBe(text);
    act(() => tree.unmount());
    expectBudget("grid panel: 12 formula-bar keystrokes + Enter", work, {
      // The keystrokes: 0 (was 12 — one re-window per render). The commit's
      // re-render: 1.
      "engine.getGridScene": 1,
      // The journal's prior input reuses the bar's memoised read (was 1);
      // the commit's re-render prefills the bar with the new input: 1.
      "engine.getCellInput": 1,
      "engine.setCell": 1,
      "=mutations": 0,
      "=engineCalls": 3,
      "=reads": 0,
      "=bytesWritten": 0,
      "=sceneItems": 0,
      "=rejected": 0,
    });
  });

  // COVERS: the workbook panel's function browser + inventories
  // (workbook-panel.tsx). A filter keystroke re-rendered the panel, which
  // re-read the whole function registry (`listFunctions`), the sheets and
  // the four inventories (freeze panes, validations, comments, charts).
  it("type 6 chars into the workbook panel's function filter [sheet.plugin.formula-bar]", async () => {
    const s = await open(
      await authorWorkbook(10, 4, (r, c) => String(r * 4 + c)),
      "A1:D10",
    );
    const tree = render(makeWorkbookPanel(raw, s));
    resetWork();
    const filter = byData(tree, "data-sheet-fn-filter");
    const q = "VLOOKU";
    for (let i = 1; i <= q.length; i++) {
      act(() => {
        filter.props.onChange({ target: { value: q.slice(0, i) } });
      });
    }
    const work = take("workbook panel: 6 filter keystrokes");
    // Behaviour: the filtered list shows VLOOKUP.
    expect(
      tree.root.findAll((n) => n.props["data-sheet-fn"] === "VLOOKUP").length,
    ).toBeGreaterThan(0);
    act(() => tree.unmount());
    expectBudget("workbook panel: 6 filter keystrokes", work, {
      // was 6 each: listFunctions, listSheets, listFreezePanes,
      // listDataValidations, listComments, listCharts, calcSettings,
      // chartKinds — one per render.
      "engine.listFunctions": 0,
      "engine.listSheets": 0,
      "engine.listFreezePanes": 0,
      "engine.listDataValidations": 0,
      "engine.listComments": 0,
      "engine.listCharts": 0,
      "engine.calcSettings": 0,
      "engine.chartKinds": 0,
      "=mutations": 0,
      "=engineCalls": 0,
      "=reads": 0,
      "=bytesWritten": 0,
      "=sceneItems": 0,
      "=rejected": 0,
    });
  });

  // COVERS: the ADR-023 binding providers' re-read signal
  // (binding-provider/invalidation.ts). Every session change signal used to
  // invalidate BOTH providers — each a host re-read that lowers the
  // selected range through the engine. Now the text provider re-reads when
  // its selection moves or an edit lands inside it, the swatches provider
  // only on unclassified changes.
  it("binding providers across edits outside the selection [sheet.edit.ops]", async () => {
    const s = await open(
      await authorWorkbook(20, 4, (r, c) => String(r * 4 + c)),
      "A1:D20",
    );
    const counts = { swatches: 0, text: 0 };
    const sub = subscribeProviderInvalidation(s, {
      swatches: { invalidate: () => (counts.swatches += 1) },
      text: { invalidate: () => (counts.text += 1) },
    });
    s.setGridSelection(0, 0, 2, 2); // A1:B2 — moves the selection: text re-reads
    for (let r = 10; r < 20; r++) expect(s.editCell(0, r, 3, String(r))).toBe(true);
    clip.payload = { tabular: { rows: [["x", "y"], ["z", "w"]] } } as never;
    s.setGridSelection(14, 2, 1, 1);
    expect((await s.pasteAtSelection()).ok).toBe(true); // selection moved, paste inside it
    s.setGridSelection(0, 0, 2, 2); // back to A1:B2
    expect(s.editCell(0, 1, 1, "inside")).toBe(true); // an edit INSIDE A1:B2
    await settle();
    sub.dispose();
    // Behaviour: the edits landed.
    expect(engineOf(s).getCellDisplay(0, 1, 1)).toBe("inside");
    expect(engineOf(s).getCellDisplay(0, 15, 3)).toBe("w");
    // Was 15 / 15 — one each per signal (3 selections, 10 edits, the
    // paste, the inside edit). Now: 3 selection moves + the paste inside
    // its selection + the edit inside A1:B2 = 5 text re-reads; the palette
    // never moved.
    expect(counts).toEqual({ swatches: 0, text: 5 });
  });
});
