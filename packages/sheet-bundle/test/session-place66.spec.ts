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

// The ONE-CALL placement (protocol 66: core resolves a table handle in a
// `tableId` / `table_id` position, 0eff96b). On a 66 host the frame, its
// binding, the table, the pour, the decor and the text styles are one
// `mutate`. On the real 0.64 core under test it is never sent unless the
// host shows a 66 door; when it is sent anyway and refused, the placement
// falls back to the two-batch path and remembers the refusal.
//
// The 66 host is `withTableHandles66` (test/perf/workload.ts) over the REAL
// headless core: the engine applies what 66 would, the bundle makes one call.
//
// Real engine: skipped without the wasm artifact, FAILS under
// REQUIRE_REAL_ENGINE=1 (the engine-real.spec.ts dual gate).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";
import type { HeadlessHost } from "@paged-media/plugin-sdk";
import { parseBinding } from "@paged-media/sheet-host-model";

import { createWorkbookSession, type WorkbookSession } from "../src";
import { countingHost, Tally } from "./perf/counting-host";
import {
  CORE_PRE_66,
  ENGINE_BUILT,
  WASM,
  authorWorkbook,
  blankPageIdml,
  exportedTableCells,
  openHost,
  settle,
  sheetHost,
  withDoors66,
  withSceneChannel,
  withTableHandles66,
} from "./perf/workload";

if (process.env.REQUIRE_REAL_ENGINE === "1" && !ENGINE_BUILT) {
  describe("one-call placement (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing`);
    });
  });
}

/** Every text frame in the scene tree. */
async function textFrames(host: BundleHost): Promise<string[]> {
  const out: string[] = [];
  const walk = (ns: readonly { id?: { kind: string; id: unknown } | null; children?: unknown[] }[]) => {
    for (const n of ns) {
      if (n.id?.kind === "textFrame") out.push(n.id.id as string);
      if (n.children) walk(n.children as never);
    }
  };
  walk((await host.document.tree()) as never);
  return out;
}

describe.skipIf(!ENGINE_BUILT)("one-call placement [sheet.lower.page]", () => {
  vi.setConfig({ testTimeout: 60_000 });
  let h: HeadlessHost;
  let raw: BundleHost;
  const sessions: WorkbookSession[] = [];

  beforeEach(async () => {
    h = await openHost();
    await h.load(blankPageIdml());
    raw = withSceneChannel(sheetHost(h)).host;
  });
  afterEach(() => {
    for (const x of sessions.splice(0)) x.dispose();
    h?.dispose();
  });

  async function placeOn(host: BundleHost, label = (r: number, c: number) => `${r}${c}`) {
    const s = createWorkbookSession(host);
    sessions.push(s);
    await s.import(await authorWorkbook(3, 2, label), "p.xlsx");
    s.setRange("A1:B3");
    return s;
  }

  it("a 66 host places frame, table and content in ONE mutate", async () => {
    const tally = new Tally();
    const { host } = countingHost(withTableHandles66(h, raw), tally);
    const s = await placeOn(host);
    tally.work.reset();
    const frame = await s.lowerSelection();
    await settle();
    expect(frame).not.toBeNull();

    expect(tally.work.count("document.mutate")).toBe(1);
    expect(tally.work.rejected).toBe(0);
    const [m] = tally.work.mutations;
    expect(m.kinds.insertTextFrame).toBe(1);
    expect(m.kinds.insertTable).toBe(1);
    expect(m.kinds.bindCreated).toBe(2);
    expect(m.kinds.insertText).toBe(6);
    // No read-back of the frame or its story: `minted` names both.
    expect(tally.work.count("document.frameChain")).toBe(0);

    const tables = await exportedTableCells(h);
    expect(tables).toHaveLength(1);
    expect(tables[0].get("1:1")).toBe("11");
    expect(tables[0].get("2:1")).toBe("21");
    // The binding rode the batch: the table record without ids (they did
    // not exist when it was written).
    const b = parseBinding(await raw.document.getMetadata({ kind: "textFrame", id: frame! } as never));
    expect(b?.data.table).toBeDefined();
    expect(b?.data.table?.tableId).toBeUndefined();

    // This session knows the ids from `minted`: an edit refreshes in place.
    s.editCell(0, 2, 1, "edited");
    await s.refreshPlacements();
    await settle();
    const after = await exportedTableCells(h);
    expect(after).toHaveLength(1);
    expect(after[0].get("2:1")).toBe("edited");
    // ...and the refresh re-stamped the record with its ids.
    const b2 = parseBinding(await raw.document.getMetadata({ kind: "textFrame", id: frame! } as never));
    expect(b2?.data.table?.tableId).toBeTruthy();
  });

  it("a later session finds a one-call table by reading the page [sheet.plugin.persistence]", async () => {
    const a = await placeOn(withTableHandles66(h, raw));
    const frame = await a.lowerSelection();
    await settle();
    await a.flushPersist();
    a.dispose();
    expect(
      parseBinding(await raw.document.getMetadata({ kind: "textFrame", id: frame! } as never))
        ?.data.table?.tableId,
    ).toBeUndefined();

    const b = createWorkbookSession(raw);
    sessions.push(b);
    expect(await b.restore()).toBe(true);
    b.editCell(0, 2, 1, "later");
    await b.refreshPlacements();
    await settle();
    const tables = await exportedTableCells(h);
    expect(tables).toHaveLength(1);
    expect(tables[0].get("2:1")).toBe("later");
    expect(tables[0].get("1:1")).toBe("11");
    // Found (its hash matched) and refreshed IN PLACE: the same frame, not
    // a re-placement.
    expect(await textFrames(raw)).toEqual([frame]);
    // ...and the refresh re-stamped the record with the ids it read.
    expect(
      parseBinding(await raw.document.getMetadata({ kind: "textFrame", id: frame! } as never))
        ?.data.table?.tableId,
    ).toBeTruthy();
  });

  // These two pin a PRE-66 core (0.64): on 66+ the door answers.
  it.skipIf(!CORE_PRE_66)("the real 0.64 core with no 66 door: never sent, two mutates as before", async () => {
    const tally = new Tally();
    const { host } = countingHost(raw, tally);
    const s = await placeOn(host);
    tally.work.reset();
    expect(await s.lowerSelection()).not.toBeNull();
    await settle();
    expect(tally.work.count("document.mutate")).toBe(2);
    expect(tally.work.rejected).toBe(0);
    expect(tally.work.mutations.some((m) => (m.kinds.bindCreated ?? 0) > 1)).toBe(false);
  });

  it.skipIf(!CORE_PRE_66)("a 66 signal over the real 0.64 core: refused once, rolled back, two-batch placement, remembered", async () => {
    // `minted` is a 66 door; 0.64 core does not resolve the table handle,
    // so the one-call batch is refused and the placement must still land
    // exactly once.
    const tally = new Tally();
    const { host } = countingHost(withDoors66(h, raw, { minted: true }), tally);
    const s = await placeOn(host);
    // The first placement knows nothing yet (two batches); its outcome
    // carries `minted` — the 66 signal.
    expect(await s.lowerSelection()).not.toBeNull();
    await settle();
    expect(tally.work.rejected).toBe(0);
    const tablesBefore = (await exportedTableCells(h)).length;
    const treeBefore = JSON.stringify(await raw.document.tree());
    const framesBefore = await textFrames(raw);
    tally.work.reset();

    const frame = await s.lowerSelection();
    await settle();
    expect(frame).not.toBeNull();
    // 1 refused one-call batch, then the two-batch placement.
    expect(tally.work.count("document.mutate")).toBe(3);
    expect(tally.work.rejected).toBe(1);
    // Exactly one new frame and one table, holding the range.
    expect(await textFrames(raw)).toEqual([...framesBefore, frame]);
    const tables = await exportedTableCells(h);
    expect(tables).toHaveLength(tablesBefore + 1);
    expect(tables[tablesBefore].get("2:1")).toBe("21");
    // Two undo steps (the two batches) and the page is as it was: the
    // refused attempt left no step behind. (Core 0.64 does leave the
    // refused frame's empty STORY in its model — a rolled-back or undone
    // insertTextFrame keeps its ParentStory; it has no frame and is not
    // exported. Core defect, reported; not asserted here.)
    await raw.document.undo();
    await raw.document.undo();
    await settle();
    expect(JSON.stringify(await raw.document.tree())).toBe(treeBefore);
    expect(await exportedTableCells(h)).toHaveLength(tablesBefore);

    // Remembered: the next placement does not try again.
    tally.work.reset();
    s.setRange("A1:A2");
    expect(await s.lowerSelection()).not.toBeNull();
    await settle();
    expect(tally.work.count("document.mutate")).toBe(2);
    expect(tally.work.rejected).toBe(0);
  });
});
