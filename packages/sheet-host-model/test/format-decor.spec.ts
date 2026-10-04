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

// Wave 6 — the formatting facets the engine now lowers (underline,
// vertical alignment, border line weight + colour) reach the document as
// native cell / character properties.

import { describe, expect, it } from "vitest";

import type { Mutation } from "@paged-media/plugin-api";

import type { LoweredContent } from "../src";
import {
  CELL_EDGE_STROKE_PT,
  cellFillSwatchOps,
  styleProps,
  tableDecorOps,
  tableRefreshOps,
} from "../src";

const FORMATTED: LoweredContent = {
  cols: [{ index: 0, widthPt: 60 }],
  rows: [
    { index: 0, heightPt: 30, cells: [{ col: 0, text: "Head", align: "center", styleKey: 1 }] },
  ],
  rules: { h: [], v: [] },
  merges: [],
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
      bold: true,
      italic: false,
      underline: true,
      vAlign: "center",
      wrap: true,
      borderTop: true,
      borderRight: false,
      borderBottom: true,
      borderLeft: false,
      borderLines: {
        top: { style: "medium", weightPt: 1.5, rgb: "#ff0000" },
        bottom: { style: "thin", weightPt: 0.75 },
      },
    },
  ],
};

type Loose = { op: string; args: { path?: string; value?: unknown; elementId?: { id: unknown } } };
const loose = (ops: Mutation[]) => ops as unknown as Loose[];
const byPath = (ops: Mutation[], path: string) =>
  loose(ops).filter((o) => o.op === "setElementProperty" && o.args.path === path);

describe("Wave 6 cell decor: border lines, colours, vertical alignment [sheet.format.cell-style]", () => {
  it("each border carries its own weight; a coloured edge names a minted swatch", () => {
    const { ops } = tableDecorOps(FORMATTED, "Story/u1", "t1");
    expect(byPath(ops, "cellTopEdgeStrokeWeight")[0].args.value).toEqual({
      type: "length",
      value: 1.5,
    });
    expect(byPath(ops, "cellBottomEdgeStrokeWeight")[0].args.value).toEqual({
      type: "length",
      value: 0.75,
    });
    expect(byPath(ops, "cellTopEdgeStrokeColor")[0].args.value).toEqual({
      type: "colorRef",
      value: "Color/uPagedSheetCellStrokeFF0000",
    });
    expect(byPath(ops, "cellBottomEdgeStrokeColor")).toHaveLength(0);
    const mints = loose(cellFillSwatchOps(FORMATTED));
    expect(mints.some((m) => JSON.stringify(m).includes("Color/uPagedSheetCellStrokeFF0000"))).toBe(
      true,
    );
  });

  it("vertical alignment becomes cellVerticalJustification", () => {
    const { ops } = tableDecorOps(FORMATTED, "Story/u1", "t1");
    expect(byPath(ops, "cellVerticalJustification")[0].args.value).toEqual({
      type: "text",
      value: "CenterAlign",
    });
  });

  it("underline is a character facet", () => {
    const props = styleProps(FORMATTED.styles![1]);
    expect(props).toContainEqual({ path: "characterUnderline", value: { type: "bool", value: true } });
  });

  it("a border without a line record keeps the hairline (older IR)", () => {
    const old: LoweredContent = {
      ...FORMATTED,
      styles: [FORMATTED.styles![0], { ...FORMATTED.styles![1], borderLines: undefined, vAlign: null }],
    };
    const { ops } = tableDecorOps(old, "Story/u1", "t1");
    expect(byPath(ops, "cellTopEdgeStrokeWeight")[0].args.value).toEqual({
      type: "length",
      value: CELL_EDGE_STROKE_PT,
    });
    expect(byPath(ops, "cellVerticalJustification")).toHaveLength(0);
  });

  it("a refresh that drops the facets resets colour and justification", () => {
    const plain: LoweredContent = {
      ...FORMATTED,
      rows: [{ index: 0, heightPt: 30, cells: [{ col: 0, text: "Head", align: "left", styleKey: 0 }] }],
    };
    const r = tableRefreshOps(FORMATTED, plain, "Story/u1", "t1", [60], [60]);
    expect(byPath(r.decor, "cellTopEdgeStrokeColor")[0].args.value).toEqual({
      type: "colorRef",
      value: null,
    });
    expect(byPath(r.decor, "cellVerticalJustification")[0].args.value).toEqual({
      type: "text",
      value: "",
    });
  });
});
