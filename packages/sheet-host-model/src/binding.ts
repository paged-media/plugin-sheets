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
): Binding {
  return { v: BINDING_VERSION, data: { sheet, range, contentVersion } };
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
  };
  if (typeof d.sheet !== "string") return null;
  if (typeof d.range !== "string") return null;
  if (typeof d.contentVersion !== "number" || !Number.isFinite(d.contentVersion))
    return null;
  return {
    v: BINDING_VERSION,
    data: {
      sheet: d.sheet,
      range: d.range,
      contentVersion: d.contentVersion,
    },
  };
}
