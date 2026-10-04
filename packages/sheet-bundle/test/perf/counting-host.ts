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

// The work counter the paged.sheet perf budgets stand on — the pattern of
// plugin-draw's `test/perf/counting-host.ts`, extended with the two
// boundaries a sheet crosses that a draw tool does not.
//
// A sheet's cost is not its arithmetic (that is Rust, counted by the
// engine's own `perf-counters` feature). It is the DOORS it goes through:
// every `host.document.*` call is a request/reply to the engine worker in
// the editor, every `mutate` is a document rebuild (and, outside a batch,
// an undo step), every scene-layer submit is a repaint, every blob/part
// write is bytes to storage. And every call into the sheet wasm is a
// boundary crossing with serde on both sides. So the budgets count calls,
// not milliseconds: a count is the same on a laptop and a CI runner, and
// a fix that halves it halves it everywhere.
//
// `countingHost(h.host)` wraps a real `BundleHost` in a Proxy that counts
// every function call by its dotted door name (`document.mutate`,
// `blob.write`, …) and forwards it untouched. The scene-layer SURFACE the
// session gets back from `contribute.sceneLayer()` is wrapped too, so
// its `submit`s count as `sceneLayer.submit`. `countingEngine(engine)`
// does the same for the `SheetEngine` facade (`engine.setCell`, …).
// Nothing is mocked: core and the sheet engine still answer, so a budget
// and a behaviour assertion can share one gesture.

import type { BundleHost } from "@paged-media/plugin-api";

import type { SheetEngine } from "../../src/engine";

/** One `document.mutate` as the engine saw it. */
export interface CountedMutation {
  /** The op name; `"batch"` for a batch. */
  op: string;
  /** How many ops it carried — 1 unless it is a batch. */
  ops: number;
}

export interface WorkLog {
  /** Calls per door, keyed by dotted path (`document.mutate`,
   *  `sceneLayer.submit`, `engine.setCell`). */
  readonly calls: Readonly<Record<string, number>>;
  /** Every `document.mutate`, in order. */
  readonly mutations: readonly CountedMutation[];
  /** Bytes handed to `blob.write` + `parts.write`. */
  readonly bytesWritten: number;
  /** Items (rects/lines/text runs) across every scene-layer submit — a
   *  keystroke that repaints the whole grid shows up here, not only as one
   *  more submit. */
  readonly sceneItems: number;
  /** `document.mutate` outcomes that came back `applied: false` — counted
   *  when the reply lands, so `settle()` first. A budget beside a
   *  rejected write is measuring nothing. */
  readonly rejected: number;
  /** Calls to one door (0 when it was never called). */
  count(door: string): number;
  /** Every `document.*` call that is not a write, a history step or a
   *  subscription — the engine round trips spent READING. */
  reads(): number;
  /** Every call into the sheet engine (`engine.*`). */
  engineCalls(): number;
  /** Forget everything counted so far. */
  reset(): void;
  /** A frozen copy of the counts as they stand. */
  snapshot(): WorkLog;
}

const NOT_A_READ = new Set([
  "document.mutate",
  "document.undo",
  "document.redo",
  "document.onDidChange",
]);

const isPlainObject = (v: unknown): v is Record<string, unknown> => {
  if (v === null || typeof v !== "object") return false;
  const proto = Object.getPrototypeOf(v);
  return proto === Object.prototype || proto === null;
};

const byteLength = (v: unknown): number =>
  v instanceof Uint8Array ? v.byteLength : 0;

interface Counts {
  calls: Record<string, number>;
  mutations: CountedMutation[];
  bytesWritten: number;
  sceneItems: number;
  rejected: number;
}

const fresh = (): Counts => ({
  calls: {},
  mutations: [],
  bytesWritten: 0,
  sceneItems: 0,
  rejected: 0,
});

/** One shared tally — host and engine counted into the same log, so a
 *  scenario reads both in one snapshot. */
export class Tally {
  private c: Counts = fresh();

  note(door: string, args: unknown[]): void {
    const calls = this.c.calls;
    calls[door] = (calls[door] ?? 0) + 1;
    if (door === "document.mutate") {
      const m = args[0] as { op?: string; args?: { ops?: unknown[] } };
      this.c.mutations.push({
        op: m?.op ?? "?",
        ops: m?.op === "batch" ? (m.args?.ops?.length ?? 0) : 1,
      });
    } else if (door === "blob.write" || door === "parts.write") {
      this.c.bytesWritten += byteLength(args[1]);
    } else if (door === "sceneLayer.submit") {
      const items = (args[1] as { items?: unknown[] } | undefined)?.items;
      this.c.sceneItems += Array.isArray(items) ? items.length : 0;
    }
  }

  /** Watch a reply without touching what the caller sees. */
  observe(door: string, result: unknown): void {
    if (door !== "document.mutate") return;
    if (!result || typeof (result as PromiseLike<unknown>).then !== "function") return;
    const counts = this.c;
    (result as PromiseLike<{ applied?: boolean }>).then(
      (o) => {
        if (o && o.applied === false) counts.rejected += 1;
      },
      () => {
        counts.rejected += 1;
      },
    );
  }

  readonly work: WorkLog = logOver(
    () => this.c,
    () => {
      this.c = fresh();
    },
  );
}

function logOver(state: () => Counts, reset: () => void): WorkLog {
  return {
    get calls() {
      return state().calls;
    },
    get mutations() {
      return state().mutations;
    },
    get bytesWritten() {
      return state().bytesWritten;
    },
    get sceneItems() {
      return state().sceneItems;
    },
    get rejected() {
      return state().rejected;
    },
    count: (door) => state().calls[door] ?? 0,
    reads: () =>
      Object.entries(state().calls)
        .filter(([k]) => k.startsWith("document.") && !NOT_A_READ.has(k))
        .reduce((n, [, v]) => n + v, 0),
    engineCalls: () =>
      Object.entries(state().calls)
        .filter(([k]) => k.startsWith("engine."))
        .reduce((n, [, v]) => n + v, 0),
    reset,
    snapshot: () => {
      const s = state();
      const frozen: Counts = {
        calls: { ...s.calls },
        mutations: [...s.mutations],
        bytesWritten: s.bytesWritten,
        sceneItems: s.sceneItems,
        rejected: s.rejected,
      };
      return logOver(
        () => frozen,
        () => {
          /* a snapshot is frozen */
        },
      );
    },
  };
}

/** Wrap `target` so every function reached through it counts by dotted
 *  path. Returned scene-layer surfaces are wrapped as `sceneLayer`. */
function wrapCounting<T extends object>(
  target: T,
  path: string,
  tally: Tally,
  wrapped: WeakMap<object, object>,
): T {
  const hit = wrapped.get(target);
  if (hit) return hit as T;
  const proxy = new Proxy(target, {
    get(obj, prop, receiver) {
      const value = Reflect.get(obj, prop, receiver) as unknown;
      if (typeof prop !== "string") return value;
      const door = path ? `${path}.${prop}` : prop;
      if (typeof value === "function") {
        return (...args: unknown[]) => {
          tally.note(door, args);
          const result = Reflect.apply(value, obj, args) as unknown;
          tally.observe(door, result);
          if (door === "contribute.sceneLayer" && result && typeof result === "object") {
            return wrapCounting(result, "sceneLayer", tally, wrapped);
          }
          return result;
        };
      }
      return isPlainObject(value) ? wrapCounting(value, door, tally, wrapped) : value;
    },
  });
  wrapped.set(target, proxy);
  return proxy;
}

/** Count every door call a bundle makes through `host`. Hand the WRAPPED
 *  host to the code under test; anything holding the raw host (the
 *  harness's own setup) is deliberately not counted. */
export function countingHost(
  host: BundleHost,
  tally: Tally = new Tally(),
): { host: BundleHost; work: WorkLog; tally: Tally } {
  const h = wrapCounting(host as unknown as object, "", tally, new WeakMap());
  return { host: h as BundleHost, work: tally.work, tally };
}

/** The tally the perf specs share: the engine facades the session boots
 *  (through the `vi.mock` of `src/engine` in the spec) and the counted
 *  host both note into it, so one snapshot holds both sides. */
export const SHARED = new Tally();

/** Count every call into the sheet engine facade as `engine.<method>`. */
export function countingEngine(engine: SheetEngine, tally: Tally): SheetEngine {
  return wrapCounting(engine as unknown as object, "engine", tally, new WeakMap()) as SheetEngine;
}

/** The per-test timeout the budget specs set. A count does not get slower
 *  on a loaded runner; the wall clock around it does. No budget here is a
 *  duration, so a generous timeout hides nothing. */
export const BUDGET_TIMEOUT_MS = 120_000;

/** The whole log as one plain object — every door, not only the ones a
 *  budget names. */
export function workSummary(work: WorkLog): Record<string, unknown> {
  const calls: Record<string, number> = {};
  for (const door of Object.keys(work.calls).sort()) {
    calls[door] = work.calls[door]!;
  }
  const ops: Record<string, number> = {};
  for (const m of work.mutations) {
    const k = m.op === "batch" ? `batch(${m.ops})` : m.op;
    ops[k] = (ops[k] ?? 0) + 1;
  }
  return {
    calls,
    reads: work.reads(),
    engineCalls: work.engineCalls(),
    mutations: ops,
    bytesWritten: work.bytesWritten,
    sceneItems: work.sceneItems,
    rejected: work.rejected,
  };
}

/** HOW TO RE-MEASURE. Run the perf specs with `PERF_SHOW=1` and every
 *  scenario prints its full work log on one `PERF` line — the numbers a
 *  budget is pinned from, and the place to look when one moves. Silent
 *  otherwise. `extra` carries what a scenario measured beside the log. */
export function report(
  scenario: string,
  work: WorkLog,
  extra: Record<string, unknown> = {},
): WorkLog {
  if (process.env.PERF_SHOW) {
    // eslint-disable-next-line no-console
    console.log(`PERF ${scenario} ${JSON.stringify({ ...workSummary(work), ...extra })}`);
  }
  return work;
}
