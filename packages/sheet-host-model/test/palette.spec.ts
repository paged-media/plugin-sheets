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

// The WORKBOOK PALETTE (ADR 023 colour/scope consumer) — the pure
// derivation, plus the property the whole slice rests on: the ids the
// panel SHOWS are the ids the lowering MINTS.

import { describe, expect, it } from "vitest";

import type { Mutation, SwatchSpec } from "@paged-media/plugin-api";

import { chartGeometryToMutations, type ChartGeometry } from "../src/chart";
import { cellFillSwatchOps } from "../src/lower-to-table";
import { cellTextSwatchOps, lowerToMutations } from "../src/lower-to-mutations";
import { makeBinding, type Binding } from "../src/binding";
import type { LoweredContent } from "../src/lowered";
import {
  distinctCellFillHexes,
  distinctCellTextHexes,
  distinctChartHexes,
  distinctDataBarHexes,
  normalizePaletteHex,
  paletteEntry,
  paletteEntryToSpec,
  paletteEntryToSummary,
  paletteRgb255,
  paletteSwatchId,
  swatchMintOps,
  workbookPalette,
} from "../src/palette";

const BINDING: Binding = makeBinding("Sheet1", "A1:B2", 1);

function chartGeom(prims: ChartGeometry["prims"]): ChartGeometry {
  return { widthPt: 200, heightPt: 120, prims };
}

/** The `createSwatch` specs inside a one-`batch` mutation, in order. */
function mintedSpecs(batch: Mutation): SwatchSpec[] {
  const ops = (batch.args as { ops?: Mutation[] }).ops ?? [];
  return ops
    .filter((m) => m.op === "createSwatch")
    .map((m) => (m.args as { spec: SwatchSpec }).spec);
}

function content(over: Partial<LoweredContent> = {}): LoweredContent {
  return {
    cols: [{ index: 0, widthPt: 60 }],
    rows: [{ index: 0, heightPt: 14, cells: [{ col: 0, text: "1", align: "right" }] }],
    rules: { h: [], v: [] },
    merges: [],
    ...over,
  };
}

describe("sheet_plugin_swatch_palette: hex normalisation", () => {
  it("canonicalises case, the leading #, and 3-digit shorthand", () => {
    expect(normalizePaletteHex("#4e79a7")).toBe("4E79A7");
    expect(normalizePaletteHex("4E79A7")).toBe("4E79A7");
    expect(normalizePaletteHex("#abc")).toBe("AABBCC");
    expect(normalizePaletteHex("  #FFF  ")).toBe("FFFFFF");
  });

  it("refuses a malformed colour instead of guessing one", () => {
    // The lowering's defensive guard: a bad colour degrades to "no
    // swatch", never a throw and never a wrong colour.
    expect(normalizePaletteHex("rgb(1,2,3)")).toBeNull();
    expect(normalizePaletteHex("#12345")).toBeNull();
    expect(normalizePaletteHex("")).toBeNull();
  });

  it("splits a canonical hex into 0..255 channels (the IDML convention)", () => {
    expect(paletteRgb255("4E79A7")).toEqual([0x4e, 0x79, 0xa7]);
    expect(paletteRgb255("000000")).toEqual([0, 0, 0]);
    expect(paletteRgb255("FFFFFF")).toEqual([255, 255, 255]);
  });
});

describe("sheet_plugin_swatch_palette: facets are distinct swatches", () => {
  it("keys a chart colour and a data-bar colour to DIFFERENT ids", () => {
    // Deliberate: recolouring the chart palette must not silently move
    // every conditional-format bar that happens to share the hue.
    expect(paletteSwatchId("chart", "4E79A7")).toBe(
      "Color/uPagedSheetChart4E79A7",
    );
    expect(paletteSwatchId("dataBar", "4E79A7")).toBe(
      "Color/uPagedSheetDataBar4E79A7",
    );
  });

  it("keys CELL FILL and CELL TEXT to their own ids too", () => {
    // A cell's background and its glyph colour are different document
    // swatches for the same reason chart and data bar are: recolouring
    // one must not move the other.
    expect(paletteSwatchId("cellFill", "FFFF00")).toBe(
      "Color/uPagedSheetCellFillFFFF00",
    );
    expect(paletteSwatchId("cellText", "FFFF00")).toBe(
      "Color/uPagedSheetCellTextFFFF00",
    );
  });

  it("projects an entry onto core's swatch ROW shape and SPEC shape", () => {
    const entry = paletteEntry("chart", "4E79A7");
    expect(paletteEntryToSummary(entry)).toEqual({
      selfId: "Color/uPagedSheetChart4E79A7",
      name: "paged.sheet chart 4E79A7",
      kind: "process",
    });
    expect(paletteEntryToSpec(entry)).toEqual({
      selfId: "Color/uPagedSheetChart4E79A7",
      name: "paged.sheet chart 4E79A7",
      space: "RGB",
      value: [0x4e, 0x79, 0xa7],
      model: "Process",
    });
  });
});

describe("sheet_plugin_swatch_palette: distinct colours, in order", () => {
  it("collects chart fills + strokes in first-appearance order, deduped", () => {
    const geom = chartGeom([
      { kind: "rect", x: 0, y: 0, w: 10, h: 10, fill: "#4E79A7", stroke: null, strokeW: 0 },
      { kind: "rect", x: 0, y: 0, w: 10, h: 10, fill: "#4e79a7", stroke: "#333", strokeW: 1 },
      { kind: "line", pts: [[0, 0], [1, 1]], stroke: "#F28E2B", strokeW: 1 },
    ]);
    expect(distinctChartHexes(geom)).toEqual(["4E79A7", "333333", "F28E2B"]);
  });

  it("collects data-bar fills the same way", () => {
    const c = content({
      databars: [
        { row: 0, col: 0, x: 0, y: 0, w: 5, h: 5, fillFraction: 0.5, fill: "#638EC6" },
        { row: 1, col: 0, x: 0, y: 0, w: 5, h: 5, fillFraction: 0.5, fill: "#638ec6" },
      ],
    });
    expect(distinctDataBarHexes(c)).toEqual(["638EC6"]);
  });

  it("workbookPalette merges both facets, deduped by swatch id", () => {
    const palette = workbookPalette({
      charts: [
        chartGeom([
          { kind: "rect", x: 0, y: 0, w: 1, h: 1, fill: "#4E79A7", stroke: null, strokeW: 0 },
        ]),
      ],
      regions: [
        content({
          databars: [
            { row: 0, col: 0, x: 0, y: 0, w: 5, h: 5, fillFraction: 1, fill: "#4E79A7" },
          ],
        }),
      ],
    });
    // Same hue, two facets ⇒ two swatches, charts first.
    expect(palette.map((e) => e.selfId)).toEqual([
      "Color/uPagedSheetChart4E79A7",
      "Color/uPagedSheetDataBar4E79A7",
    ]);
  });

  it("is EMPTY for a workbook with no charts and no data bars", () => {
    // The provider turns this into a DECLINE, not empty rows — a
    // workbook with nothing to say about colour lets core answer.
    expect(workbookPalette({})).toEqual([]);
    expect(workbookPalette({ regions: [content()] })).toEqual([]);
  });
});

// ── THE PROPERTY THE WHOLE SLICE RESTS ON ──────────────────────────────
//
// The binding provider serves palette ids to the host Swatches panel,
// and the host resolves a chip (and addresses an edit) by DOCUMENT
// SWATCH ID. If the palette and the lowering ever disagreed about an id,
// the panel would show swatches core has never heard of. They cannot
// disagree now, because there is one implementation — and this asserts
// it against the ACTUAL emitted ops rather than against the shared
// helper, so a future divergence fails here.

describe("sheet_plugin_swatch_palette: the panel's ids ARE the lowering's ids", () => {
  it("chart lowering mints exactly the chart palette's swatch specs", () => {
    const geom = chartGeom([
      { kind: "rect", x: 0, y: 0, w: 10, h: 10, fill: "#4E79A7", stroke: "#333333", strokeW: 1 },
      { kind: "line", pts: [[0, 0], [5, 5]], stroke: "#F28E2B", strokeW: 1 },
    ]);
    const { batch } = chartGeometryToMutations(
      geom,
      { pageId: "page-1", bounds: [0, 0, 120, 200] },
      BINDING,
    );
    const minted = mintedSpecs(batch);

    const expected = workbookPalette({ charts: [geom] }).map(paletteEntryToSpec);
    expect(minted).toEqual(expected);
  });

  it("data-bar lowering mints exactly the data-bar palette's swatch specs", () => {
    const c = content({
      databars: [
        { row: 0, col: 0, x: 0, y: 0, w: 20, h: 10, fillFraction: 0.8, fill: "#638EC6" },
        { row: 0, col: 0, x: 0, y: 0, w: 10, h: 10, fillFraction: 0.4, fill: "#FF0000" },
      ],
    });
    const { batch } = lowerToMutations(
      c,
      { pageId: "page-1", bounds: [0, 0, 100, 100] },
      BINDING,
    );
    const minted = mintedSpecs(batch);

    const expected = workbookPalette({ regions: [c] }).map(paletteEntryToSpec);
    expect(minted).toEqual(expected);
  });
});

// The same falsifiable property, extended to the CELL colour axis the
// Swatches slice deliberately left out: a `cellFillColor` /
// `characterFillColor` colorRef is a SWATCH ID, and a render proved an
// unresolvable one paints nothing (fill) or the default colour (text).
// So the ids the lowering REFERENCES must be the ids it MINTS.

describe("sheet_plugin_swatch_palette: cell colours are minted, not raw hex", () => {
  const STYLED = content({
    styles: [
      {
        key: 0,
        bold: false,
        italic: false,
        borderTop: false,
        borderRight: false,
        borderBottom: false,
        borderLeft: false,
      },
      {
        key: 1,
        bold: false,
        italic: false,
        fillRgb: "#ffff00",
        textRgb: "#F00",
        borderTop: false,
        borderRight: false,
        borderBottom: false,
        borderLeft: false,
      },
    ],
  });

  it("reads the distinct cell colours off the styles table (key 0 excluded)", () => {
    expect(distinctCellFillHexes(STYLED)).toEqual(["FFFF00"]);
    // 3-digit shorthand canonicalises, so one colour is one swatch.
    expect(distinctCellTextHexes(STYLED)).toEqual(["FF0000"]);
  });

  it("the cell-fill mints ARE the palette's cell-fill entries", () => {
    const minted = mintedSpecs({
      op: "batch",
      args: { ops: cellFillSwatchOps(STYLED) },
    } as unknown as Mutation);
    const expected = workbookPalette({ regions: [STYLED] })
      .filter((e) => e.facet === "cellFill")
      .map(paletteEntryToSpec);
    expect(minted).toEqual(expected);
    expect(minted).toHaveLength(1);
  });

  it("cell TEXT mints exist but stay OUT of the palette (nothing applies them)", () => {
    // `cellTextSwatchOps` is real and correct; `StyleEmission` has no
    // driver yet, so a palette chip would name a swatch no document has.
    const minted = mintedSpecs({
      op: "batch",
      args: { ops: cellTextSwatchOps(STYLED) },
    } as unknown as Mutation);
    expect(minted.map((s) => s.selfId)).toEqual([
      "Color/uPagedSheetCellTextFF0000",
    ]);
    expect(
      workbookPalette({ regions: [STYLED] }).some(
        (e) => e.facet === "cellText",
      ),
    ).toBe(false);
  });
});

// ── THE MINT GUARD (sheet.lower.swatch-mint-dedupe) ─────────────────────
//
// An idempotent-LOOKING mint inside a batch is not idempotent. Every id
// above is content-addressed, so a second lowering asks for ids the
// document already has — and core refuses a duplicate `createSwatch`,
// which fails the WHOLE `Operation::Batch`. Measured against core's real
// engine over the exact ops these translators emit: lowering a chart
// twice (and lowering ANY second chart, since every chart shares the axis
// grey `#888888`) left the scene tree unchanged at 18 nodes — the second
// chart never landed. The same batch with its mints STRIPPED still
// applied (18 nodes, art present but unpainted), which is why a failed
// read degrades the colour rather than the batch.

describe("sheet_lower_swatch_mint_dedupe: mint only what is absent", () => {
  const HEXES = ["4E79A7", "F28E2B"];

  it("mints every colour when the caller consults no document", () => {
    expect(swatchMintOps("chart", HEXES).map((m) => m.op)).toEqual([
      "createSwatch",
      "createSwatch",
    ]);
  });

  it("SKIPS a colour the document already carries", () => {
    const known = new Set(["Color/uPagedSheetChart4E79A7"]);
    const minted = mintedSpecs({
      op: "batch",
      args: { ops: swatchMintOps("chart", HEXES, known) },
    } as unknown as Mutation);
    expect(minted.map((s) => s.selfId)).toEqual([
      "Color/uPagedSheetChartF28E2B",
    ]);
  });

  it("mints NOTHING when the swatch read FAILED (null ≠ 'there are none')", () => {
    // Minting blind risks a duplicate, and a duplicate takes the whole
    // batch down. Minting nothing costs only colour.
    expect(swatchMintOps("chart", HEXES, null)).toEqual([]);
    expect(swatchMintOps("dataBar", ["638EC6"], null)).toEqual([]);
  });

  it("matches on the FACETED id, so one facet never masks another", () => {
    // A cellFill swatch of the same hue must not suppress the chart mint.
    const known = new Set(["Color/uPagedSheetCellFill4E79A7"]);
    expect(
      swatchMintOps("chart", ["4E79A7"], known).map(
        (m) => (m.args as { spec: SwatchSpec }).spec.selfId,
      ),
    ).toEqual(["Color/uPagedSheetChart4E79A7"]);
  });
});

describe("sheet_lower_swatch_mint_dedupe: every minting lane honours it", () => {
  const GEOM = chartGeom([
    { kind: "rect", x: 0, y: 0, w: 10, h: 10, fill: "#4E79A7", stroke: "#888888", strokeW: 1 },
  ]);
  const BARS = content({
    databars: [
      { row: 0, col: 0, x: 0, y: 0, w: 20, h: 10, fillFraction: 0.8, fill: "#638EC6" },
    ],
  });
  const CELLS = content({
    styles: [
      {
        key: 0,
        bold: false,
        italic: false,
        fontSizePt: null,
        fontName: null,
        fillRgb: null,
        textRgb: null,
        borderTop: false,
        borderRight: false,
        borderBottom: false,
        borderLeft: false,
      },
      {
        key: 1,
        bold: false,
        italic: false,
        fontSizePt: null,
        fontName: null,
        fillRgb: "#FFFF00",
        textRgb: "#F00",
        borderTop: false,
        borderRight: false,
        borderBottom: false,
        borderLeft: false,
      },
    ],
  });

  it("a RE-LOWER of the same chart mints nothing (the batch is not gambled)", () => {
    const first = mintedSpecs(
      chartGeometryToMutations(
        GEOM,
        { pageId: "page-1", bounds: [0, 0, 120, 200] },
        BINDING,
      ).batch,
    );
    expect(first).toHaveLength(2);

    // The document now carries exactly what the first lowering minted.
    const known = new Set(first.map((s) => s.selfId!));
    const { batch } = chartGeometryToMutations(
      GEOM,
      { pageId: "page-1", bounds: [0, 0, 120, 200] },
      BINDING,
      known,
    );
    expect(mintedSpecs(batch)).toEqual([]);
    // The GEOMETRY is untouched — only the redundant mints are gone, and
    // the fill still names the swatch that is already in the document.
    const ops = (batch.args as { ops: Mutation[] }).ops;
    expect(ops.some((m) => m.op === "insertPath")).toBe(true);
    expect(
      ops.some(
        (m) =>
          m.op === "setElementProperty" &&
          (m.args as { path: string; value: { value: string } }).path ===
            "frameFillColor" &&
          (m.args as { value: { value: string } }).value.value ===
            "Color/uPagedSheetChart4E79A7",
      ),
    ).toBe(true);
  });

  it("a SECOND, DIFFERENT chart mints only its NEW colour", () => {
    // The real failure was not the literal re-lower: two different charts
    // share the axis grey, so a workbook's second chart never landed.
    const other = chartGeom([
      { kind: "rect", x: 0, y: 0, w: 10, h: 10, fill: "#3366CC", stroke: "#888888", strokeW: 1 },
    ]);
    const known = new Set([
      "Color/uPagedSheetChart4E79A7",
      "Color/uPagedSheetChart888888",
    ]);
    const minted = mintedSpecs(
      chartGeometryToMutations(
        other,
        { pageId: "page-1", bounds: [0, 0, 120, 200] },
        BINDING,
        known,
      ).batch,
    );
    expect(minted.map((s) => s.selfId)).toEqual([
      "Color/uPagedSheetChart3366CC",
    ]);
  });

  it("the tab-text lane's data bars dedupe, and the FRAME survives a failed read", () => {
    const known = new Set(["Color/uPagedSheetDataBar638EC6"]);
    const deduped = lowerToMutations(
      BARS,
      { pageId: "page-1", bounds: [0, 0, 100, 100] },
      BINDING,
      known,
    ).batch;
    expect(mintedSpecs(deduped)).toEqual([]);

    // A failed read mints nothing — but the frame, the rules and the
    // binding still ride the batch (only the bar colour degrades).
    const blind = lowerToMutations(
      BARS,
      { pageId: "page-1", bounds: [0, 0, 100, 100] },
      BINDING,
      null,
    ).batch;
    expect(mintedSpecs(blind)).toEqual([]);
    const ops = (blind.args as { ops: Mutation[] }).ops;
    expect(ops[0].op).toBe("insertTextFrame");
    expect(ops.some((m) => m.op === "insertPath")).toBe(true);
    expect(ops.some((m) => m.op === "setPluginMetadata")).toBe(true);
  });

  it("the cell-fill and cell-text mint lists dedupe on their own facets", () => {
    expect(
      cellFillSwatchOps(CELLS, new Set(["Color/uPagedSheetCellFillFFFF00"])),
    ).toEqual([]);
    expect(cellFillSwatchOps(CELLS, null)).toEqual([]);
    expect(
      cellTextSwatchOps(CELLS, new Set(["Color/uPagedSheetCellTextFF0000"])),
    ).toEqual([]);
    expect(cellTextSwatchOps(CELLS, null)).toEqual([]);
    // A cellFill id in the known set does NOT suppress the cellText mint.
    expect(
      cellTextSwatchOps(CELLS, new Set(["Color/uPagedSheetCellFillFF0000"])),
    ).toHaveLength(1);
  });
});
