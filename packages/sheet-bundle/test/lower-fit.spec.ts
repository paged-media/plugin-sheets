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

// A placed table range lands in a frame sized to the rows it holds — read
// back off the REAL engine, which is the only judge of what fits.
//
// Found filming the editor: a 7-row price list placed as A1:D7 got a 105 pt
// frame (7 × the workbook's 15 pt rows) and the last two rows were overset
// ("1 overset story" in Publication health). The engine lays the table out
// one line BELOW the top of the frame — `insertTable` appends the table on a
// fresh paragraph after the story's empty first one — so a frame as tall as
// the rows cannot hold them.
//
// Real engine: skipped without the wasm artifact, FAILS under
// REQUIRE_REAL_ENGINE=1 (the engine-real.spec.ts dual gate).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";
import type { HeadlessHost } from "@paged-media/plugin-sdk";

import { createWorkbookSession, type WorkbookSession } from "../src";
import {
  ENGINE_BUILT,
  WASM,
  authorWorkbook,
  blankPageIdml,
  openHost,
  settle,
  sheetHost,
  withSceneChannel,
} from "./perf/workload";

if (process.env.REQUIRE_REAL_ENGINE === "1" && !ENGINE_BUILT) {
  describe("placed-table fit (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing`);
    });
  });
}

/** The film's price list: a header, five items with `Qty × Price`, a total. */
const PRICE_LIST: (string | null)[][] = [
  ["Item", "Qty", "Price", "Amount"],
  ["Notebooks", "40", "3.5", "=B2*C2"],
  ["Pencils", "120", "0.8", "=B3*C3"],
  ["Erasers", "60", "1.2", "=B4*C4"],
  ["Rulers", "30", "2.4", "=B5*C5"],
  ["Markers", "80", "1.9", "=B6*C6"],
  ["Total", null, null, "=SUM(D2:D6)"],
];

describe.skipIf(!ENGINE_BUILT)("a placed table range is sized to its rows", () => {
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

  /** The placed frame's box and whether its story is overset. */
  async function placed(frame: string) {
    const [g] = await host.document.elementGeometry([{ kind: "textFrame", id: frame }]);
    const storyId = (g as { storyId?: string | null }).storyId;
    expect(storyId, "the frame's story").toBeTruthy();
    const chain = await host.document.frameChain(storyId!);
    return { bounds: g.bounds, overset: chain.some((l) => l.overflow), storyId: storyId! };
  }

  /** Whether the frame's story is overset at `height` (pt). */
  async function oversetAt(frame: string, top: number, left: number, right: number, height: number) {
    const r = await host.document.mutate({
      op: "setElementProperty",
      args: {
        elementId: { kind: "textFrame", id: frame },
        path: "frameBounds",
        value: { type: "bounds", value: [top, left, top + height, right] },
      },
    });
    expect(r.applied).toBe(true);
    const { overset } = await placed(frame);
    await host.document.undo();
    return overset;
  }

  it("A1:D7 of the price list shows every row, in a frame no taller than its rows [sheet.lower.page]", async () => {
    const session = createWorkbookSession(host);
    s = session;
    await session.import(
      await authorWorkbook(7, 4, (r, c) => PRICE_LIST[r][c]),
      "price-list.xlsx",
    );
    session.setRange("A1:D7");
    const frame = await session.lowerSelection();
    expect(frame).not.toBeNull();
    await settle();

    const { bounds, overset } = await placed(frame!);
    // Every row shows: nothing is overset (the film's "1 overset story").
    expect(overset, "the placed table is overset").toBe(false);
    // ...and the frame is sized TO the rows, not merely large: one point
    // shorter and the last row no longer fits.
    const [top, left, bottom, right] = bounds;
    expect(await oversetAt(frame!, top, left, right, bottom - top - 1)).toBe(true);
  });

  it("a single-row range fits too — the lead-in is per frame, not per row [sheet.lower.page]", async () => {
    const session = createWorkbookSession(host);
    s = session;
    await session.import(await authorWorkbook(1, 2, (_r, c) => `cell ${c}`), "one.xlsx");
    session.setRange("A1:B1");
    const frame = await session.lowerSelection();
    expect(frame).not.toBeNull();
    await settle();
    const { bounds, overset } = await placed(frame!);
    expect(overset).toBe(false);
    const [top, left, bottom, right] = bounds;
    expect(await oversetAt(frame!, top, left, right, bottom - top - 1)).toBe(true);
  });
});
