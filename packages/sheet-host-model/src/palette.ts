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

// THE WORKBOOK PALETTE — the document swatches paged.sheet mints, in one
// place, so the panel that SHOWS them and the lowering that CREATES them
// can never disagree about an id.
//
// WHY THIS FILE EXISTS AT ALL (ADR 023, the colour/scope proof consumer).
// paged.sheet has been minting real document swatches from PRODUCTION
// code for a long time — `chart.ts` emits one `createSwatch` per distinct
// chart colour and `lower-to-mutations.ts` one per distinct data-bar
// colour — because a `frameFillColor` takes a `Color/<id>` ref, not an
// inline hex. What it never had was a SURFACE: no panel could show that
// palette, rename an entry or recolour one, because the platform's only
// two ways to put a panel on screen both mint a NEW panel. ADR 023's
// binding-provider seam is that surface, and this module is the pure
// half of it: workbook facts in, `SwatchSummary`/`SwatchSpec` out.
//
// THE ID CONVENTION IS THE CONTRACT. `Color/uPagedSheet<Facet><HEX>` is
// deterministic and content-addressed, which is what makes one colour
// reuse ONE swatch across primitives, across re-lowers, and — now —
// across the panel. Both minting call sites were carrying their own copy
// of the convention; a provider serving ids the lowering does not mint
// would hand the host a swatch id core cannot resolve, so the two copies
// became one and the lowering imports it from here.
//
// PURE. Data in, descriptors out — no host calls, no engine calls, no
// spreadsheet semantics (the colours are DECIDED in Rust: `fill_rgb` /
// `text_rgb` on `LoweredStyle`, the chart palette in `sheet-chart`).

import type { SwatchSpec, SwatchSummary } from "@paged-media/plugin-api";

import type { ChartGeometry } from "./chart";
import type { LoweredContent } from "./lowered";

/** Which part of the workbook a palette colour came from. The facet is
 *  part of the swatch ID, not decoration: a chart blue and a data-bar
 *  blue are different document swatches on purpose, so recolouring the
 *  chart palette does not silently move every conditional-format bar. */
export type PaletteFacet = "chart" | "dataBar";

/** One entry of the workbook palette: a colour paged.sheet mints (or
 *  would mint) as a document swatch.
 *
 *  `selfId` is a REAL document swatch id — the same one the lowering
 *  emits — because the vocabulary rule obliges a binding provider to
 *  serve core's own row shape, and because the host's colour chip is
 *  resolved by a separate core RPC keyed on exactly this id. */
export interface PaletteEntry {
  selfId: string;
  name: string;
  facet: PaletteFacet;
  /** Canonical uppercase 6-digit hex body, WITHOUT the leading `#`. */
  hex: string;
  /** 0..255 RGB channels — the IDML convention core's `SwatchSpec` takes. */
  rgb: [number, number, number];
}

/** Normalise a `#RGB` / `#RRGGBB` (any case) to the canonical uppercase
 *  6-digit body WITHOUT `#`, or `null` when it is not a well-formed hex
 *  colour. A malformed colour degrades to "no swatch", never a throw —
 *  the same defensive guard both minting call sites already had, now
 *  shared so they cannot normalise differently. 3-digit shorthand
 *  expands per CSS (`#abc` → `AABBCC`). */
export function normalizePaletteHex(hex: string): string | null {
  const m = /^#?([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$/.exec(hex.trim());
  if (!m) return null;
  let body = m[1];
  if (body.length === 3) {
    body = body[0] + body[0] + body[1] + body[1] + body[2] + body[2];
  }
  return body.toUpperCase();
}

/** RGB channel triple (0..255) from a canonical 6-digit hex body. */
export function paletteRgb255(canonHex: string): [number, number, number] {
  return [
    parseInt(canonHex.slice(0, 2), 16),
    parseInt(canonHex.slice(2, 4), 16),
    parseInt(canonHex.slice(4, 6), 16),
  ];
}

/** The `u`-prefixed local part follows core's `Color/u<...>` minted-id
 *  convention; the hex keeps it stable AND human-recognisable in the
 *  Swatches panel. */
const FACET_PREFIX: Record<PaletteFacet, string> = {
  chart: "Color/uPagedSheetChart",
  dataBar: "Color/uPagedSheetDataBar",
};

const FACET_LABEL: Record<PaletteFacet, string> = {
  chart: "paged.sheet chart",
  dataBar: "paged.sheet data bar",
};

/** THE deterministic document-swatch id for a workbook colour. The one
 *  implementation — `chart.ts`, `lower-to-mutations.ts` and the binding
 *  provider all route through it. */
export function paletteSwatchId(facet: PaletteFacet, canonHex: string): string {
  return `${FACET_PREFIX[facet]}${canonHex}`;
}

/** THE swatch NAME for a workbook colour, likewise shared. */
export function paletteSwatchName(
  facet: PaletteFacet,
  canonHex: string,
): string {
  return `${FACET_LABEL[facet]} ${canonHex}`;
}

/** Build one palette entry from an already-canonical hex body. */
export function paletteEntry(
  facet: PaletteFacet,
  canonHex: string,
): PaletteEntry {
  return {
    selfId: paletteSwatchId(facet, canonHex),
    name: paletteSwatchName(facet, canonHex),
    facet,
    hex: canonHex,
    rgb: paletteRgb255(canonHex),
  };
}

/** The DISTINCT chart colours (fills + strokes) across a chart geometry,
 *  in first-appearance order, as canonical hex bodies. Deterministic —
 *  the property `chart.ts`'s §8.4 swatch coherence rests on. */
export function distinctChartHexes(geom: ChartGeometry): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  const add = (c: string | null) => {
    if (c == null) return;
    const h = normalizePaletteHex(c);
    if (h == null || seen.has(h)) return;
    seen.add(h);
    out.push(h);
  };
  for (const prim of geom.prims) {
    switch (prim.kind) {
      case "rect":
        add(prim.fill);
        add(prim.stroke);
        break;
      case "line":
        add(prim.stroke);
        break;
      case "polygon":
        add(prim.fill);
        add(prim.stroke);
        break;
      case "wedge":
        add(prim.fill);
        add(prim.stroke);
        break;
      // text carries no fill/stroke colour in the IR.
    }
  }
  return out;
}

/** The distinct data-bar colours across a lowered region, in
 *  first-appearance order, as canonical hex bodies. */
export function distinctDataBarHexes(content: LoweredContent): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const bar of content.databars ?? []) {
    const h = normalizePaletteHex(bar.fill);
    if (h == null || seen.has(h)) continue;
    seen.add(h);
    out.push(h);
  }
  return out;
}

/** What the palette is derived FROM. Both members are optional: a
 *  chartless workbook still has data bars, and a table without
 *  conditional formatting still has charts. */
export interface PaletteSources {
  /** Every parsed chart's geometry, in workbook chart order. */
  charts?: readonly ChartGeometry[];
  /** The lowered region(s) currently on the page — data bars live here. */
  regions?: readonly LoweredContent[];
}

/**
 * THE WORKBOOK PALETTE: every colour paged.sheet mints as a document
 * swatch, deduped by swatch id, in a deterministic order (charts in
 * chart order, then data bars in region order).
 *
 * WHAT IS DELIBERATELY NOT IN HERE, and it is the honest half of this
 * slice. A workbook's CELL fill and CELL text colours
 * (`LoweredStyle.fillRgb` / `.textRgb`) are NOT palette entries, because
 * paged.sheet does not mint swatches for them: `lower-to-table.ts` and
 * `lower-to-mutations.ts` pass those raw hex strings straight into a
 * `{type:"colorRef"}` value, which core resolves by SWATCH ID
 * (`Graphic::resolve`) and therefore cannot resolve at all. Serving them
 * through the binding provider would mean handing the host swatch ids
 * that name nothing — an unresolvable chip in a COLOUR panel, which is
 * the exact class of lie the platform refuses. They stay out until the
 * lowering mints them, which is a change to production lowering output
 * and belongs in its own slice.
 */
export function workbookPalette(sources: PaletteSources): PaletteEntry[] {
  const byId = new Map<string, PaletteEntry>();
  const push = (facet: PaletteFacet, canonHex: string) => {
    const entry = paletteEntry(facet, canonHex);
    if (!byId.has(entry.selfId)) byId.set(entry.selfId, entry);
  };
  for (const geom of sources.charts ?? []) {
    for (const hex of distinctChartHexes(geom)) push("chart", hex);
  }
  for (const region of sources.regions ?? []) {
    for (const hex of distinctDataBarHexes(region)) push("dataBar", hex);
  }
  return [...byId.values()];
}

/** Project a palette entry onto core's `swatches` ROW shape. `kind`
 *  mirrors what core reports for a minted process colour, so the host's
 *  one row renderer draws a provider row and a core row identically —
 *  the vocabulary rule applied to collections. */
export function paletteEntryToSummary(entry: PaletteEntry): SwatchSummary {
  return { selfId: entry.selfId, name: entry.name, kind: "process" };
}

/** Project a palette entry onto core's `SwatchSpec` — the payload
 *  `createSwatch` / `editSwatch` carry. RGB process at 0..255 channels
 *  (the IDML convention core expects). */
export function paletteEntryToSpec(entry: PaletteEntry): SwatchSpec {
  return {
    selfId: entry.selfId,
    name: entry.name,
    space: "RGB",
    value: [...entry.rgb],
    model: "Process",
  };
}
