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

import type { BundleHost } from "@paged-media/plugin-api";
import type { HeadlessHost } from "@paged-media/plugin-sdk";

import {
  CHAIN_REFLOW_DEBOUNCE_MS,
  createWorkbookSession,
  lowerPaginatedToChain,
  subscribeChainReflow,
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
  async function open(bytes: Uint8Array, range: string): Promise<WorkbookSession> {
    const { host } = countingHost(withSceneChannel(raw).host, SHARED);
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

  // COVERS: lower.ts placement — phase 1 (frame + binding batch), the
  // stories diff, column measurement, insertTable, then the cell pour.
  // 1000 cells → 1000 insertText mutates, one awaited round trip each
  // (lower.ts pourCellContent). Core has applied text inside a batch as one
  // undo step since v0.61, so the pour → 1 batch is plugin-side (Wave 2).
  it("place a 50×20 range as a native table [sheet.lower.page]", async () => {
    const s = await open(
      await authorWorkbook(50, 20, (r, c) => `r${r}c${c}`),
      "A1:T50",
    );
    const frame = await s.lowerSelection();
    await settle();
    const work = take("place 50x20");
    // Behaviour: a frame landed and every cell's text was poured, once.
    expect(frame).not.toBeNull();
    expect(work.mutations.filter((m) => m.op === "insertText").length).toBe(1000);
    expectBudget("place 50x20", work, {
      "document.meta": 1,
      "document.collection": 3,
      "text.measureString": 20,
      "document.mutate": 1003, // → 4 (frame batch, insertTable, ONE pour batch, decor)
      "selection.set": 1,
      // Wave 4: the PAGE door (styles + conditional formatting) replaces the
      // key-0 door one for one; the placement reads the selection (an
      // in-memory read, no document round trip) to land the frame there.
      "engine.getRangePage": 1,
      "selection.get": 1,
      "engine.listSheets": 1,
      "=mutations": 1003,
      "=engineCalls": 2,
      "=reads": 4,
      "=bytesWritten": 0,
      "=sceneItems": 0,
      "=rejected": 0,
    });
  });

  // COVERS: the K-1 in-frame edit session (session.ts typeCellChar /
  // commitCellEdit / submitInFrameGrid) — every keystroke re-renders the
  // whole windowed grid scene and submits it (and core answers a vector
  // SubmitSceneLayer with CacheEffect::ClearAll — the core half is Wave 8).
  // Inherent: one small submit per keystroke at most, coalesced per frame.
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
    const text = "abcdefghijklmnopqrst";
    for (const ch of text) expect(s.typeCellChar(ch)).toBe(true);
    expect(s.commitCellEdit()).toBe(true);
    await s.flushPersist();
    await settle();
    const work = take("type 20 chars in-frame + commit");
    // Behaviour: the committed cell holds the typed text (A1 — the hit).
    expect(engineOf(s).getCellDisplay(0, 0, 0)).toBe(text);
    expectBudget("type 20 chars in-frame + commit", work, {
      "engine.getGridScene": 21, // the whole window, per keystroke → 1 + a cell patch
      "sceneLayer.submit": 21, // → coalesced per animation frame
      "supports": 23,
      "engine.getCellInput": 1,
      "engine.setCell": 1,
      "engine.saveXlsx": 1, // the persist after the commit
      "blob.write": 1,
      "parts.write": 2,
      "storage.set": 1,
      "=mutations": 0,
      "=engineCalls": 24,
      "=reads": 0,
      "=bytesWritten": 5293,
      "=sceneItems": 5922, // 21 × the full grid; a keystroke changes one cell
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
});
