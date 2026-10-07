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

// [sheet.plugin.persistence] Two engine boots in flight at once share ONE
// wasm instance.
//
// Reopening a saved .paged in a fresh editor boots the engine twice at
// the same time: the activation restore and the `documentLoaded` restore
// (and, on frame entry, the lazy one). The browser path called the
// wasm-bindgen glue's async `default()` per boot. Its "already
// initialised" check runs BEFORE the await, so two concurrent calls both
// instantiate, and the second REPLACES the glue's module-level instance.
// An engine built on the first instance then reads the second instance's
// memory with its own pointer: "RuntimeError: memory access out of
// bounds" in `list_freeze_panes` (the first read the grid panel makes).
//
// This drives the REAL glue down the browser path (a stubbed `window`;
// `fetch` answers the wasm bytes, delayed per call so the race is
// deterministic). Before the fix the first engine read garbage (an empty
// freeze list) or trapped. Same dual gate as engine-real.spec.ts.

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

const HERE = dirname(fileURLToPath(import.meta.url));
const WASM = join(HERE, "..", "bin", "sheet_js_bg.wasm");
const FREEZE_XLSX = join(HERE, "..", "..", "..", "corpus/xlsx-corpus/11-freeze.xlsx");
const built = existsSync(WASM);
const required = process.env.REQUIRE_REAL_ENGINE === "1";

// Each boot's fetch of the `?url` artifact answers the wasm bytes, later
// for each later boot (the first boot finishes first and mints its engine
// before the second instantiates).
let fetches = 0;
function delayedWasmFetch(): Promise<Response> {
  const delay = 10 * ++fetches;
  return new Promise((r) =>
    setTimeout(
      () => r(new Response(readFileSync(WASM), { headers: { "Content-Type": "application/wasm" } })),
      delay,
    ),
  );
}

describe.skipIf(!built && !required)(
  "concurrent engine boots share one wasm instance [sheet.plugin.persistence]",
  () => {
    beforeAll(() => {
      expect(existsSync(WASM), `${WASM} missing — run scripts/build-wasm.sh`).toBe(true);
      (globalThis as { window?: unknown }).window = globalThis; // the browser branch
      vi.stubGlobal("fetch", delayedWasmFetch);
    });
    afterAll(() => {
      delete (globalThis as { window?: unknown }).window;
      vi.unstubAllGlobals();
    });

    it("an engine booted alongside another still reads its workbook (list_freeze_panes)", async () => {
      const { bootEngine } = await import("../src/engine");
      const xlsx = new Uint8Array(readFileSync(FREEZE_XLSX));
      // The activation restore boots and loads; the document-open restore
      // boots alongside it.
      const first = bootEngine().then((e) => {
        e.loadXlsx(xlsx);
        expect(e.listFreezePanes(), "the fixture has one frozen sheet").toHaveLength(1);
        return e;
      });
      const second = bootEngine();
      const [a, b] = await Promise.all([first, second]);
      expect(a.listFreezePanes()).toHaveLength(1);
      expect(a.listSheets().length).toBeGreaterThan(0);
      b.loadXlsx(xlsx);
      expect(b.listFreezePanes()).toEqual(a.listFreezePanes());
      a.dispose();
      b.dispose();
    });
  },
);

// A trap is a diagnostic, never a second read of a broken instance.
describe("a wasm trap surfaces as a diagnostic [sheet.plugin.persistence]", () => {
  it("names the trapping call, refuses every later call, and disposes quietly", async () => {
    const { wrapEngine } = await import("../src/engine");
    const reached: string[] = [];
    const fake = {
      list_freeze_panes() {
        reached.push("list_freeze_panes");
        throw new WebAssembly.RuntimeError("memory access out of bounds");
      },
      list_sheets() {
        reached.push("list_sheets");
        return [];
      },
      set_cell() {
        throw new Error("sheet id 9 out of range (1 sheets)");
      },
      free() {
        reached.push("free");
      },
    };
    const engine = wrapEngine(fake as never);
    // A boundary error passes through and leaves the engine usable.
    expect(() => engine.setCell(9, 0, 0, "1")).toThrow("out of range");
    expect(engine.listSheets()).toEqual([]);
    expect(() => engine.listFreezePanes()).toThrow(
      /sheet engine trapped in listFreezePanes \(memory access out of bounds\)/,
    );
    expect(() => engine.listSheets()).toThrow(/trapped in listFreezePanes/);
    expect(() => engine.dispose()).not.toThrow();
    expect(reached).toEqual(["list_sheets", "list_freeze_panes"]);
  });
});
