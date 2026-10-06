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

// [sheet.plugin.persistence] DEFECT (found 2026-10-04 by the editor journey
// sheet-edit-persist): a reopened `.paged` kept its placed values but NOT
// its workbook. The bundle read its container part only when it ACTIVATED,
// at app boot, before any document was open — so a document opened later
// (File ▸ Open, a fresh editor) never had its workbook restored, and
// entering its sheet frame logged "showGridInFrame: no workbook / sheet".
//
// The workbook belongs to the DOCUMENT: it is restored when a document
// opens (`host.document.onDidOpen`; the client's raw `documentLoaded`
// broadcast on an older host), lazily when a sheet frame is entered with
// none loaded, and a second document never sees the first one's workbook.
//
// Real engine (the round trip is engine bytes): skipped without the
// artifact, FAILS under REQUIRE_REAL_ENGINE=1.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it, vi } from "vitest";

import type {
  BundleHost,
  EditContextContribution,
  ExporterContribution,
  SceneLayer,
  WorkerToMain,
} from "@paged-media/plugin-api";

import { activate } from "../src/activate";
import { bootEngine } from "../src/engine";

const HERE = dirname(fileURLToPath(import.meta.url));
const WASM = join(HERE, "..", "bin", "sheet_js_bg.wasm");
const built = existsSync(WASM);
const FIXTURE = join(HERE, "..", "..", "..", "corpus/xlsx-corpus/01-minimal.xlsx");

if (process.env.REQUIRE_REAL_ENGINE === "1" && !built) {
  describe("session document open (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error(`REQUIRE_REAL_ENGINE=1 but ${WASM} is missing — run scripts/build-wasm.sh`);
    });
  });
}

/** The fixture with A10 set to `marker` — a workbook we can recognise. */
async function workbookWith(marker: string): Promise<Uint8Array> {
  const engine = await bootEngine();
  engine.loadXlsx(new Uint8Array(readFileSync(FIXTURE)));
  engine.setCell(0, 9, 0, marker);
  const bytes = engine.saveXlsx();
  engine.dispose();
  return bytes;
}

/** An editor that opens documents one after another. Each document has its
 *  own container parts; the per-browser blob is shared by all of them. When
 *  `broadcasts`, every open is announced: through `document.onDidOpen`
 *  (`document.onDidOpen@1`), or — `rawOnly`, an older host — only on the
 *  client's raw `documentLoaded` broadcast. */
function fakeEditor(opts: { broadcasts?: boolean; rawOnly?: boolean } = {}) {
  const door = !!opts.broadcasts && !opts.rawOnly;
  const opened = new Set<(e: unknown) => void>();
  const docs = new Map<string, Map<string, Uint8Array>>();
  let current = new Map<string, Uint8Array>(); // no document yet
  const blobs = new Map<string, Uint8Array>();
  const kv = new Map<string, unknown>();
  const listeners = new Set<(m: WorkerToMain) => void>();
  const exporters: ExporterContribution[] = [];
  const editContexts: EditContextContribution[] = [];
  const submits: SceneLayer[] = [];
  const supported = new Set([
    "storage.parts@1",
    "storage.blob@1",
    "rendering.sceneLayer@1",
    "contribute.exporter@1",
    "contribute.editContext@1",
    ...(door ? ["document.onDidOpen@1"] : []),
  ]);
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) => supported.has(f),
    parts: {
      read: async (p: string) => current.get(p) ?? null,
      write: async (p: string, b: Uint8Array) => void current.set(p, b.slice()),
      list: async () => [...current.keys()],
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
    editor: {
      client: {
        subscribe(l: (m: WorkerToMain) => void) {
          if (!opts.broadcasts) throw new Error("no raw client");
          listeners.add(l);
          return () => listeners.delete(l);
        },
      },
    },
    document: {
      elementGeometry: async () => [{ bounds: [0, 0, 200, 400] }],
      onDidChange: () => ({ dispose() {} }),
      ...(door
        ? {
            onDidOpen(l: (e: unknown) => void) {
              opened.add(l);
              return { dispose: () => void opened.delete(l) };
            },
          }
        : {}),
    },
    contribute: {
      panel: () => ({ dispose() {} }),
      command: () => ({ dispose() {} }),
      importer: () => ({ dispose() {} }),
      exporter(c: ExporterContribution) {
        exporters.push(c);
        return { dispose() {} };
      },
      objectType: () => ({ dispose() {} }),
      editContext(c: EditContextContribution) {
        editContexts.push(c);
        return { dispose() {} };
      },
      sceneLayer: () => ({
        async submit(_id: string, layer: SceneLayer) {
          submits.push(layer);
        },
        async clear() {},
        dispose() {},
      }),
    },
    shell: { openPanel() {}, closePanel() {} },
  } as unknown as BundleHost;

  return {
    host,
    blobs,
    submits,
    /** Open document `id` (its parts become the live container) and
     *  broadcast the load. A new id starts with `parts`. */
    open(id: string, parts?: Record<string, Uint8Array>) {
      let doc = docs.get(id);
      if (!doc) {
        doc = new Map(Object.entries(parts ?? {}));
        docs.set(id, doc);
      }
      current = doc;
      if (door) {
        const e = { docId: id, pageCount: 1, pageIds: [], pageSizesPt: [] };
        for (const l of opened) l(e);
      } else if (opts.broadcasts) {
        const msg = {
          kind: "documentLoaded",
          payload: { docId: id, pageCount: 1, pageIds: [] },
        } as unknown as WorkerToMain;
        for (const l of listeners) l(msg);
      }
    },
    parts: (id: string) => docs.get(id),
    /** Raw client subscribers (the bundle uses the door where it exists). */
    rawSubscribers: () => listeners.size,
    /** The active workbook as the exporter would save it, or null. */
    async exported(): Promise<Uint8Array | null> {
      const r = await exporters[0]!.export();
      return r ? (r as { bytes: Uint8Array }).bytes : null;
    },
    sheetContext: () => editContexts.find((c) => c.type === "sheet")!,
  };
}

/** A10 of `bytes` (null when there is no workbook). */
async function a10(bytes: Uint8Array | null): Promise<string | null> {
  if (!bytes) return null;
  const engine = await bootEngine();
  engine.loadXlsx(bytes);
  const v = engine.getCellInput(0, 9, 0);
  engine.dispose();
  return v;
}

const texts = (layer: SceneLayer) =>
  layer.items.flatMap((i) => (i.kind === "text" ? [i.text] : []));

describe.skipIf(!built)("the workbook follows the open document [sheet.plugin.persistence]", () => {
  for (const rawOnly of [false, true]) {
    it(`a document opened after boot restores its workbook part (${rawOnly ? "raw client broadcast" : "document.onDidOpen"})`, async () => {
      const ed = fakeEditor({ broadcasts: true, rawOnly });
      const handle = activate(ed.host);
      await vi.waitFor(async () => expect(await ed.exported()).toBeNull()); // boot: nothing yet
      // The door replaces the raw subscription; the raw one is the fallback.
      expect(ed.rawSubscribers()).toBe(rawOnly ? 1 : 0);

      ed.open("A", { "workbook.xlsx": await workbookWith("from A"), "workbook.name": new TextEncoder().encode("a.xlsx") });
      await vi.waitFor(async () => expect(await a10(await ed.exported())).toBe("from A"));
      handle.dispose();
      expect(ed.rawSubscribers()).toBe(0);
    });
  }

  it("entering a sheet frame with no workbook loaded restores it first (no open signal)", async () => {
    const ed = fakeEditor({ broadcasts: false });
    const handle = activate(ed.host);
    await new Promise((r) => setTimeout(r, 20)); // the boot restore found nothing
    ed.open("A", { "workbook.xlsx": await workbookWith("lazy") });

    ed.sheetContext().onEnter?.({ type: "sheet", id: { kind: "textFrame", id: "f1" } } as never);
    await vi.waitFor(() => expect(ed.submits.length).toBeGreaterThan(0));
    expect(texts(ed.submits.at(-1)!)).toContain("lazy");
    handle.dispose();
  });

  it("each document has its own workbook; one without a part has none", async () => {
    const ed = fakeEditor({ broadcasts: true });
    const handle = activate(ed.host);
    ed.open("A", { "workbook.xlsx": await workbookWith("from A") });
    await vi.waitFor(async () => expect(await a10(await ed.exported())).toBe("from A"));

    ed.open("B", { "workbook.xlsx": await workbookWith("from B") });
    await vi.waitFor(async () => expect(await a10(await ed.exported())).toBe("from B"));

    // A new document: the per-browser blob still holds B's workbook (every
    // persist writes it), and it must NOT leak into a document of its own.
    ed.open("C");
    await vi.waitFor(async () => expect(await ed.exported()).toBeNull());
    expect(ed.parts("C")!.has("workbook.xlsx")).toBe(false);

    ed.open("A");
    await vi.waitFor(async () => expect(await a10(await ed.exported())).toBe("from A"));
    handle.dispose();
  });

  it("an edit pending when another document opens is not written into it", async () => {
    const ed = fakeEditor({ broadcasts: true });
    const handle = activate(ed.host);
    ed.open("A", { "workbook.xlsx": await workbookWith("from A") });
    await vi.waitFor(async () => expect(await a10(await ed.exported())).toBe("from A"));
    // Enter the frame and type into A1 (the debounce has not elapsed).
    const ctx = ed.sheetContext();
    ctx.onEnter?.({ type: "sheet", id: { kind: "textFrame", id: "f1" } } as never);
    await vi.waitFor(() => expect(ed.submits.length).toBeGreaterThan(0));
    ctx.onContentPointerDown?.({
      contentPoint: [5, 5],
      button: 0,
      modifiers: { shift: false, alt: false, cmd: false, ctrl: false },
    } as never);
    for (const key of ["x", "Enter"]) ctx.onContentKey?.({ key } as KeyboardEvent);
    await vi.waitFor(() => expect(texts(ed.submits.at(-1)!)).toContain("x")); // the edit landed

    ed.open("B", { "workbook.xlsx": await workbookWith("from B") });
    await vi.waitFor(async () => expect(await a10(await ed.exported())).toBe("from B"));
    await new Promise((r) => setTimeout(r, 1_600)); // past the persist debounce
    expect(await a10(ed.parts("B")!.get("workbook.xlsx")!)).toBe("from B");
    handle.dispose();
  });
});

describe.skipIf(!built)("a wheel over the entered frame scrolls the grid [sheet.grid.inframe]", () => {
  it("declares onContentWheel: down scrolls by rows and claims; up at row 1 declines", async () => {
    const ed = fakeEditor({ broadcasts: true });
    const handle = activate(ed.host);
    ed.open("A", { "workbook.xlsx": await workbookWith("row ten") });
    await vi.waitFor(async () => expect(await a10(await ed.exported())).toBe("row ten"));
    const ctx = ed.sheetContext() as EditContextContribution & {
      onContentWheel?(e: unknown): boolean;
    };
    expect(typeof ctx.onContentWheel).toBe("function");
    const wheel = (dy: number, dx = 0, shift = false) =>
      ctx.onContentWheel!({
        contentPoint: [10, 10],
        elementId: "f1",
        delta: [dx, dy],
        modifiers: { shift, alt: false, cmd: false, ctrl: false },
      });

    expect(wheel(40)).toBe(false); // no grid showing yet: the canvas pans
    ctx.onEnter?.({ type: "sheet", id: { kind: "textFrame", id: "f1" } } as never);
    await vi.waitFor(() => expect(ed.submits.length).toBeGreaterThan(0));
    expect(wheel(-40)).toBe(false); // already at row 1: decline

    const before = ed.submits.length;
    expect(texts(ed.submits.at(-1)!)).toContain("1"); // A1's value
    expect(wheel(400)).toBe(true); // many rows down
    await vi.waitFor(() => expect(ed.submits.length).toBeGreaterThan(before));
    expect(texts(ed.submits.at(-1)!)).not.toContain("1"); // row 1 scrolled off

    const scrolled = ed.submits.length;
    expect(wheel(1)).toBe(true); // a sub-row step is still the grid's (carried)
    await new Promise((r) => setTimeout(r, 20));
    expect(ed.submits.length).toBe(scrolled); // …but moves nothing yet
    expect(wheel(-4000)).toBe(true); // back up, clamped at row 1
    await vi.waitFor(() => expect(texts(ed.submits.at(-1)!)).toContain("1"));
    expect(wheel(-40)).toBe(false);
    handle.dispose();
  });
});
