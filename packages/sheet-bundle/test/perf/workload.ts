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
import type { BundleHost, ClipboardPayload, ElementId } from "@paged-media/plugin-api";

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

/** The protocol-66 doors over a 0.64 host, for the Wave 9 scenarios that
 *  drive the 66 path (the pins stay at 0.2.37 until 66 is published):
 *
 *   `minted` — core already sends it on `mutationApplied`; the 0.2.37
 *       host-impl drops it. Here `document.mutate` goes to the editor
 *       client directly and the outcome carries the engine's own list.
 *   `geometryStoryId` — each text frame's geometry item gains its story,
 *       resolved test-side by walking the stories' frame chains (what a 66
 *       core answers from the frame's ParentStory).
 *
 *  Everything else forwards untouched (scene channel, counting, gates). */
export function withDoors66(
  h: HeadlessHost,
  host: BundleHost,
  doors: { minted?: boolean; geometryStoryId?: boolean },
): BundleHost {
  const client = (h.host as unknown as {
    editor: { client: { mutate(m: unknown): Promise<{ kind: string; payload: Record<string, unknown> }> } };
  }).editor.client;
  const raw = host.document;
  const document = new Proxy(raw, {
    get(o, p, r) {
      if (p === "mutate" && doors.minted) {
        return async (m: unknown) => {
          const reply = await client.mutate(m);
          if (reply.kind !== "mutationApplied") return { applied: false, error: reply.payload ?? reply };
          return {
            applied: true,
            createdId: reply.payload.createdId ?? null,
            pageIds: reply.payload.pageIds,
            ...(reply.payload.minted !== undefined ? { minted: reply.payload.minted } : {}),
          };
        };
      }
      if (p === "elementGeometry" && doors.geometryStoryId) {
        return async (ids: Parameters<typeof raw.elementGeometry>[0]) => {
          const items = await raw.elementGeometry(ids);
          const stories = await raw.collection<{ selfId: string }>("stories");
          const storyOf = new Map<string, string>();
          for (const s of stories) {
            for (const l of await raw.frameChain(s.selfId)) storyOf.set(l.frameId, s.selfId);
          }
          return items.map((it) =>
            it.id.kind === "textFrame" && storyOf.has(it.id.id as string)
              ? { ...it, storyId: storyOf.get(it.id.id as string) }
              : it,
          );
        };
      }
      return Reflect.get(o, p, r);
    },
  });
  return new Proxy(host, {
    get(o, p, r) {
      if (p === "document") return document;
      return Reflect.get(o, p, r);
    },
  });
}

/** A protocol-66 host over the real 0.64 core, for the ONE-CALL placement
 *  (core 0eff96b resolves `$h:t`, bound to an `insertTable`, in a
 *  `tableId` / `table_id` position to the table; 0.64 does not, and the
 *  pins stay at 0.2.37 until 66 is published).
 *
 *  It answers `supports("document.onWillSave@1")` (the 66 signal the bundle
 *  keys the one-call placement on) and sends `minted`. A batch that binds a
 *  table handle is applied in two engine batches: the head up to the
 *  `insertTable`, then the rest with `$h:t` rewritten the way 66 core does
 *  (`storyId` / `story_id` → the table's story, `tableId` / `table_id` →
 *  the table). The CALLER still makes one `mutate` — what the budget counts.
 *  Not emulated: atomicity across the split and the single undo step. */
export function withTableHandles66(h: HeadlessHost, host: BundleHost): BundleHost {
  const client = (h.host as unknown as {
    editor: { client: { mutate(m: unknown): Promise<{ kind: string; payload: Record<string, unknown> }> } };
  }).editor.client;
  type Op = { op: string; args: Record<string, unknown> };
  const outcome = (reply: { kind: string; payload: Record<string, unknown> }) =>
    reply.kind === "mutationApplied"
      ? {
          applied: true as const,
          createdId: (reply.payload.createdId ?? null) as ElementId | null,
          pageIds: reply.payload.pageIds,
          minted: (reply.payload.minted ?? []) as { handle: string | null; element: ElementId; storyId: string | null }[],
        }
      : { applied: false as const, error: reply.payload ?? reply };
  const rewrite = (v: unknown, key: string | null, story: string, table: string): unknown => {
    if (typeof v === "string" && v === "$h:t") {
      if (key === "storyId" || key === "story_id") return story;
      if (key === "tableId" || key === "table_id") return table;
      return v;
    }
    if (Array.isArray(v)) return v.map((x) => rewrite(x, null, story, table));
    if (v && typeof v === "object") {
      return Object.fromEntries(
        Object.entries(v).map(([k, x]) => [k, rewrite(x, k, story, table)]),
      );
    }
    return v;
  };
  const mutate = async (m: { op: string; args: { ops?: Op[] } }) => {
    const ops = m.op === "batch" ? (m.args.ops ?? []) : [];
    const at = ops.findIndex((o) => o.op === "bindCreated" && o.args.handle === "t");
    if (at < 1 || ops[at - 1].op !== "insertTable") return outcome(await client.mutate(m));
    const head = outcome(await client.mutate({ op: "batch", args: { ops: ops.slice(0, at) } }));
    if (!head.applied) return head;
    const t = head.createdId;
    if (t?.kind !== "table") return { applied: false as const, error: "no table minted" };
    const { story_id: story, table_id: table } = t.id;
    const minted = [
      ...head.minted.filter((x) => x.element.kind !== "table"),
      { handle: "t", element: t, storyId: story },
    ];
    const rest = ops.slice(at + 1).map((o) => rewrite(o, null, story, table));
    if (rest.length > 0) {
      const tail = outcome(await client.mutate({ op: "batch", args: { ops: rest } }));
      if (!tail.applied) return tail;
      minted.push(...tail.minted);
    }
    return { applied: true as const, createdId: minted[minted.length - 1].element, pageIds: [], minted };
  };
  const document = new Proxy(host.document, {
    get(o, p, r) {
      if (p === "mutate") return mutate;
      return Reflect.get(o, p, r);
    },
  });
  return new Proxy(host, {
    get(o, p, r) {
      if (p === "document") return document;
      if (p === "supports") {
        return (f: string) => f === "document.onWillSave@1" || host.supports(f);
      }
      return Reflect.get(o, p, r);
    },
  });
}

/** A PRE-66 host over the real core: the doors a protocol-66+ engine and
 *  SDK answer are taken away — no `minted` on an outcome, no `storyId` on a
 *  geometry item, `supports("document.onWillSave@1")` false — so the
 *  fallback lanes the bundle keeps for older hosts run against the engine
 *  under test (0.68 since the 0.2.41 contract; they ran on the real 0.64
 *  core before). */
export function withoutDoors66(host: BundleHost): BundleHost {
  const raw = host.document;
  const document = new Proxy(raw, {
    get(o, p, r) {
      if (p === "mutate") {
        return async (m: Parameters<typeof raw.mutate>[0]) => {
          const out = await raw.mutate(m);
          if (!out.applied) return out;
          const { minted: _minted, ...rest } = out as typeof out & { minted?: unknown };
          return rest;
        };
      }
      if (p === "elementGeometry") {
        return async (ids: Parameters<typeof raw.elementGeometry>[0]) =>
          (await raw.elementGeometry(ids)).map((it) => {
            const { storyId: _storyId, ...rest } = it as typeof it & { storyId?: unknown };
            return rest;
          });
      }
      return Reflect.get(o, p, r);
    },
  });
  return new Proxy(host, {
    get(o, p, r) {
      if (p === "document") return document;
      if (p === "supports") {
        return (f: string) => f !== "document.onWillSave@1" && host.supports(f);
      }
      return Reflect.get(o, p, r);
    },
  });
}

/** A host whose core refuses a batch naming the in-batch TABLE handle
 *  (`$h:t`) — what a pre-66 core does with the one-call placement (it
 *  resolves no table handle, so the whole batch is refused and nothing
 *  lands). Anything else passes through. */
export function refusingTableHandles(host: BundleHost): BundleHost {
  const raw = host.document;
  const namesTableHandle = (v: unknown): boolean =>
    v === "$h:t" ||
    (Array.isArray(v) ? v.some(namesTableHandle) : !!v && typeof v === "object" && Object.values(v).some(namesTableHandle));
  const document = new Proxy(raw, {
    get(o, p, r) {
      if (p === "mutate") {
        return async (m: Parameters<typeof raw.mutate>[0]) =>
          namesTableHandle(m)
            ? { applied: false as const, error: "unresolved handle $h:t (pre-66 core)" }
            : raw.mutate(m);
      }
      return Reflect.get(o, p, r);
    },
  });
  return new Proxy(host, {
    get(o, p, r) {
      if (p === "document") return document;
      return Reflect.get(o, p, r);
    },
  });
}
