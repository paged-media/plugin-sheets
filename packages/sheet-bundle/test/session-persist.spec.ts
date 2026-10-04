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

// [sheet.plugin.persistence] DATA LOSS (found 2026-10-04): edits were
// never written back. The workbook part / blob was written only on
// import, so a reload restored the last IMPORTED bytes and every cell
// edit, paste, sort or replace since was gone. These specs edit a
// workbook, end the session the two ways a session ends (the debounce
// elapsing; the bundle deactivating), then restore a NEW session over the
// same document and require the edit to be there.
//
// Real engine (the round trip is engine bytes): same dual gate as
// engine-real.spec.ts — skipped without the artifact, FAILS under
// REQUIRE_REAL_ENGINE=1.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterEach, describe, expect, it, vi } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";

import { createWorkbookSession } from "../src/session";

const HERE = dirname(fileURLToPath(import.meta.url));
const WASM = join(HERE, "..", "bin", "sheet_js_bg.wasm");
const built = existsSync(WASM);
const FIXTURE = join(HERE, "..", "..", "..", "corpus/xlsx-corpus/01-minimal.xlsx");

if (process.env.REQUIRE_REAL_ENGINE === "1" && !built) {
  describe("session persistence (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing — run scripts/build-wasm.sh`);
    });
  });
}

/** One DOCUMENT: an in-memory container-parts store + blob store that
 *  successive sessions (= reloads) share. */
function fakeDocument() {
  const parts = new Map<string, Uint8Array>();
  const blobs = new Map<string, Uint8Array>();
  const kv = new Map<string, unknown>();
  const partWrites = vi.fn();
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) => f === "storage.parts@1" || f === "storage.blob@1",
    parts: {
      read: async (p: string) => parts.get(p) ?? null,
      write: async (p: string, b: Uint8Array) => {
        partWrites(p);
        parts.set(p, b.slice());
      },
      list: async () => [...parts.keys()],
    },
    blob: {
      read: async (k: string) => blobs.get(k) ?? null,
      write: async (k: string, b: Uint8Array) => void blobs.set(k, b.slice()),
      delete: async (k: string) => void blobs.delete(k),
      keys: async () => [...blobs.keys()],
      usage: async () => ({ used: 0, quota: 0 }),
    },
    storage: {
      get: (k: string) => kv.get(k),
      set: (k: string, v: unknown) => void kv.set(k, v),
      delete: (k: string) => void kv.delete(k),
      keys: () => [...kv.keys()],
    },
  } as unknown as BundleHost;
  const workbookWrites = () =>
    partWrites.mock.calls.filter(([p]) => p === "workbook.xlsx").length;
  return { host, workbookWrites };
}

async function reload(host: BundleHost) {
  const next = createWorkbookSession(host);
  expect(await next.restore()).toBe(true);
  return next;
}

describe.skipIf(!built)("sheet edits persist [sheet.plugin.persistence]", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("edit, let the debounce elapse, reload: the value and the formula survive", async () => {
    const doc = fakeDocument();
    const s1 = createWorkbookSession(doc.host);
    await s1.import(new Uint8Array(readFileSync(FIXTURE)), "minimal.xlsx");
    expect(doc.workbookWrites()).toBe(1); // the import itself

    vi.useFakeTimers();
    expect(s1.editCell(0, 9, 0, "42")).toBe(true);
    expect(s1.editCell(0, 9, 1, "=A10*2")).toBe(true);
    // Debounced: two edits in quick succession are ONE write, after the delay.
    expect(doc.workbookWrites()).toBe(1);
    await vi.advanceTimersByTimeAsync(5_000);
    vi.useRealTimers();
    await vi.waitFor(() => expect(doc.workbookWrites()).toBe(2));

    const s2 = await reload(doc.host);
    expect(s2.state().engine!.getCellInput(0, 9, 0)).toBe("42");
    expect(s2.state().engine!.getCellInput(0, 9, 1)).toBe("=A10*2");
    expect(s2.state().engine!.getCellDisplay(0, 9, 1)).toBe("84");
    s1.dispose();
    s2.dispose();
  });

  it("edit, deactivate before the debounce fires, reload: the edit is flushed, not dropped", async () => {
    const doc = fakeDocument();
    const s1 = createWorkbookSession(doc.host);
    await s1.import(new Uint8Array(readFileSync(FIXTURE)), "minimal.xlsx");
    expect(s1.editCell(0, 9, 0, "flushed")).toBe(true);
    s1.dispose(); // the bundle deactivates (or the editor closes the doc)
    await vi.waitFor(() => expect(doc.workbookWrites()).toBe(2));

    const s2 = await reload(doc.host);
    expect(s2.state().engine!.getCellInput(0, 9, 0)).toBe("flushed");
    s2.dispose();
  });

  it("leaving the frame flushes: a save right after the exit carries the edit", async () => {
    // The host has no will-save hook yet, so a document saved inside the
    // debounce window shipped the PRE-edit workbook part — and reopening it
    // then re-lowered the placed table from the stale workbook (the editor
    // journey sheet-edit-persist, saving right after Esc). Exiting the
    // frame is where the session's edits reach the page; the part follows
    // at the same moment.
    const doc = fakeDocument();
    const s1 = createWorkbookSession(doc.host);
    await s1.import(new Uint8Array(readFileSync(FIXTURE)), "minimal.xlsx");
    vi.useFakeTimers();
    expect(s1.editCell(0, 9, 0, "on exit")).toBe(true);
    s1.hideGridInFrame(); // the context exits (Esc) — no timer has run
    vi.useRealTimers();
    await vi.waitFor(() => expect(doc.workbookWrites()).toBe(2));

    const s2 = await reload(doc.host);
    expect(s2.state().engine!.getCellInput(0, 9, 0)).toBe("on exit");
    s1.dispose();
    s2.dispose();
  });

  it("an undo is an edit too: undone value is what reloads", async () => {
    const doc = fakeDocument();
    const s1 = createWorkbookSession(doc.host);
    await s1.import(new Uint8Array(readFileSync(FIXTURE)), "minimal.xlsx");
    const before = s1.state().engine!.getCellInput(0, 0, 0);
    expect(s1.editCell(0, 0, 0, "changed")).toBe(true);
    expect(s1.undoCellEdit()).toBe(true);
    s1.dispose();
    await vi.waitFor(() => expect(doc.workbookWrites()).toBe(2));

    const s2 = await reload(doc.host);
    expect(s2.state().engine!.getCellInput(0, 0, 0)).toBe(before);
    s2.dispose();
  });

  it("no edit, no write: a session that only reads leaves the stored bytes alone", async () => {
    const doc = fakeDocument();
    const s1 = createWorkbookSession(doc.host);
    await s1.import(new Uint8Array(readFileSync(FIXTURE)), "minimal.xlsx");
    s1.dispose();
    await new Promise((r) => setTimeout(r, 50));
    expect(doc.workbookWrites()).toBe(1);
  });
});
