// sheet.plugin.lower.mutations — the two-phase translator (S-03):
// rule-offset math, op ordering, the $created sentinel, and the
// tab/newline join (incl. empty cells). Pure data in, mutations out.

import { describe, expect, it } from "vitest";

import {
  joinText,
  lowerToMutations,
  makeBinding,
  type Binding,
  type LoweredContent,
  type LowerPlacement,
} from "../src";

// A 2x2 region: two 60pt columns, two 20pt rows; one interior h-rule
// and one interior v-rule at the cell boundaries; content-space offsets.
function fixture(): LoweredContent {
  return {
    cols: [
      { index: 0, widthPt: 60 },
      { index: 1, widthPt: 60 },
    ],
    rows: [
      {
        index: 0,
        heightPt: 20,
        cells: [
          { col: 0, text: "A1", align: "left" },
          { col: 1, text: "100", align: "right" },
        ],
      },
      {
        index: 1,
        heightPt: 20,
        cells: [
          { col: 0, text: "A2", align: "left" },
          { col: 1, text: "200", align: "right" },
        ],
      },
    ],
    rules: {
      // an h-rule at y=20 spanning x 0..120
      h: [{ at: 20, from: 0, to: 120 }],
      // a v-rule at x=60 spanning y 0..40
      v: [{ at: 60, from: 0, to: 40 }],
    },
    merges: [],
  };
}

const placement: LowerPlacement = { pageId: "Page/u1", bounds: [24, 30, 64, 150] };
const binding: Binding = makeBinding("Sheet1", "A1:B2", 7);

describe("sheet_plugin_lower_mutations: phase-1 batch", () => {
  it("emits a single batch wrapping the frame + rules + binding", () => {
    const { batch } = lowerToMutations(fixture(), placement, binding);
    expect(batch.op).toBe("batch");
    const ops = (batch as { args: { ops: unknown[] } }).args.ops as Array<{
      op: string;
    }>;
    // insertTextFrame, 1 h-rule, 1 v-rule, setPluginMetadata = 4 ops.
    expect(ops.map((o) => o.op)).toEqual([
      "insertTextFrame",
      "insertLine",
      "insertLine",
      "setPluginMetadata",
    ]);
  });

  it("the frame is placed at the requested page-local bounds", () => {
    const { batch } = lowerToMutations(fixture(), placement, binding);
    const ops = (batch as { args: { ops: Array<{ op: string; args: unknown }> } })
      .args.ops;
    const frame = ops[0] as {
      args: { pageId: string; bounds: [number, number, number, number] };
    };
    expect(frame.args.pageId).toBe("Page/u1");
    expect(frame.args.bounds).toEqual([24, 30, 64, 150]);
  });

  it("rule offsets are page-local: content-space offset + frame origin", () => {
    const { batch } = lowerToMutations(fixture(), placement, binding);
    const ops = (batch as { args: { ops: Array<{ op: string; args: unknown }> } })
      .args.ops;
    // bounds = [top=24, left=30, ...]; the h-rule at y=20, x 0..120.
    const h = ops[1] as {
      args: { start: [number, number]; end: [number, number] };
    };
    expect(h.args.start).toEqual([30 + 0, 24 + 20]); // [left+from, top+at]
    expect(h.args.end).toEqual([30 + 120, 24 + 20]); // [left+to, top+at]
    // the v-rule at x=60, y 0..40.
    const v = ops[2] as {
      args: { start: [number, number]; end: [number, number] };
    };
    expect(v.args.start).toEqual([30 + 60, 24 + 0]); // [left+at, top+from]
    expect(v.args.end).toEqual([30 + 60, 24 + 40]); // [left+at, top+to]
  });

  it("binding is written onto the $created sentinel with this plugin's key", () => {
    const { batch } = lowerToMutations(fixture(), placement, binding);
    const ops = (batch as { args: { ops: Array<{ op: string; args: unknown }> } })
      .args.ops;
    const meta = ops[3] as {
      op: string;
      args: {
        elementId: { kind: string; id: string };
        key: string;
        value: string;
      };
    };
    expect(meta.op).toBe("setPluginMetadata");
    expect(meta.args.elementId).toEqual({ kind: "textFrame", id: "$created" });
    expect(meta.args.key).toBe("x-paged:media.paged.sheet");
    // The value is the JSON-stringified binding envelope, round-trippable.
    expect(JSON.parse(meta.args.value)).toEqual({
      v: 1,
      data: { sheet: "Sheet1", range: "A1:B2", contentVersion: 7 },
    });
  });

  it("setPluginMetadata is LAST so $created resolves the freshly-minted frame", () => {
    const { batch } = lowerToMutations(fixture(), placement, binding);
    const ops = (batch as { args: { ops: Array<{ op: string }> } }).args.ops;
    expect(ops[0].op).toBe("insertTextFrame");
    expect(ops[ops.length - 1].op).toBe("setPluginMetadata");
  });
});

describe("sheet_plugin_lower_mutations: phase-2 text join", () => {
  it("joins cells tab-within-row, newline-between-rows", () => {
    const { text } = lowerToMutations(fixture(), placement, binding);
    expect(text).toBe("A1\t100\nA2\t200");
  });

  it("empty cells become empty tab fields so columns stay aligned", () => {
    const content: LoweredContent = {
      cols: [
        { index: 0, widthPt: 40 },
        { index: 1, widthPt: 40 },
        { index: 2, widthPt: 40 },
      ],
      rows: [
        {
          index: 0,
          heightPt: 16,
          // col 1 is absent (empty middle column).
          cells: [
            { col: 0, text: "x", align: "left" },
            { col: 2, text: "z", align: "left" },
          ],
        },
        {
          index: 1,
          heightPt: 16,
          cells: [], // a fully empty row.
        },
      ],
      rules: { h: [], v: [] },
      merges: [],
    };
    expect(joinText(content)).toBe("x\t\tz\n\t\t");
  });

  it("a single empty region lowers to an empty string", () => {
    const empty: LoweredContent = {
      cols: [],
      rows: [],
      rules: { h: [], v: [] },
      merges: [],
    };
    expect(joinText(empty)).toBe("");
  });

  it("no rules → batch is just frame + binding", () => {
    const content = fixture();
    content.rules = { h: [], v: [] };
    const { batch } = lowerToMutations(content, placement, binding);
    const ops = (batch as { args: { ops: Array<{ op: string }> } }).args.ops;
    expect(ops.map((o) => o.op)).toEqual([
      "insertTextFrame",
      "setPluginMetadata",
    ]);
  });
});
