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

// sheet.plugin.workbook.part (.paged container) — the workbook bytes + name
// round-trip through `host.parts` (the portable home that travels WITH the
// document, vs the per-browser `host.blob`). Pure module test: no engine, no
// session — an in-memory parts store stands in for the container writer.

import { describe, expect, it } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";

import { readWorkbookPart, writeWorkbookPart } from "../src/workbook-part";

/** A host exposing just the `parts` door + `supports`, backed by an in-memory
 *  store (the host normally prepends the plugin namespace; this fake keeps the
 *  relative paths verbatim, which is all the module speaks). */
function partsHost(partsSupported = true) {
  const store = new Map<string, Uint8Array>();
  const host = {
    supports: (f: string) => f === "storage.parts@1" && partsSupported,
    parts: {
      write: async (p: string, b: Uint8Array) => void store.set(p, b),
      read: async (p: string) => store.get(p) ?? null,
      list: async (prefix?: string) =>
        [...store.keys()].filter((k) => !prefix || k.startsWith(prefix)),
    },
  } as unknown as BundleHost;
  return { host, store };
}

describe("sheet_plugin_workbook_part", () => {
  it("writes then reads back the workbook bytes + name", async () => {
    const { host, store } = partsHost();
    const bytes = new Uint8Array([0x50, 0x4b, 0x03, 0x04]); // "PK␃␄" (a zip magic)
    await writeWorkbookPart(host, bytes, "budget.xlsx");

    // Two parts under the namespace-relative paths the module owns.
    expect([...store.keys()].sort()).toEqual(["workbook.name", "workbook.xlsx"]);

    const back = await readWorkbookPart(host);
    expect(back).not.toBeNull();
    expect(Array.from(back!.bytes)).toEqual(Array.from(bytes));
    expect(back!.name).toBe("budget.xlsx");
  });

  it("returns null when no workbook part is present", async () => {
    const { host } = partsHost();
    expect(await readWorkbookPart(host)).toBeNull();
  });

  it("is a clean no-op when the host has no container writer", async () => {
    const { host, store } = partsHost(false);
    await writeWorkbookPart(host, new Uint8Array([1, 2, 3]), "x.xlsx");
    expect(store.size).toBe(0); // nothing written
    expect(await readWorkbookPart(host)).toBeNull();
  });

  it("falls back to a default name when only the bytes part exists", async () => {
    const { host, store } = partsHost();
    // Simulate a part written by a producer that omitted the name part.
    store.set("workbook.xlsx", new Uint8Array([9]));
    const back = await readWorkbookPart(host);
    expect(back!.name).toBe("workbook.xlsx");
  });
});
