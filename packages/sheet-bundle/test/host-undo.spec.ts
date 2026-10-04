// Wave 5 — host Cmd-Z reaches sheet edits OUTSIDE the modal session
// (ADR-012 Tier 2), against a REAL headless host (core 0.64 via
// canvas-wasm: real document undo, real plugin metadata) and the REAL
// sheet engine:
//   · an in-frame session's edits land on the page as ONE refresh on exit
//     (refreshes are held while the frame is entered);
//   · undoing that refresh with the DOCUMENT's undo takes the workbook back
//     with it (the binding's content version drops → the journal unwinds);
//     redo brings both forward again;
//   · inside the next session, Cmd-Z stops at that session's start.
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
  openHost,
  settle,
  sheetHost,
  withSceneChannel,
} from "./perf/workload";

if (process.env.REQUIRE_REAL_ENGINE === "1" && !ENGINE_BUILT) {
  describe("host undo (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing`);
    });
  });
}

describe.skipIf(!ENGINE_BUILT)("host undo reaches the workbook [sheet.edit.undo]", () => {
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

  async function placed(): Promise<{ s: WorkbookSession; frame: string }> {
    const session = createWorkbookSession(host);
    s = session;
    await session.import(await authorWorkbook(3, 2, (r, c) => String(r * 2 + c + 1)), "u.xlsx");
    session.setRange("A1:B3");
    const frame = await session.lowerSelection();
    expect(frame).not.toBeNull();
    await settle();
    return { s: session, frame: frame! };
  }

  const shownVersion = async (frame: string) =>
    parseBinding(await host.document.getMetadata({ kind: "textFrame", id: frame } as never))
      ?.data.contentVersion;

  it("a session's edits refresh the page once on exit; document undo takes the workbook back", async () => {
    const { s, frame } = await placed();
    const v0 = await shownVersion(frame);
    expect(await s.showGridInFrame(frame)).toBe(true);
    // Two committed cell edits in the session.
    s.selectCell(0, 0);
    for (const ch of "10") s.handleGridKey({ key: ch });
    s.handleGridKey({ key: "Enter" });
    for (const ch of "20") s.handleGridKey({ key: ch });
    s.handleGridKey({ key: "Enter" });
    expect([s.cellInputAt(0, 0), s.cellInputAt(1, 0)]).toEqual(["10", "20"]);
    // Held while the frame is entered: the page still shows v0.
    await new Promise((r) => setTimeout(r, 400));
    await settle();
    expect(await shownVersion(frame)).toBe(v0);
    // In-session Cmd-Z stops at the session's start.
    expect(s.undoCellEdit()).toBe(true);
    expect(s.redoCellEdit()).toBe(true);
    // Exit: ONE refresh lands the session.
    s.hideGridInFrame();
    await s.refreshPlacements();
    await settle();
    const v1 = await shownVersion(frame);
    expect(v1).toBeGreaterThan(v0!);

    // The DOCUMENT's undo, step by step, until the binding is back at v0:
    // the workbook follows the moment it is.
    for (let i = 0; i < 8 && (await shownVersion(frame)) !== v0; i++) {
      await host.document.undo();
      await settle();
    }
    expect(await shownVersion(frame)).toBe(v0);
    await settle();
    expect([s.cellInputAt(0, 0), s.cellInputAt(1, 0)]).toEqual(["1", "3"]);

    // Redo brings both forward.
    for (let i = 0; i < 8 && (await shownVersion(frame)) !== v1; i++) {
      await host.document.redo();
      await settle();
    }
    await settle();
    expect([s.cellInputAt(0, 0), s.cellInputAt(1, 0)]).toEqual(["10", "20"]);
  });

  it("inside a new session, Cmd-Z stops at the session's start", async () => {
    const { s, frame } = await placed();
    s.editCell(0, 2, 1, "99"); // before the session — the document's history
    expect(await s.showGridInFrame(frame)).toBe(true);
    expect(s.canUndoCellEdit()).toBe(false);
    expect(s.undoCellEdit()).toBe(false);
    s.selectCell(0, 1);
    s.handleGridKey({ key: "7" });
    s.handleGridKey({ key: "Enter" });
    expect(s.undoCellEdit()).toBe(true);
    expect(s.cellInputAt(0, 1)).toBe("2");
    expect(s.undoCellEdit()).toBe(false); // the floor
    expect(s.cellInputAt(2, 1)).toBe("99");
    s.hideGridInFrame();
    // Outside the frame (the grid panel) the whole journal is reachable.
    expect(s.undoCellEdit()).toBe(true);
    expect(s.cellInputAt(2, 1)).toBe("6");
  });
});
