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

import { createWorkbookSession, type WorkbookSession } from "../src";

import {
  ENGINE_BUILT,
  WASM,
  authorWorkbook,
  blankPageIdml,
  exportedTableCells,
  openHost,
  settle,
  sheetHost,
  withSceneChannel,
} from "./perf/workload";

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
