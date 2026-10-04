// Wave 9 — consuming the protocol-66 doors, against a REAL headless host
// (core via canvas-wasm: real document undo, real plugin metadata) and the
// REAL sheet engine. Each 66 door is FEATURE-DETECTED: the real host under
// test predates it (pins stay at 0.2.37 until the owner tags 66), so these
// scenarios drive the fallback path for real, and the 66 path through a
// host wrapper that adds the door's reply shape.
//
// Real engine: skipped without the wasm artifact, FAILS under
// REQUIRE_REAL_ENGINE=1 (the engine-real.spec.ts dual gate).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";
import type { HeadlessHost } from "@paged-media/plugin-sdk";
import { parseBinding } from "@paged-media/sheet-host-model";

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { createWorkbookSession, storyOfFrame, type WorkbookSession } from "../src";
import { countingHost, Tally } from "./perf/counting-host";

import {
  ENGINE_BUILT,
  WASM,
  authorWorkbook,
  blankPageIdml,
  exportedIdmlParts,
  exportedTableCells,
  openHost,
  settle,
  sheetHost,
  withDoors66,
  withSceneChannel,
} from "./perf/workload";

const HERE = dirname(fileURLToPath(import.meta.url));
const corpus = (name: string) =>
  new Uint8Array(readFileSync(join(HERE, "..", "..", "..", "corpus/xlsx-corpus", name)));

/** Every addressable element id in the scene tree (kind:id). */
async function treeIds(host: BundleHost): Promise<Set<string>> {
  const out = new Set<string>();
  const walk = (ns: readonly { id?: { kind: string; id: unknown } | null; children?: unknown[] }[]) => {
    for (const n of ns) {
      if (n.id && typeof n.id.id === "string") out.add(`${n.id.kind}:${n.id.id}`);
      if (n.children) walk(n.children as never);
    }
  };
  walk((await host.document.tree()) as never);
  return out;
}

if (process.env.REQUIRE_REAL_ENGINE === "1" && !ENGINE_BUILT) {
  describe("wave 9 (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing`);
    });
  });
}

describe.skipIf(!ENGINE_BUILT)("wave 9 — placed-table refresh", () => {
  vi.setConfig({ testTimeout: 60_000 });
  let h: HeadlessHost;
  let host: BundleHost;
  let s: WorkbookSession | null = null;

  beforeEach(async () => {
    h = await openHost();
    await h.load(blankPageIdml());
    host = withSceneChannel(sheetHost(h)).host;
  });
  afterEach(() => {
    s?.dispose();
    s = null;
    h?.dispose();
  });

  const shownVersion = async (frame: string) =>
    parseBinding(await host.document.getMetadata({ kind: "textFrame", id: frame } as never))
      ?.data.contentVersion;

  it("a refresh is ONE document undo step: binding and page text revert together [sheet.edit.undo]", async () => {
    const session = createWorkbookSession(host);
    s = session;
    await session.import(await authorWorkbook(3, 2, (r, c) => String(r * 2 + c + 1)), "u.xlsx");
    session.setRange("A1:B3");
    const frame = await session.lowerSelection();
    expect(frame).not.toBeNull();
    await settle();
    const v0 = await shownVersion(frame!);
    expect((await exportedTableCells(h))[0].get("0:0")).toBe("1");

    expect(session.editCell(0, 0, 0, "42")).toBe(true);
    await session.refreshPlacements();
    await settle();
    expect((await exportedTableCells(h))[0].get("0:0")).toBe("42");
    expect(await shownVersion(frame!)).toBeGreaterThan(v0!);

    // ONE document undo: the page text AND the binding stamp go back, and
    // the workbook follows the binding.
    await host.document.undo();
    await settle();
    await settle();
    expect(await shownVersion(frame!)).toBe(v0);
    expect((await exportedTableCells(h))[0].get("0:0")).toBe("1");
    expect(session.cellInputAt(0, 0)).toBe("1");
  });
});

describe.skipIf(!ENGINE_BUILT)("wave 9 — placed charts and frame stories", () => {
  vi.setConfig({ testTimeout: 60_000 });
  let h: HeadlessHost;
  let raw: BundleHost;
  let s: WorkbookSession | null = null;

  beforeEach(async () => {
    h = await openHost();
    await h.load(blankPageIdml());
    raw = withSceneChannel(sheetHost(h)).host;
  });
  afterEach(() => {
    s?.dispose();
    s = null;
    h?.dispose();
  });

  /** The spreads as core exports them — where a chart's art lives. */
  const spreads = async () =>
    [...(await exportedIdmlParts(h))]
      .filter(([n]) => n.startsWith("Spreads/"))
      .map(([, x]) => x)
      .join("\n");

  async function chartScenario(host: BundleHost) {
    const session = createWorkbookSession(host);
    s = session;
    await session.import(corpus("09-chart.xlsx"), "09-chart.xlsx");
    const [chart] = session.listCharts();
    const empty = await treeIds(raw);
    expect(await session.lowerChart(chart.index)).toBe(true);
    await settle();
    const placed = new Set([...(await treeIds(raw))].filter((k) => !empty.has(k)));
    expect(placed.size).toBeGreaterThan(0);
    const art = await spreads();
    // An edit to a series value (B2) replaces the chart: as many elements
    // (never a second chart beside the first), different art.
    session.editCell(0, 1, 1, "999");
    await session.refreshPlacements();
    await settle();
    const replaced = new Set([...(await treeIds(raw))].filter((k) => !empty.has(k)));
    expect(replaced.size).toBe(placed.size);
    expect(await spreads()).not.toBe(art);
    return { art };
  }

  it("a chart's labels land with it, and its replacement is ONE undo step [sheet.chart.engine]", async () => {
    const { art } = await chartScenario(raw);
    // The label texts are in the document (poured in the chart's batch).
    const parts = await exportedIdmlParts(h);
    const stories = [...parts].filter(([n]) => n.startsWith("Stories/")).map(([, x]) => x);
    expect(stories.some((x) => /<Content>[^<]+<\/Content>/.test(x))).toBe(true);
    // ONE document undo restores the chart it replaced, whole.
    await raw.document.undo();
    await settle();
    expect(await spreads()).toBe(art);
  });

  it("with 66 `minted`, a chart is placed and replaced with no scene-tree or story reads [sheet.chart.engine]", async () => {
    const tally = new Tally();
    const { host } = countingHost(withDoors66(h, raw, { minted: true }), tally);
    await chartScenario(host);
    // The FIRST lowering cannot know yet whether the host sends `minted`, so
    // it reads the tree before its batch (the fallback's half); its outcome
    // answers, and the replacement reads nothing. No tree read AFTER either
    // batch, no stories read at all.
    expect(tally.work.count("document.tree")).toBe(1);
    // pages (the active page, once) + one swatch read per lowering.
    expect(tally.work.count("document.collection")).toBe(3);
    expect(tally.work.count("document.mutate")).toBe(2); // place; replace (deletes + new art + labels)
  });

  it("with 66 `minted`, a placement names its frame by handle: no frame-chain read [sheet.lower.page]", async () => {
    const tally = new Tally();
    const { host } = countingHost(withDoors66(h, raw, { minted: true }), tally);
    const session = createWorkbookSession(host);
    s = session;
    await session.import(await authorWorkbook(3, 2, (r, c) => `${r}${c}`), "m.xlsx");
    session.setRange("A1:B3");
    const frame = await session.lowerSelection();
    expect(frame).not.toBeNull();
    expect(tally.work.count("document.frameChain")).toBe(0);
    expect((await exportedTableCells(h))[0].get("2:1")).toBe("21");
    expect(
      parseBinding(await raw.document.getMetadata({ kind: "textFrame", id: frame! } as never)),
    ).not.toBeNull();
  });

  it("frame -> story: one geometry read with 66 storyId, the stories walk without [sheet.lower.paginate]", async () => {
    const before = new Set(
      (await raw.document.collection<{ selfId: string }>("stories")).map((x) => x.selfId),
    );
    const o = await raw.document.mutate({
      op: "insertTextFrame",
      args: { pageId: "usp", bounds: [36, 36, 200, 300] },
    });
    expect(o.applied).toBe(true);
    const frameId = (o as { createdId: { id: string } }).createdId.id;
    const storyId = (await raw.document.collection<{ selfId: string }>("stories"))
      .map((x) => x.selfId)
      .find((id) => !before.has(id));

    const t66 = new Tally();
    const h66 = countingHost(withDoors66(h, raw, { geometryStoryId: true }), t66).host;
    expect(await storyOfFrame(h66, frameId)).toBe(storyId);
    expect(t66.work.calls).toEqual({ "document.elementGeometry": 1 });

    // A pre-66 host: the geometry item has no story — asked once, then the
    // walk; the second resolution skips the geometry read.
    const t64 = new Tally();
    const h64 = countingHost(raw, t64).host;
    expect(await storyOfFrame(h64, frameId)).toBe(storyId);
    expect(await storyOfFrame(h64, frameId)).toBe(storyId);
    expect(t64.work.count("document.elementGeometry")).toBe(1);
    expect(t64.work.count("document.collection")).toBe(2);
  });
});
