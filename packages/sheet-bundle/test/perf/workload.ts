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

// The perf workloads' fixtures: a REAL engine-backed headless host
// (`createHeadlessHost` over the published `@paged-media/canvas-wasm`, a
// devDependency of this package so CI needs no sibling checkout), a
// one-page IDML to place into, and workbooks authored by the REAL sheet
// engine (so every value the budgets' behaviour checks read was computed
// in Rust).

import { existsSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { inflateRawSync } from "node:zlib";

import { createHeadlessHost, type HeadlessHost } from "@paged-media/plugin-sdk";
import type { BundleHost, ClipboardPayload } from "@paged-media/plugin-api";

import { sheetBundle } from "../../src";

import { packageWithSpread } from "./build-idml";

const HERE = dirname(fileURLToPath(import.meta.url));
const PKG = resolve(HERE, "../..");
const BIN = join(PKG, "bin");
export const WASM = join(BIN, "sheet_js_bg.wasm");
/** The sheet engine artifact is built (scripts/build-wasm.sh). */
export const ENGINE_BUILT = existsSync(WASM);

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

/** An in-memory clipboard the paste stream fills before it pastes. */
export interface TestClipboard {
  payload: ClipboardPayload | null;
  read(): Promise<ClipboardPayload | null>;
  write(p: ClipboardPayload): Promise<void>;
}

export function testClipboard(): TestClipboard {
  const c: TestClipboard = {
    payload: null,
    read: async () => c.payload,
    write: async (p) => {
      c.payload = p;
    },
  };
  return c;
}

/** Boot a real headless host (silent console, in-memory storage). */
export async function openHost(clipboard?: TestClipboard): Promise<HeadlessHost> {
  return createHeadlessHost({
    console: silent,
    storage: mapBacking(),
    resolveFrom: PKG,
    ...(clipboard ? { clipboard } : {}),
  });
}

/** The host core hands paged.sheet itself: a stand-in bundle carrying
 *  the REAL sheet manifest (its namespace, its declared capabilities —
 *  `rendering.sceneLayer`, `storage.blob`, `clipboard`) whose activate
 *  only captures the host. The harness's own `h.host` is a different
 *  plugin (`media.paged.harness`), and core refuses its writes into the
 *  sheet's metadata namespace. The sheet bundle's own activate is NOT
 *  run: the workloads drive a session they own. */
export function sheetHost(h: HeadlessHost): BundleHost {
  let captured: BundleHost | null = null;
  h.loadBundle({
    ...sheetBundle,
    activate: (host: BundleHost) => {
      captured = host;
      return { dispose() {} };
    },
  } as typeof sheetBundle);
  if (!captured) throw new Error("the stand-in bundle did not activate");
  return captured;
}

/** The editor's scene channel, which the headless host does not wire
 *  (`supports("rendering.sceneLayer@1")` is false there): a recording
 *  surface, so the in-frame grid has somewhere to submit and the counting
 *  host can count what it submits. Everything else forwards untouched. */
export function withSceneChannel(host: BundleHost): {
  host: BundleHost;
  submits: { elementId: string; items: number; texts: string[] }[];
} {
  const submits: { elementId: string; items: number; texts: string[] }[] = [];
  const surface = {
    submit: async (elementId: string, layer: { items: unknown[] }) => {
      const texts = (layer.items as { kind?: string; text?: string }[])
        .filter((i) => i.kind === "text")
        .map((i) => i.text ?? "");
      submits.push({ elementId, items: layer.items.length, texts });
    },
    clear: async () => {},
    dispose: () => {},
  };
  const contribute = new Proxy(host.contribute, {
    get(o, p, r) {
      if (p === "sceneLayer") return () => surface;
      return Reflect.get(o, p, r);
    },
  });
  const wrapped = new Proxy(host, {
    get(o, p, r) {
      if (p === "supports") {
        return (f: string) => f === "rendering.sceneLayer@1" || o.supports(f);
      }
      if (p === "contribute") return contribute;
      return Reflect.get(o, p, r);
    },
  });
  return { host: wrapped, submits };
}

/** One empty Letter page (612 × 792 pt) to place into. */
export const blankPageIdml = (): Uint8Array => packageWithSpread("");

/** The wasm-bindgen glue, initialised once (the same module instance
 *  `src/engine.ts` boots — `initSync` is a no-op the second time). */
interface Glue {
  initSync(o: { module: Uint8Array }): void;
  SheetEngine: new () => {
    set_cell(s: number, r: number, c: number, input: string): unknown;
    save_xlsx(): Uint8Array;
    free(): void;
  };
}

async function glue(): Promise<Glue> {
  const g = (await import(/* @vite-ignore */ join(BIN, "sheet_js.js"))) as Glue;
  g.initSync({ module: readFileSync(WASM) });
  return g;
}

/** Author an xlsx in the REAL engine: `cell(r, c)` is the input for each
 *  cell of a `rows × cols` block at A1 (`null` leaves it blank), plus any
 *  `extra` `[row, col, input]` cells. */
export async function authorWorkbook(
  rows: number,
  cols: number,
  cell: (r: number, c: number) => string | null,
  extra: [number, number, string][] = [],
): Promise<Uint8Array> {
  const g = await glue();
  const e = new g.SheetEngine();
  try {
    for (let r = 0; r < rows; r++) {
      for (let c = 0; c < cols; c++) {
        const v = cell(r, c);
        if (v !== null) e.set_cell(0, r, c, v);
      }
    }
    for (const [r, c, v] of extra) e.set_cell(0, r, c, v);
    return e.save_xlsx();
  } finally {
    e.free();
  }
}

/** Let queued microtasks and the zero-delay timers behind them run. */
export const settle = async (): Promise<void> => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

/** The document as core exports it (`exportIdml` on the editor client):
 *  every zip entry's text, by path. How a scenario reads back what a
 *  placement actually wrote — table cell text included, which no plugin
 *  read door returns. */
export async function exportedIdmlParts(h: HeadlessHost): Promise<Map<string, string>> {
  const client = (h.host as unknown as {
    editor: { client: { send(m: unknown): Promise<{ kind: string; payload: { idmlBytes?: number[] } }> } };
  }).editor.client;
  const reply = await client.send({ kind: "exportIdml", payload: {} });
  if (reply.kind !== "idmlExported" || !reply.payload.idmlBytes) {
    throw new Error(`exportIdml failed: ${JSON.stringify(reply).slice(0, 200)}`);
  }
  const zip = Buffer.from(reply.payload.idmlBytes);
  // The end-of-central-directory record, then each central entry.
  let eocd = zip.length - 22;
  while (eocd >= 0 && zip.readUInt32LE(eocd) !== 0x06054b50) eocd -= 1;
  const count = zip.readUInt16LE(eocd + 10);
  let p = zip.readUInt32LE(eocd + 16);
  const out = new Map<string, string>();
  for (let i = 0; i < count; i++) {
    const method = zip.readUInt16LE(p + 10);
    const size = zip.readUInt32LE(p + 20);
    const nameLen = zip.readUInt16LE(p + 28);
    const extraLen = zip.readUInt16LE(p + 30);
    const commentLen = zip.readUInt16LE(p + 32);
    const local = zip.readUInt32LE(p + 42);
    const name = zip.subarray(p + 46, p + 46 + nameLen).toString("utf8");
    const lName = zip.readUInt16LE(local + 26);
    const lExtra = zip.readUInt16LE(local + 28);
    const data = zip.subarray(local + 30 + lName + lExtra, local + 30 + lName + lExtra + size);
    out.set(name, (method === 8 ? inflateRawSync(data) : data).toString("utf8"));
    p += 46 + nameLen + extraLen + commentLen;
  }
  return out;
}

/** The text of every `<Cell>` in the exported document's stories, in
 *  document order, keyed `row:col` per table (`Name="col:row"` in IDML). */
export async function exportedTableCells(h: HeadlessHost): Promise<Map<string, string>[]> {
  const parts = await exportedIdmlParts(h);
  const tables: Map<string, string>[] = [];
  for (const [name, xml] of parts) {
    if (!name.startsWith("Stories/")) continue;
    for (const t of xml.matchAll(/<Table\b[\s\S]*?<\/Table>/g)) {
      const cells = new Map<string, string>();
      for (const c of t[0].matchAll(/<Cell\b[^>]*\bName="(\d+):(\d+)"[^>]*>([\s\S]*?)<\/Cell>/g)) {
        const text = [...c[3].matchAll(/<Content>([\s\S]*?)<\/Content>/g)].map((m) => m[1]).join("");
        cells.set(`${c[2]}:${c[1]}`, text);
      }
      tables.push(cells);
    }
  }
  return tables;
}
