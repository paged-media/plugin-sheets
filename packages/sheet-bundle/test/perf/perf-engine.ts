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

// The COUNTING engine (scripts/build-wasm-perf.sh → bin-perf/): the same
// sheet engine built with the `perf-counters` feature, which exports the
// calc engine's work counters. Opt-in — `PERF_ENGINE=1` with the artifact
// built — and trended, never budgeted here (CI does not build it; the
// engine's counts are pinned in Rust, sheet-calc/sheet-js perf_budgets).
//
// Kept free of `src/` imports: the spec's `vi.mock` of src/engine loads
// this while that module is being mocked.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const BIN_PERF = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "bin-perf");
const WASM = join(BIN_PERF, "sheet_js_bg.wasm");

interface PerfGlue {
  initSync(o: { module: Uint8Array }): void;
  SheetEngine: new () => unknown;
  perfCounters(): Record<string, number>;
  resetPerfCounters(): void;
}

let glue: PerfGlue | null = null;

/** The counting engine is wanted and built. */
export const PERF_ENGINE = process.env.PERF_ENGINE === "1" && existsSync(WASM);

/** Boot a counting engine through `wrap` (src/engine's `wrapEngine`), or
 *  null when the counting engine is not in use. */
export async function perfBoot<T>(wrap: (wasm: never) => T): Promise<T | null> {
  if (!PERF_ENGINE) return null;
  if (!glue) {
    glue = (await import(/* @vite-ignore */ join(BIN_PERF, "sheet_js.js"))) as PerfGlue;
    glue.initSync({ module: readFileSync(WASM) });
  }
  return wrap(new glue.SheetEngine() as never);
}

/** The engine's counters since the last reset (null without it). */
export function engineCounters(): Record<string, number> | null {
  return glue ? glue.perfCounters() : null;
}

export function resetEngineCounters(): void {
  glue?.resetPerfCounters();
}
