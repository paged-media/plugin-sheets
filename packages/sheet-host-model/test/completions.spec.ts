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

// sheet.plugin.formula-bar (host-model half): the PURE prefix-match over the
// engine's function name table. ZERO spreadsheet semantics — the names are the
// engine's (the test feeds a fixed table), this only matches a typed prefix and
// splices a chosen completion. The matching/token/apply contract is pinned
// here; the panel just renders the result.

import { describe, expect, it } from "vitest";

import {
  applyCompletion,
  arityHint,
  completionTokenAt,
  matchFunctions,
  type FunctionEntry,
} from "../src";

const FUNCS: FunctionEntry[] = [
  { name: "SUM", family: "math", minArgs: 1, maxArgs: null },
  { name: "SUMIF", family: "math", minArgs: 2, maxArgs: 3 },
  { name: "SUMIFS", family: "math", minArgs: 3, maxArgs: null },
  { name: "AVERAGE", family: "agg", minArgs: 1, maxArgs: null },
  { name: "VLOOKUP", family: "lookup", minArgs: 3, maxArgs: 4 },
  { name: "IF", family: "logical", minArgs: 2, maxArgs: 3 },
];

describe("sheet_plugin_formula_bar: completion token scan", () => {
  it("captures the name run ending at the caret", () => {
    const t = completionTokenAt("=SU", 3);
    expect(t).toEqual({ text: "SU", start: 1, end: 3 });
  });

  it("captures a name mid-formula after an operator", () => {
    const input = "=A1+SUM";
    const t = completionTokenAt(input, input.length);
    expect(t.text).toBe("SUM");
    expect(input.slice(t.start, t.end)).toBe("SUM");
  });

  it("is empty when the caret sits right after a paren/operator", () => {
    expect(completionTokenAt("=SUM(", 5).text).toBe("");
    expect(completionTokenAt("=1+", 3).text).toBe("");
  });

  it("clamps an out-of-range caret to the input bounds", () => {
    expect(completionTokenAt("=SUM", 99).text).toBe("SUM");
    expect(completionTokenAt("=SUM", -3).text).toBe("");
  });
});

describe("sheet_plugin_formula_bar: prefix matching", () => {
  it("prefix-matches case-insensitively", () => {
    const m = matchFunctions(FUNCS, "su");
    expect(m.map((e) => e.name)).toEqual(["SUM", "SUMIF", "SUMIFS"]);
  });

  it("surfaces an EXACT match first (SUM before SUMIF)", () => {
    const m = matchFunctions(FUNCS, "SUM");
    expect(m[0].name).toBe("SUM");
    expect(m.map((e) => e.name)).toContain("SUMIF");
  });

  it("an empty prefix yields no suggestions (never a full dump)", () => {
    expect(matchFunctions(FUNCS, "")).toEqual([]);
  });

  it("honours the result limit", () => {
    expect(matchFunctions(FUNCS, "s", 2)).toHaveLength(2);
  });

  it("a non-matching prefix yields nothing", () => {
    expect(matchFunctions(FUNCS, "ZZZ")).toEqual([]);
  });
});

describe("sheet_plugin_formula_bar: applying a completion", () => {
  it("splices the name + opening paren over the token, caret after the paren", () => {
    const input = "=SU";
    const token = completionTokenAt(input, input.length);
    const res = applyCompletion(input, token, "SUM");
    expect(res.value).toBe("=SUM(");
    expect(res.caret).toBe(res.value.length);
  });

  it("preserves the tail after the token", () => {
    const input = "=SU+1";
    const token = completionTokenAt(input, 3); // caret after "SU"
    const res = applyCompletion(input, token, "SUM");
    expect(res.value).toBe("=SUM(+1");
    expect(res.value.slice(res.caret)).toBe("+1");
  });
});

describe("sheet_plugin_formula_bar: arity hints", () => {
  it("renders variadic / fixed / ranged arities", () => {
    expect(arityHint({ name: "SUM", family: "", minArgs: 1, maxArgs: null })).toBe("1+");
    expect(arityHint({ name: "IF", family: "", minArgs: 2, maxArgs: 2 })).toBe("2");
    expect(arityHint({ name: "X", family: "", minArgs: 2, maxArgs: 4 })).toBe("2–4");
  });
});
