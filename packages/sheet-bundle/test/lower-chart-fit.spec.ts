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

// A placed chart reads as one sized, legible object — read back off the
// REAL engine.
//
// Found filming the editor (Workbook ▸ Charts ▸ Place on page): the chart
// landed as 18 loose items on top of the table placed a moment before; the
// title's frame was 80 pt wide, so "Amount by item" broke and its last word
// was overset; the value-axis labels sat at x = −20 pt, off the page; the
// category labels ran together ("NotebooksPencils"). Every label frame was
// a fixed 80 pt box, left-aligned at the label's anchor, and its text kept
// the document's 12 pt rather than the chart's 8–10 pt.
//
// The host here measures with the ENGINE's shaper (`engineShaper`), as the
// editor's does, so a label sized from a measurement is sized from the face
// the engine lays it out in.
//
// Real engine: skipped without the wasm artifact, FAILS under
// REQUIRE_REAL_ENGINE=1 (the engine-real.spec.ts dual gate).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { BundleHost, ElementId, SceneTreeNode } from "@paged-media/plugin-api";
import { createHeadlessHost, type HeadlessHost } from "@paged-media/plugin-sdk";

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { createWorkbookSession, type WorkbookSession } from "../src";
import {
  ENGINE_BUILT,
  WASM,
  blankPageIdml,
  exportedIdmlParts,
  settle,
  sheetHost,
  withSceneChannel,
} from "./perf/workload";

const HERE = dirname(fileURLToPath(import.meta.url));
const corpus = (name: string) =>
  new Uint8Array(readFileSync(join(HERE, "..", "..", "..", "corpus/xlsx-corpus", name)));

if (process.env.REQUIRE_REAL_ENGINE === "1" && !ENGINE_BUILT) {
  describe("placed-chart fit (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing`);
    });
  });
}

const silent = { debug: () => {}, info: () => {}, warn: () => {}, error: () => {} };
const mapBacking = () => {
  const m = new Map<string, string>();
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    removeItem: (k: string) => void m.delete(k),
    keys: () => Array.from(m.keys()),
  };
};

type Box = [number, number, number, number]; // top, left, bottom, right

const key = (id: ElementId) => `${id.kind}:${String(id.id)}`;

/** Every node of the scene tree, keyed, with its parent's key. */
function nodes(roots: readonly SceneTreeNode[]): Map<string, { id: ElementId; parent: string | null; children: string[] }> {
  const out = new Map<string, { id: ElementId; parent: string | null; children: string[] }>();
  const walk = (ns: readonly SceneTreeNode[], parent: string | null) => {
    for (const n of ns) {
      const id = n.id as ElementId | null;
      const k = id ? key(id) : null;
      if (id && k) out.set(k, { id, parent, children: (n.children ?? []).flatMap((c) => (c.id ? [key(c.id as ElementId)] : [])) });
      if (n.children) walk(n.children, k ?? parent);
    }
  };
  walk(roots, null);
  return out;
}

const overlaps = (a: Box, b: Box) => a[1] < b[3] && b[1] < a[3] && a[0] < b[2] && b[0] < a[2];

describe.skipIf(!ENGINE_BUILT)("a placed chart is one sized, legible object", () => {
  vi.setConfig({ testTimeout: 90_000 });
  let h: HeadlessHost;
  let host: BundleHost;
  let s: WorkbookSession | null = null;

  beforeEach(async () => {
    h = await createHeadlessHost({
      console: silent,
      storage: mapBacking(),
      resolveFrom: resolve(HERE, ".."),
      engineShaper: true,
    });
    await h.load(blankPageIdml());
    host = withSceneChannel(sheetHost(h)).host;
  });
  afterEach(() => {
    s?.dispose();
    s = null;
    h?.dispose();
  });

  /** Import 09-chart with the film's long category labels, place the data as
   *  a table (it stays selected), then place the chart. */
  async function placeTableThenChart() {
    const session = createWorkbookSession(host);
    s = session;
    await session.import(corpus("09-chart.xlsx"), "09-chart.xlsx");
    expect(session.editCell(0, 1, 0, "Notebooks")).toBe(true);
    expect(session.editCell(0, 2, 0, "Pencils")).toBe(true);
    expect(session.editCell(0, 3, 0, "Highlighters")).toBe(true);
    session.setRange("A1:B4");
    const table = await session.lowerSelection();
    expect(table).not.toBeNull();
    await settle();
    const before = nodes(await host.document.tree());
    const [chart] = session.listCharts();
    expect(await session.lowerChart(chart.index)).toBe(true);
    await settle();
    const after = nodes(await host.document.tree());
    const added = [...after.values()].filter((n) => !before.has(key(n.id)));
    return { session, table: table!, added, after };
  }

  it("the chart is ONE group holding every element it made [sheet.chart.engine]", async () => {
    const { added, after } = await placeTableThenChart();
    const groups = added.filter((n) => n.id.kind === "group");
    expect(groups, "one group for the whole chart").toHaveLength(1);
    const group = key(groups[0].id);
    expect(after.get(group)!.parent, "the group sits on the page").toBeNull();
    const members = added.filter((n) => n.id.kind !== "group");
    expect(members.length).toBeGreaterThan(5);
    for (const m of members) expect(m.parent, `${key(m.id)} is in the chart's group`).toBe(group);
  });

  it("lands beside what is selected, not on top of it [sheet.chart.engine]", async () => {
    const { table, added } = await placeTableThenChart();
    const [t] = await host.document.elementGeometry([{ kind: "textFrame", id: table }]);
    const boxes = await host.document.elementGeometry(
      added.filter((n) => n.id.kind !== "group").map((n) => n.id),
    );
    for (const g of boxes) {
      expect(overlaps(g.bounds as Box, t.bounds as Box), `${key(g.id)} lies over the placed table`).toBe(false);
    }
  });

  it("every label is sized to its text, set at the chart's size, inside the chart and on the page [sheet.chart.engine]", async () => {
    const { added } = await placeTableThenChart();
    const members = added.filter((n) => n.id.kind !== "group");
    const geom = await host.document.elementGeometry(members.map((n) => n.id));
    const art = geom.filter((g) => g.id.kind !== "textFrame");
    const labels = geom.filter((g) => g.id.kind === "textFrame");
    expect(labels.length).toBeGreaterThan(5);
    // The chart's extent: its vector art (plot frame, axes, bars).
    const left = Math.min(...art.map((g) => g.bounds[1]));
    const right = Math.max(...art.map((g) => g.bounds[3]));
    const [page] = await host.document.collection<{ sizePt: [number, number] }>("pages");
    for (const g of labels) {
      const [, l, , r] = g.bounds;
      expect(l, `${key(g.id)} starts on the page`).toBeGreaterThanOrEqual(0);
      expect(r, `${key(g.id)} ends on the page`).toBeLessThanOrEqual(page.sizePt[0]);
      // The value-axis labels hang left of the plot frame, inside the chart.
      expect(l, `${key(g.id)} starts inside the chart`).toBeGreaterThanOrEqual(left - 40);
      expect(r, `${key(g.id)} ends inside the chart`).toBeLessThanOrEqual(right + 12);
      const storyId = (g as { storyId?: string | null }).storyId;
      expect(storyId).toBeTruthy();
      const chain = await host.document.frameChain(storyId!);
      expect(chain.some((c) => c.overflow), `${key(g.id)}'s label is overset`).toBe(false);
    }
    // Labels on one line (the category axis) never run into each other.
    const byBaseline = new Map<number, Box[]>();
    for (const g of labels) {
      const b = Math.round(g.bounds[2]);
      byBaseline.set(b, [...(byBaseline.get(b) ?? []), g.bounds as Box]);
    }
    for (const row of byBaseline.values()) {
      for (let i = 0; i < row.length; i++) {
        for (let j = i + 1; j < row.length; j++) {
          expect(overlaps(row[i], row[j]), "two labels on one line overlap").toBe(false);
        }
      }
    }
    // The text carries the chart's sizes (8 pt ticks, a 10 pt title), not
    // the document's 12 pt.
    const stories = [...(await exportedIdmlParts(h))]
      .filter(([n]) => n.startsWith("Stories/"))
      .map(([, x]) => x);
    const title = stories.find((x) => x.includes("<Content>Q1 Revenue</Content>"));
    expect(title, "the title's story").toBeDefined();
    expect(title).toMatch(/PointSize="10"/);
    expect(title).toMatch(/Justification="CenterAlign"/);
    const tick = stories.find((x) => x.includes("<Content>Notebooks</Content>") && !x.includes("<Table"));
    expect(tick, "a category label's story").toBeDefined();
    expect(tick).toMatch(/PointSize="8"/);
  });

  it("a data change still redraws it: one group, nothing of the old chart left [sheet.chart.engine]", async () => {
    const { session, added } = await placeTableThenChart();
    const spreads = async () =>
      [...(await exportedIdmlParts(h))]
        .filter(([n]) => n.startsWith("Spreads/"))
        .map(([, x]) => x)
        .join("\n");
    const art = await spreads();
    const before = nodes(await host.document.tree());
    expect(session.editCell(0, 2, 1, "45")).toBe(true);
    await session.refreshPlacements();
    await settle();
    const after = nodes(await host.document.tree());
    // Redrawn: different art, the same number of elements (the old chart is
    // replaced, never left beside the new one; the engine re-mints ids, so
    // the count is the test, not the names).
    expect(await spreads()).not.toBe(art);
    expect(after.size).toBe(before.size);
    const groups = [...after.values()].filter((n) => n.id.kind === "group");
    expect(groups).toHaveLength(1);
    expect(groups[0].children.length).toBe(added.length - 1);
    // No group anywhere is left empty.
    for (const n of after.values()) {
      if (n.id.kind === "group") expect(n.children.length, `${key(n.id)} is empty`).toBeGreaterThan(0);
    }
  });
});
