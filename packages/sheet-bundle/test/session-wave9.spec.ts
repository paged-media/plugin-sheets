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

describe.skipIf(!ENGINE_BUILT)("wave 9 — placements from an earlier session", () => {
  vi.setConfig({ testTimeout: 60_000 });
  let h: HeadlessHost;
  let host: BundleHost;
  const sessions: WorkbookSession[] = [];

  beforeEach(async () => {
    h = await openHost();
    await h.load(blankPageIdml());
    host = withSceneChannel(sheetHost(h)).host;
  });
  afterEach(() => {
    for (const x of sessions.splice(0)) x.dispose();
    h?.dispose();
  });

  async function firstSession(): Promise<WorkbookSession> {
    const a = createWorkbookSession(host);
    sessions.push(a);
    await a.import(await authorWorkbook(3, 2, (r, c) => `${r}${c}`), "p.xlsx");
    a.setRange("A1:B3");
    expect(await a.lowerSelection()).not.toBeNull();
    await settle();
    return a;
  }

  it("a later session finds the table and refreshes it in place [sheet.plugin.persistence]", async () => {
    const a = await firstSession();
    a.editCell(0, 0, 0, "first");
    await a.refreshPlacements();
    await a.flushPersist();
    a.dispose();

    const b = createWorkbookSession(host);
    sessions.push(b);
    expect(await b.restore()).toBe(true);
    expect(b.cellInputAt(0, 0)).toBe("first");
    b.editCell(0, 2, 1, "later");
    await b.refreshPlacements();
    await settle();
    const tables = await exportedTableCells(h);
    expect(tables).toHaveLength(1);
    expect(tables[0].get("0:0")).toBe("first");
    expect(tables[0].get("2:1")).toBe("later");
  });

  it("a page that does not show the saved workbook is replaced on restore [sheet.plugin.persistence]", async () => {
    const a = await firstSession();
    // Edited and persisted, never refreshed onto the page.
    a.editCell(0, 1, 0, "unseen");
    await a.flushPersist();
    a.dispose();
    expect((await exportedTableCells(h))[0].get("1:0")).toBe("10");

    const b = createWorkbookSession(host);
    sessions.push(b);
    expect(await b.restore()).toBe(true);
    await b.refreshPlacements();
    await settle();
    const tables = await exportedTableCells(h);
    expect(tables).toHaveLength(1);
    expect(tables[0].get("1:0")).toBe("unseen");
    expect(tables[0].get("2:1")).toBe("21");
  });
});

/** `row:col` → the paragraph style each table cell's text sits in, read off
 *  the exported stories (`Name="col:row"` in IDML). */
async function cellParagraphStyles(h: HeadlessHost): Promise<Map<string, string>> {
  const out = new Map<string, string>();
  for (const [name, xml] of await exportedIdmlParts(h)) {
    if (!name.startsWith("Stories/")) continue;
    for (const c of xml.matchAll(/<Cell\b[^>]*\bName="(\d+):(\d+)"[^>]*>([\s\S]*?)<\/Cell>/g)) {
      const p = /<ParagraphStyleRange\b[^>]*AppliedParagraphStyle="([^"]*)"/.exec(c[3]);
      out.set(`${c[2]}:${c[1]}`, p?.[1] ?? "");
    }
  }
  return out;
}

describe.skipIf(!ENGINE_BUILT)("wave 9 — alignment reaches the placed table", () => {
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

  it("numbers sit right, text left; an edit that changes the kind re-aligns [sheet.lower.page]", async () => {
    const session = createWorkbookSession(host);
    s = session;
    await session.import(
      await authorWorkbook(3, 2, (r, c) => (c === 0 ? `name${r}` : String(r * 10 + 5))),
      "a.xlsx",
    );
    session.setRange("A1:B3");
    expect(await session.lowerSelection()).not.toBeNull();
    await settle();
    const RIGHT = "ParagraphStyle/paged.sheet align right";
    let styles = await cellParagraphStyles(h);
    expect(styles.get("0:1")).toBe(RIGHT);
    expect(styles.get("2:1")).toBe(RIGHT);
    expect(styles.get("0:0")).not.toBe(RIGHT);

    session.editCell(0, 0, 0, "42"); // text → number
    session.editCell(0, 1, 1, "label"); // number → text
    await session.refreshPlacements();
    await settle();
    styles = await cellParagraphStyles(h);
    expect(styles.get("0:0")).toBe(RIGHT);
    expect(styles.get("1:1")).not.toBe(RIGHT);
    expect(styles.get("2:1")).toBe(RIGHT);
  });
});

describe.skipIf(!ENGINE_BUILT)("wave 9 — format changes are undo steps [sheet.edit.undo]", () => {
  vi.setConfig({ testTimeout: 60_000 });
  let h: HeadlessHost;
  let s: WorkbookSession | null = null;

  beforeEach(async () => {
    h = await openHost();
    await h.load(blankPageIdml());
  });
  afterEach(() => {
    s?.dispose();
    s = null;
    h?.dispose();
  });

  /** A blank workbook with a 3×2 block. */
  async function block(): Promise<WorkbookSession> {
    const session = createWorkbookSession(withSceneChannel(sheetHost(h)).host);
    s = session;
    await session.newWorkbook();
    const cells = [["Region", "Units"], ["North", "1.5"], ["South", "2"]];
    cells.forEach((row, r) => row.forEach((v, c) => session.editCell(0, r, c, v)));
    return session;
  }

  it("setStyle and borders: one Cmd-Z each, redo re-applies", async () => {
    const session = await block();
    session.setGridSelection(0, 0, 1, 2); // A1:B1
    expect(session.setStyle({ bold: true, fill: "#FFFF00" })).toEqual({ ok: true });
    expect(session.setBorders("outline", { style: "thin" })).toEqual({ ok: true });
    const style = () => session.state().engine!.getStyle!(0, 0, 1);
    expect(style()).toMatchObject({ bold: true, fill: "#FFFF00" });
    expect(style().borderTop?.style).toBe("thin");
    expect(session.undoCellEdit()).toBe(true); // the borders
    expect(style().borderTop?.style ?? "none").toBe("none");
    expect(style().bold).toBe(true);
    expect(session.undoCellEdit()).toBe(true); // the style
    expect(style().bold).toBe(false);
    expect(session.redoCellEdit()).toBe(true);
    expect(style()).toMatchObject({ bold: true, fill: "#FFFF00" });
    // The cell edits before stay reachable behind them.
    expect(session.undoCellEdit()).toBe(true);
    expect(session.undoCellEdit()).toBe(true);
    expect(session.cellInputAt(2, 1)).toBe("");
  });

  it("merge brings its cleared cells back on undo; unmerge re-merges", async () => {
    const session = await block();
    session.setGridSelection(0, 0, 1, 2); // A1:B1
    expect(session.mergeTarget()).toEqual({ ok: true });
    expect(session.cellInputAt(0, 1)).toBe("");
    expect(session.undoCellEdit()).toBe(true);
    expect(session.layout()!.merges).toEqual([]);
    expect(session.cellInputAt(0, 1)).toBe("Units");
    expect(session.redoCellEdit()).toBe(true);
    expect(session.layout()!.merges).toEqual(["A1:B1"]);
    expect(session.cellInputAt(0, 1)).toBe("");
    expect(session.unmergeTarget()).toEqual({ ok: true });
    expect(session.undoCellEdit()).toBe(true);
    expect(session.layout()!.merges).toEqual(["A1:B1"]);
  });

  it("sizes, freeze and names: one step each", async () => {
    const session = await block();
    session.setGridSelection(1, 1, 2, 1); // B2:B3
    expect(session.setColumnWidth(12)).toEqual({ ok: true });
    expect(session.setRowHeight(24)).toEqual({ ok: true });
    expect(session.toggleFreeze()).toEqual({ ok: true });
    expect(session.defineName("Units", "B2:B3")).toEqual({ ok: true });
    expect(session.defineName("Units", "B2")).toEqual({ ok: true }); // redefine
    const names = () => session.names().map((n) => `${n.name}=${n.refersTo}`);
    expect(names()[0]).toMatch(/^Units=.*B\$?2$/);
    expect(session.undoCellEdit()).toBe(true); // the redefinition
    expect(names()[0]).toMatch(/B\$?3$/);
    expect(session.undoCellEdit()).toBe(true); // the definition
    expect(names()).toEqual([]);
    expect(session.undoCellEdit()).toBe(true); // the freeze
    expect(session.layout()).toMatchObject({ freezeRows: 0, freezeCols: 0 });
    expect(session.undoCellEdit()).toBe(true); // the row heights
    expect(session.layout()!.rowHeights).toEqual([]);
    expect(session.undoCellEdit()).toBe(true); // the column width
    expect(session.layout()!.colWidths).toEqual([]);
    expect(session.redoCellEdit()).toBe(true);
    expect(session.layout()!.colWidths).toEqual([[1, 12]]);
    // A deleted name comes back.
    session.defineName("Keep", "A1");
    expect(session.deleteName("Keep")).toEqual({ ok: true });
    expect(session.undoCellEdit()).toBe(true);
    expect(session.names().some((n) => n.name === "Keep")).toBe(true);
  });

  it("a fill's carried formats undo with its values, in one step", async () => {
    const session = await block();
    session.setGridSelection(1, 1, 1, 1); // B2
    session.setStyle({ bold: true, numFmt: "0.00" });
    session.setGridSelection(1, 1, 2, 1); // B2:B3
    expect(session.fillDown()).toEqual({ ok: true });
    const e = session.state().engine!;
    expect(e.getStyle!(0, 2, 1).bold).toBe(true);
    expect(e.getCellDisplay(0, 2, 1)).toBe("1.50");
    expect(session.undoCellEdit()).toBe(true);
    expect(e.getStyle!(0, 2, 1).bold).toBe(false);
    expect(session.cellInputAt(2, 1)).toBe("2");
    expect(e.getStyle!(0, 1, 1).bold).toBe(true); // the source keeps its own
  });
});

describe.skipIf(!ENGINE_BUILT)("wave 9 — data bars on the native table [sheet.xlsx.condfmt]", () => {
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

  /** The spread's page items in stacking order (back first). */
  const stack = async () => {
    for (const [n, x] of await exportedIdmlParts(h)) {
      if (!n.startsWith("Spreads/")) continue;
      return [...x.matchAll(/<(TextFrame|Polygon)\b[^>]*\bSelf="([^"]+)"/g)].map((m) => `${m[1]}:${m[2]}`);
    }
    return [];
  };
  const spread = async () =>
    [...(await exportedIdmlParts(h))].filter(([n]) => n.startsWith("Spreads/")).map(([, x]) => x).join("");

  async function placeBars(host: BundleHost) {
    const session = createWorkbookSession(host);
    s = session;
    await session.import(corpus("08-condfmt.xlsx"), "08-condfmt.xlsx");
    session.setRange("D1:D5");
    expect(await session.lowerSelection()).not.toBeNull();
    await settle();
    return session;
  }

  it("bars are drawn under the table; a refresh redraws them in the same undo step", async () => {
    const session = await placeBars(raw);
    let order = await stack();
    // D1 is the domain minimum (a zero-length bar); D2..D5 draw.
    expect(order.filter((x) => x.startsWith("Polygon"))).toHaveLength(4);
    expect(order[order.length - 1]).toMatch(/^TextFrame/); // the table's frame on top
    expect(await spread()).toContain("Color/uPagedSheetDataBar638EC6");
    const before = await spread();

    session.editCell(0, 1, 3, "5"); // D2: 40 → 5, the new minimum
    await session.refreshPlacements();
    await settle();
    order = await stack();
    expect(order.filter((x) => x.startsWith("Polygon"))).toHaveLength(4); // D1 now draws, D2 does not
    expect(order[order.length - 1]).toMatch(/^TextFrame/);
    expect(await spread()).not.toBe(before);

    await raw.document.undo(); // one step: text, bars and stamp
    await settle();
    expect(await spread()).toBe(before);
    expect((await exportedTableCells(h))[0].get("1:0")).toBe("40");
  });

  it("with 66 `minted`, the bars' ids come back without a scene-tree read", async () => {
    const tally = new Tally();
    const { host } = countingHost(withDoors66(h, raw, { minted: true }), tally);
    const session = await placeBars(host);
    // The placement learned that the host sends `minted` from its first
    // batch, so the content batch drew the bars without a tree read.
    expect(tally.work.count("document.tree")).toBe(0);
    session.editCell(0, 1, 3, "5");
    await session.refreshPlacements();
    await settle();
    expect(tally.work.count("document.tree")).toBe(0);
    expect((await stack()).filter((x) => x.startsWith("Polygon"))).toHaveLength(4);
  });
});
