// sheet.plugin.binding.metadata — the frame-binding envelope: round-trip
// through make/parse, the fixed namespace key, and defensive rejection
// of every malformed shape (a foreign/corrupt metadata blob degrades to
// "not a sheet frame", never a crash).

import { describe, expect, it } from "vitest";

import {
  BINDING_KEY,
  BINDING_VERSION,
  makeBinding,
  parseBinding,
} from "../src";

describe("sheet_plugin_binding_metadata: key + version", () => {
  it("the binding key is this plugin's own derived namespace", () => {
    expect(BINDING_KEY).toBe("x-paged:media.paged.sheet");
  });
  it("the envelope version is 1", () => {
    expect(BINDING_VERSION).toBe(1);
  });
});

describe("sheet_plugin_binding_metadata: round-trip", () => {
  it("make → parse is the identity for a valid binding", () => {
    const b = makeBinding("Budget 2026", "A1:F40", 12);
    expect(b).toEqual({
      v: 1,
      data: { sheet: "Budget 2026", range: "A1:F40", contentVersion: 12 },
    });
    expect(parseBinding(b)).toEqual(b);
  });

  it("survives JSON serialisation (the metadata carrier form)", () => {
    const b = makeBinding("Sheet1", "B2:C9", 0);
    expect(parseBinding(JSON.parse(JSON.stringify(b)))).toEqual(b);
  });
});

describe("sheet_plugin_binding_metadata: rejects garbage", () => {
  const garbage: Array<[string, unknown]> = [
    ["null", null],
    ["undefined", undefined],
    ["a number", 42],
    ["a string", "A1:B2"],
    ["an array", [1, 2, 3]],
    ["wrong version", { v: 2, data: { sheet: "S", range: "A1", contentVersion: 1 } }],
    ["missing data", { v: 1 }],
    ["data not an object", { v: 1, data: "A1" }],
    ["missing sheet", { v: 1, data: { range: "A1", contentVersion: 1 } }],
    ["sheet not a string", { v: 1, data: { sheet: 1, range: "A1", contentVersion: 1 } }],
    ["missing range", { v: 1, data: { sheet: "S", contentVersion: 1 } }],
    ["range not a string", { v: 1, data: { sheet: "S", range: 5, contentVersion: 1 } }],
    ["contentVersion not a number", { v: 1, data: { sheet: "S", range: "A1", contentVersion: "x" } }],
    ["contentVersion NaN", { v: 1, data: { sheet: "S", range: "A1", contentVersion: NaN } }],
  ];

  for (const [label, value] of garbage) {
    it(`returns null for ${label}`, () => {
      expect(parseBinding(value)).toBeNull();
    });
  }

  it("strips extra fields — parse normalises to the contract shape", () => {
    const dirty = {
      v: 1,
      data: { sheet: "S", range: "A1:B2", contentVersion: 3, extra: "ignored" },
      stray: true,
    };
    expect(parseBinding(dirty)).toEqual({
      v: 1,
      data: { sheet: "S", range: "A1:B2", contentVersion: 3 },
    });
  });
});
