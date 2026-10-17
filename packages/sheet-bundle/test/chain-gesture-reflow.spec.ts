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

// A range paginated over a frame chain re-splits when a chain frame is
// resized by its HANDLES: a canvas gesture (begin, update, commit), which
// the engine commits as a `gestureCommitted`, not a `resizeFrame` mutation.
// Core v0.71.13 reports that gesture's reflow on the reply (core ADR 159),
// plugin-sdk 0.2.58 passes it through `document.onDidChange`, and the
// chain subscription re-paginates on any change that carries one. Before,
// only a `resizeFrame` carried it, and a handle resize left the split
// where it was. A move by the same gesture kit carries none and changes
// nothing (display geometry only).
//
// Real engine: skipped without the wasm artifact, FAILS under
// REQUIRE_REAL_ENGINE=1 (the engine-real.spec.ts dual gate).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";
import type { HeadlessHost } from "@paged-media/plugin-sdk";

import { createWorkbookSession, storyOfFrame, type WorkbookSession } from "../src";
import { CHAIN_REFLOW_DEBOUNCE_MS } from "../src/lower";
import {
  ENGINE_BUILT,
  WASM,
  authorWorkbook,
  blankPageIdml,
  exportedTableRows,
  openHost,
  resizeByHandle,
  settle,
  sheetHost,
  withSceneChannel,
} from "./perf/workload";

if (process.env.REQUIRE_REAL_ENGINE === "1" && !ENGINE_BUILT) {
  describe("chain re-split on a handle resize (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing`);
    });
  });
}

/** Wait out the chain's reflow debounce and the pass it schedules. */
async function afterDebounce(): Promise<void> {
  await new Promise((r) => setTimeout(r, CHAIN_REFLOW_DEBOUNCE_MS + 50));
  for (let i = 0; i < 20; i++) await settle();
}

describe.skipIf(!ENGINE_BUILT)("a paginated range follows a handle resize [sheet.lower.paginate]", () => {
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

  /** Two threaded frames side by side on the page, a 40-row range
   *  paginated over them; returns the first frame's id. */
  async function paginatedChain(): Promise<string> {
    const frame = async (bounds: [number, number, number, number]) => {
      const o = await host.document.mutate({ op: "insertTextFrame", args: { pageId: "usp", bounds } });
      expect(o.applied).toBe(true);
      return (o as { createdId: { id: string } }).createdId.id;
    };
    const first = await frame([36, 36, 556, 296]);
    const second = await frame([36, 316, 556, 576]);
    expect((await host.document.mutate({ op: "linkFrames", args: { from: first, to: second } })).applied).toBe(true);
    const storyId = await storyOfFrame(host, first);
    expect(storyId, "the chain's story").toBeTruthy();
    expect(await host.document.frameChain(storyId!)).toHaveLength(2);

    const session = createWorkbookSession(host);
    s = session;
    await session.import(await authorWorkbook(40, 2, (r, c) => `r${r + 1}c${c + 1}`), "chain.xlsx");
    session.setRange("A1:B40");
    await host.selection.set([{ kind: "textFrame", id: first }]);
    expect(await session.paginateSelection()).toEqual({ ok: true });
    await settle();
    return first;
  }

  it("rows move into the second frame when the first is shortened by its handle", async () => {
    const first = await paginatedChain();
    const before = await exportedTableRows(h);
    expect(before, "one table per chain frame").toHaveLength(2);
    expect(before[0].rows).toBeGreaterThan(0);
    expect(before[1].rows).toBeGreaterThan(0);

    // The first frame's bottom edge up by 200 pt, dragged by its handle.
    await resizeByHandle(h, first, "south", [0, -200]);
    await afterDebounce();

    const after = await exportedTableRows(h);
    expect(after).toHaveLength(2);
    expect(after[0].rows, "the first frame holds fewer rows").toBeLessThan(before[0].rows);
    expect(after[1].rows, "the rows it lost moved into the second").toBeGreaterThan(before[1].rows);
    expect(after[0].rows + after[1].rows).toBe(before[0].rows + before[1].rows);
    // Refreshed in place: the same two tables.
    expect(after.map((t) => t.self)).toEqual(before.map((t) => t.self));
  });

  it("a move by the gesture kit leaves the split alone", async () => {
    const first = await paginatedChain();
    const before = await exportedTableRows(h);
    const client = (h.host as unknown as {
      editor: { client: { send(m: unknown): Promise<{ kind: string; payload: Record<string, unknown> }> } };
    }).editor.client;
    const begun = await client.send({
      kind: "beginGesture",
      payload: { nodes: [{ kind: "textFrame", id: first }], gesture: { kind: "translate" } },
    });
    expect(begun.kind).toBe("gestureBegun");
    await client.send({
      kind: "updateGesture",
      payload: { handle: begun.payload.handle, delta: [0, 40], modifiers: { shift: false, alt: false, disableSnap: true } },
    });
    const committed = await client.send({ kind: "commitGesture", payload: { handle: begun.payload.handle } });
    expect(committed.kind).toBe("gestureCommitted");
    expect(committed.payload.reflow ?? null, "a move reports no reflow").toBeNull();
    await afterDebounce();
    expect(await exportedTableRows(h)).toEqual(before);
  });
});
