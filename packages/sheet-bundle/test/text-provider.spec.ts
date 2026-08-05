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

// ADR 023 phase D — the paged.sheet CHARACTER/PARAGRAPH binding
// provider: the VALUE axis, which is the axis neither the Layers nor
// the Swatches proof can reach.
//
// A layer row has ONE visibility and a swatch list is a list; only a
// range of cells can be MIXED. So the assertions here are about the
// collapse, and about the three answers that are NOT a value:
//
//   · `mixed`   — the cells disagree. Never a winner, and never the
//                 first one.
//   · `absent`  — owned, and nothing to say. Must NOT fall through: the
//                 text caret is independent of the edit-context stack,
//                 so core's answer would be some unrelated paragraph's.
//   · `decline` — no workbook / nothing lowered. NOT `absent`: there is
//                 no cell here to own.
//
// Pure: a hand-rolled source, no engine, no editor.

import { describe, expect, it } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";
import type { LoweredContent, LoweredStyle } from "../../sheet-host-model/src";

import {
  ABSENT_PATHS,
  SERVED_PATHS,
  VALUED_PATHS,
  collapseCells,
  makeTextBindingProvider,
  stylesOfRange,
} from "../src/binding-provider/text-provider";

const silentHost = {
  log: { debug() {}, info() {}, warn() {}, error() {} },
} as unknown as BundleHost;

/** A style table entry — the mirror of the Rust `LoweredStyle`. */
function style(key: number, over: Partial<LoweredStyle> = {}): LoweredStyle {
  return {
    key,
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
    ...over,
  };
}

/** A lowered range whose cells carry the given style keys, row-major.
 *  Geometry is irrelevant here — the provider reads styles only. */
function content(styles: LoweredStyle[], keys: number[][]): LoweredContent {
  return {
    cols: keys[0].map((_, i) => ({ index: i, widthPt: 60 })),
    rows: keys.map((row, r) => ({
      index: r,
      heightPt: 14,
      cells: row.map((styleKey, c) => ({
        col: c,
        text: `r${r}c${c}`,
        align: "left" as const,
        styleKey,
      })),
    })),
    rules: { h: [], v: [] },
    merges: [],
    styles,
  };
}

const DEFAULT = style(0);
const BIG = style(1, { bold: true, fontSizePt: 18, fontName: "Georgia" });
const SMALL = style(2, { italic: true, fontSizePt: 9, fontName: "Verdana" });

function providerOver(
  styles: LoweredStyle[],
  keys: number[][],
  range = "A1:B1",
) {
  return makeTextBindingProvider(silentHost, {
    textSelectionRange: () => ({ sheet: 0, range }),
    lowerRange: () => content(styles, keys),
  }).provider;
}

const SELECTION = { kind: "selection", scope: "content" } as const;

describe("paged.sheet text provider — the declaration", () => {
  it("declares the WHOLE Character/Paragraph surface, and writes NONE of it", () => {
    const p = providerOver([DEFAULT, BIG], [[1]]);
    // Owning the selection means owning the whole panel surface — see
    // the module header. A path left undeclared would DECLINE, and a
    // decline falls through to core.
    expect(p.provides.paths).toEqual(SERVED_PATHS);
    expect(SERVED_PATHS.length).toBe(
      VALUED_PATHS.length + ABSENT_PATHS.length,
    );
    // The read/write split: everything read, nothing written. The
    // engine has no cell-style write API, and DECLARING that is what
    // keeps the host's controls read-only instead of fake-interactive.
    expect(p.provides.writablePaths).toEqual([]);
    // The mirror does not even carry a `writeProperty` lane — see
    // `adr023-seam.ts`: neither of this repo's providers implements one,
    // and an unimplemented lane is not declared.
    expect("writeProperty" in p).toBe(false);
    expect(p.applyMutation).toBeUndefined();
  });

  it("serves NO collections and NO ops — this is not a second Swatches provider", () => {
    const p = providerOver([DEFAULT, BIG], [[1]]);
    expect(p.provides.collections).toBeUndefined();
    expect(p.provides.ops).toBeUndefined();
  });

  it("`characterFillColor` is DECLARED but not VALUED — nothing mints the swatch it names", () => {
    // Since `c321642` the value is EXPRESSIBLE (a minted swatch id, not
    // the raw hex that used to name nothing and render as the default
    // text colour). It stays absent because nothing applies the
    // emission that mints it: a colour chip for a swatch no document
    // carries is the same lie the Swatches slice marked
    // `data-swatch-preview="unresolved"` from the other side.
    expect(VALUED_PATHS).not.toContain("characterFillColor");
    expect(ABSENT_PATHS).toContain("characterFillColor");
  });
});

describe("paged.sheet text provider — reads", () => {
  it("a UNIFORM selection answers with the value", async () => {
    const p = providerOver([DEFAULT, BIG], [[1, 1]]);
    expect(
      await p.readProperty!({ path: "characterFontSize", target: SELECTION }),
    ).toEqual({ kind: "value", value: { type: "length", value: 18 } });
    expect(
      await p.readProperty!({ path: "characterFontStyle", target: SELECTION }),
    ).toEqual({ kind: "value", value: { type: "text", value: "Bold" } });
    expect(
      await p.readProperty!({ path: "characterFontFamily", target: SELECTION }),
    ).toEqual({ kind: "value", value: { type: "text", value: "Georgia" } });
  });

  it("cells that DISAGREE are MIXED — never the first one, never a winner", async () => {
    const p = providerOver([DEFAULT, BIG, SMALL], [[1, 2]]);
    for (const path of VALUED_PATHS) {
      expect(await p.readProperty!({ path, target: SELECTION })).toEqual({
        kind: "mixed",
      });
    }
  });

  it("SOME declare and some do not: also MIXED, not the declared one", async () => {
    // A cell with no override inherits the workbook default, which by
    // construction is NOT 18pt — the engine records an override only
    // when it differs from the default font. So they genuinely
    // disagree; rounding that to "18 pt" is the same silent winner in a
    // subtler costume.
    const p = providerOver([DEFAULT, BIG], [[1, 0]]);
    expect(
      await p.readProperty!({ path: "characterFontSize", target: SELECTION }),
    ).toEqual({ kind: "mixed" });
  });

  it("nobody declares: ABSENT with a reason, not mixed and not a fall-through", async () => {
    const p = providerOver([DEFAULT], [[0, 0]]);
    const read = await p.readProperty!({
      path: "characterFontSize",
      target: SELECTION,
    });
    expect(read.kind).toBe("absent");
    expect((read as { reason: string }).reason).toMatch(/workbook default/);
  });

  it("a path a CELL does not model is ABSENT — the conflation this axis exists to prevent", async () => {
    const p = providerOver([DEFAULT, BIG], [[1]]);
    for (const path of ["characterLeading", "paragraphRuleAbove"] as const) {
      const read = await p.readProperty!({ path, target: SELECTION });
      // NOT `decline`. A decline continues down the stack and then to
      // core, and core's content selection is the TEXT CARET's, which
      // entering this context did not clear.
      expect(read.kind).toBe("absent");
    }
  });

  it("EMPTY cells are skipped, not counted as a disagreement", async () => {
    // The IR is sparse: an empty cell is simply absent from the row. An
    // empty cell has no text, so no text formatting to disagree about.
    const sparse: LoweredContent = {
      ...content([DEFAULT, BIG], [[1, 1]]),
      rows: [
        { index: 0, heightPt: 14, cells: [{ col: 0, text: "x", align: "left", styleKey: 1 }] },
        { index: 1, heightPt: 14, cells: [] },
      ],
    };
    const p = makeTextBindingProvider(silentHost, {
      textSelectionRange: () => ({ sheet: 0, range: "A1:A2" }),
      lowerRange: () => sparse,
    }).provider;
    expect(
      await p.readProperty!({ path: "characterFontSize", target: SELECTION }),
    ).toEqual({ kind: "value", value: { type: "length", value: 18 } });
  });

  it("no workbook / nothing lowered DECLINES — there is no cell here to own", async () => {
    const p = makeTextBindingProvider(silentHost, {
      textSelectionRange: () => null,
      lowerRange: () => null,
    }).provider;
    const read = await p.readProperty!({
      path: "characterFontSize",
      target: SELECTION,
    });
    expect(read.kind).toBe("decline");
  });

  it("a lowering that THROWS declines rather than wedging the panel", async () => {
    const p = makeTextBindingProvider(silentHost, {
      textSelectionRange: () => ({ sheet: 0, range: "A1" }),
      lowerRange: () => {
        throw new Error("engine gone");
      },
    }).provider;
    expect(
      (await p.readProperty!({ path: "characterFontSize", target: SELECTION }))
        .kind,
    ).toBe("decline");
  });

  it("a ROW- or ELEMENT-scoped read is declined — this provider answers for the SELECTION", async () => {
    const p = providerOver([DEFAULT, BIG], [[1]]);
    expect(
      (
        await p.readProperty!({
          path: "characterFontSize",
          target: { kind: "element", id: { kind: "rectangle", id: "u1" } },
        })
      ).kind,
    ).toBe("decline");
    expect(
      (
        await p.readProperty!({
          path: "characterFontSize",
          target: { kind: "row", collection: "layers", id: "L1" },
        })
      ).kind,
    ).toBe("decline");
  });

  it("the ELEMENT scope is served too — the target is the SELECTION, not the caret", async () => {
    // `{scope:"element"}` and `{scope:"content"}` are the same cells
    // here: the sheet has one selection, and it is neither the host's
    // page-item selection nor its text caret.
    const p = providerOver([DEFAULT, BIG], [[1]]);
    expect(
      await p.readProperty!({
        path: "characterFontSize",
        target: { kind: "selection", scope: "element" },
      }),
    ).toEqual({ kind: "value", value: { type: "length", value: 18 } });
  });
});

describe("paged.sheet text provider — the pure pieces", () => {
  it("collapseCells: agreement, disagreement, partial declaration, silence", () => {
    const a = { type: "length" as const, value: 18 };
    const b = { type: "length" as const, value: 9 };
    expect(collapseCells([a, { ...a }], "characterFontSize")).toEqual({
      kind: "value",
      value: a,
    });
    expect(collapseCells([a, b], "characterFontSize")).toEqual({
      kind: "mixed",
    });
    expect(collapseCells([a, undefined], "characterFontSize")).toEqual({
      kind: "mixed",
    });
    expect(
      collapseCells([undefined, undefined], "characterFontSize").kind,
    ).toBe("absent");
    expect(collapseCells([], "characterFontSize").kind).toBe("absent");
  });

  it("stylesOfRange walks row-major and defaults a keyless cell to key 0", () => {
    const c = content([DEFAULT, BIG, SMALL], [[1, 2], [0, 1]]);
    expect(stylesOfRange(c).map((s) => s.key)).toEqual([1, 2, 0, 1]);
  });
});
