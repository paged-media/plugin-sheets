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

// The frame-binding envelope — what makes a frame a SHEET frame: a small
// JSON payload attached as this plugin's own page-item metadata (spec
// §8.2: "a sheet frame binds (sheet, range, view options)"). It rides
// the protocol-v33 plugin-metadata carrier, which round-trips IDML as a
// Properties/Label KeyValuePair (host.ts `setMetadata` doc), so the
// binding survives a round-trip through InDesign even with the plugin
// absent. S-08: this small envelope is the ONLY thing persisted; the
// workbook bytes themselves stay in memory (the panel says so).
//
// KEY SHAPE (verified against the plugin-api contract): the host derives
// the metadata namespace from the plugin id — `host.document.setMetadata`
// keys implicitly off `x-paged:<manifest id>` and a bundle can only see/
// write its OWN namespace (host.ts §DocumentSurface). The lower-level
// `setPluginMetadata` MUTATION (the batch path the two-phase lower uses)
// takes an explicit `key`, which the host gate verifies equals this
// plugin's own namespace (see plugin-web insert.ts). So BINDING_KEY here
// MUST equal that derived key: `x-paged:media.paged.sheet`.

/** This plugin's metadata namespace — MUST equal the host's derived key
 *  (`x-paged:<manifest.id>`, id `media.paged.sheet`). The host gate
 *  rejects any other key, so a drift here fails loudly, not silently
 *  (the plugin-web METADATA_KEY precedent). */
export const BINDING_KEY = "x-paged:media.paged.sheet";

/** The current binding envelope version. Migrations are plugin-owned
 *  (PluginMetadataEnvelope.v semantics, host.ts §PluginMetadataEnvelope). */
export const BINDING_VERSION = 1;

/** What a native placed table needs to be found and refreshed by a LATER
 *  session (Wave 9): the story and table it lives in, a hash of the content
 *  it shows (so a session can tell whether the page is current) and the
 *  column widths it was sized with. Additive inside the v1 envelope: an
 *  older binding simply has none. */
export interface TableRecord {
  /** The table's story and id. ABSENT on a table placed in ONE batch (core
   *  66 in-batch handles): the binding rides the batch that mints them and a
   *  `$h:` reference is never rewritten inside a metadata VALUE, so a later
   *  session reads them off the page instead (the frame's story; the table
   *  under its first cell). The first refresh re-stamps them. Both or
   *  neither. */
  storyId?: string;
  tableId?: string;
  /** {@link contentHash} of the LoweredContent the table shows. */
  hash: string;
  /** The column widths (pt) the table carries. */
  widths: number[];
}

/** The binding payload: which sheet + range the frame projects, and the
 *  workbook content version it was lowered from (so a stale frame can be
 *  detected and re-lowered). Plain JSON — it is the `data` of the
 *  metadata envelope. */
export interface BindingData {
  /** The bound worksheet name (the user-facing tab name). */
  sheet: string;
  /** The bound A1 range (e.g. `"A1:D20"`). Resolved + validated in Rust;
   *  carried here as the opaque string the engine round-trips. */
  range: string;
  /** The workbook content version this lowered output reflects. */
  contentVersion: number;
  /** The native table this frame holds (Wave 9; absent on the tab-text
   *  lane and on bindings written before it). */
  table?: TableRecord;
}

/** The full envelope as it sits on the page item: a versioned wrapper
 *  around the binding data. Shape matches PluginMetadataEnvelope's
 *  `{ v, data }` (host.ts) so it serialises straight through
 *  `setMetadata` / `setPluginMetadata`. */
export interface Binding {
  v: typeof BINDING_VERSION;
  data: BindingData;
}

/** Build a binding envelope from its parts (pure constructor). */
export function makeBinding(
  sheet: string,
  range: string,
  contentVersion: number,
  table?: TableRecord,
): Binding {
  return {
    v: BINDING_VERSION,
    data: table ? { sheet, range, contentVersion, table } : { sheet, range, contentVersion },
  };
}

/** A short, stable hash of a value's JSON (FNV-1a, 32-bit, hex) — what a
 *  table record compares to tell whether the page shows the content the
 *  workbook lowers to now. Not cryptographic: it guards a refresh, it does
 *  not authenticate anything. */
export function contentHash(value: unknown): string {
  const text = JSON.stringify(value) ?? "";
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) {
    h ^= text.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, "0");
}

/** Defensive parse of a {@link TableRecord}; null unless well-formed. */
function parseTableRecord(input: unknown): TableRecord | null {
  if (typeof input !== "object" || input === null) return null;
  const t = input as Record<string, unknown>;
  const ids = t.storyId !== undefined || t.tableId !== undefined;
  if (ids && (typeof t.storyId !== "string" || typeof t.tableId !== "string")) return null;
  if (typeof t.hash !== "string") return null;
  if (!Array.isArray(t.widths) || !t.widths.every((w) => typeof w === "number" && Number.isFinite(w))) {
    return null;
  }
  const widths = [...(t.widths as number[])];
  return ids
    ? { storyId: t.storyId as string, tableId: t.tableId as string, hash: t.hash, widths }
    : { hash: t.hash, widths };
}

/** Defensive parse: accept only a well-formed binding envelope, else
 *  `null`. Never throws on garbage — a foreign / corrupt / older-shape
 *  metadata blob must degrade to "not a sheet frame", not crash the
 *  panel (the same robustness rule web-model's linter follows). */
export function parseBinding(input: unknown): Binding | null {
  if (typeof input !== "object" || input === null) return null;
  const env = input as { v?: unknown; data?: unknown };
  if (env.v !== BINDING_VERSION) return null;
  if (typeof env.data !== "object" || env.data === null) return null;
  const d = env.data as {
    sheet?: unknown;
    range?: unknown;
    contentVersion?: unknown;
    table?: unknown;
  };
  if (typeof d.sheet !== "string") return null;
  if (typeof d.range !== "string") return null;
  if (typeof d.contentVersion !== "number" || !Number.isFinite(d.contentVersion))
    return null;
  const table = d.table === undefined ? null : parseTableRecord(d.table);
  return {
    v: BINDING_VERSION,
    data: {
      sheet: d.sheet,
      range: d.range,
      contentVersion: d.contentVersion,
      // A malformed record is dropped, never the binding.
      ...(table ? { table } : {}),
    },
  };
}
