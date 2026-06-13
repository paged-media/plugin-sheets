// sheet.plugin.lower.mutations — the tab-text FALLBACK translator (the
// retained spec §2.2 degradation; the native-table lane is
// lower-to-table.spec.ts):
// rule-offset math, op ordering, the $created sentinel, and the
// tab/newline join (incl. empty cells). Pure data in, mutations out.

import { describe, expect, it } from "vitest";

import {
  joinText,
  lowerToMutations,
  makeBinding,
  styleEmissions,
  styleProps,
  type Binding,
  type LoweredContent,
  type LoweredStyle,
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

// sheet.lower.condfmt.databar — data bars lower to the page-draw geometry
// lane: one createSwatch per distinct bar colour, then one insertPath (closed
// rect ring) + frameFillColor ref per bar, emitted BEFORE the binding so
// setPluginMetadata stays last (the single-undo property).
describe("sheet_lower_condfmt_databar: drawn-rect lowering", () => {
  function withBars(): LoweredContent {
    const c = fixture();
    c.databars = [
      // Two bars sharing the document-default blue → ONE swatch reused.
      { row: 0, col: 1, x: 60, y: 0, w: 30, h: 18, fillFraction: 0.5, fill: "#638EC6" },
      { row: 1, col: 1, x: 60, y: 20, w: 60, h: 18, fillFraction: 1.0, fill: "#638EC6" },
      // A zero-width bar (value at domain min) is skipped — no path emitted.
      { row: 0, col: 0, x: 0, y: 0, w: 0, h: 18, fillFraction: 0.0, fill: "#638EC6" },
    ];
    return c;
  }

  it("emits one swatch + per-bar insertPath, with the binding still LAST", () => {
    const { batch } = lowerToMutations(withBars(), placement, binding);
    const ops = (batch as { args: { ops: Array<{ op: string }> } }).args.ops;
    // frame, h-rule, v-rule, createSwatch (one, deduped), then 2 (insertPath +
    // frameFillColor) for the two non-zero bars = 4, then setPluginMetadata.
    expect(ops.map((o) => o.op)).toEqual([
      "insertTextFrame",
      "insertLine",
      "insertLine",
      "createSwatch",
      "insertPath",
      "setElementProperty",
      "insertPath",
      "setElementProperty",
      "setPluginMetadata",
    ]);
    expect(ops[ops.length - 1].op).toBe("setPluginMetadata");
  });

  it("bar rects are page-local closed rings; the fill is a swatch colorRef", () => {
    const { batch } = lowerToMutations(withBars(), placement, binding);
    const ops = (batch as { args: { ops: Array<{ op: string; args: unknown }> } })
      .args.ops;
    const path = ops.find((o) => o.op === "insertPath") as {
      args: { anchors: Array<{ anchor: [number, number] }>; open: boolean };
    };
    expect(path.args.open).toBe(false);
    // bounds = [top=24, left=30]; first bar at content (60,0) size 30x18.
    expect(path.args.anchors.map((a) => a.anchor)).toEqual([
      [30 + 60, 24 + 0],
      [30 + 90, 24 + 0],
      [30 + 90, 24 + 18],
      [30 + 60, 24 + 18],
    ]);
    const fill = ops.find((o) => o.op === "setElementProperty") as {
      args: { path: string; value: { type: string; value: string } };
    };
    expect(fill.args.path).toBe("frameFillColor");
    expect(fill.args.value).toEqual({
      type: "colorRef",
      value: "Color/uPagedSheetDataBar638EC6",
    });
  });

  it("no databars → batch is unchanged (frame + rules + binding)", () => {
    const { batch } = lowerToMutations(fixture(), placement, binding);
    const ops = (batch as { args: { ops: Array<{ op: string }> } }).args.ops;
    expect(ops.map((o) => o.op)).toEqual([
      "insertTextFrame",
      "insertLine",
      "insertLine",
      "setPluginMetadata",
    ]);
  });
});

// sheet.style.applystyle-pour — the IR-v2 style table (spec §8.3) lowers
// into host character-level property overrides; fill/border facets this
// fallback's text-frame degradation cannot place are REPORTED (not faked) —
// the native lane places them via tableCell-scoped properties.
describe("sheet_style_applystyle_pour: style emission", () => {
  const defaultStyle: LoweredStyle = {
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
  };
  const mk = (over: Partial<LoweredStyle>): LoweredStyle => ({
    ...defaultStyle,
    ...over,
  });

  it("bold collapses to one characterFontStyle face token", () => {
    expect(styleProps(mk({ key: 1, bold: true }))).toEqual([
      { path: "characterFontStyle", value: { type: "text", value: "Bold" } },
    ]);
    expect(styleProps(mk({ key: 1, bold: true, italic: true }))).toEqual([
      {
        path: "characterFontStyle",
        value: { type: "text", value: "Bold Italic" },
      },
    ]);
    expect(styleProps(mk({ key: 1, italic: true }))).toEqual([
      { path: "characterFontStyle", value: { type: "text", value: "Italic" } },
    ]);
  });

  it("font size/name and TEXT colour map to character-level props", () => {
    const props = styleProps(
      mk({ key: 1, fontSizePt: 18, fontName: "Cambria", textRgb: "#FF0000" }),
    );
    expect(props).toEqual([
      { path: "characterFontFamily", value: { type: "text", value: "Cambria" } },
      { path: "characterFontSize", value: { type: "length", value: 18 } },
      {
        path: "characterFillColor",
        value: { type: "colorRef", value: "#FF0000" },
      },
    ]);
  });

  it("the default style emits no props", () => {
    expect(styleProps(defaultStyle)).toEqual([]);
  });

  it("styleEmissions skips key 0 and visually-default keys", () => {
    const content: LoweredContent = {
      cols: [],
      rows: [],
      rules: { h: [], v: [] },
      merges: [],
      styles: [
        defaultStyle, // key 0 — never emitted
        mk({ key: 1, bold: true }),
        mk({ key: 2, fillRgb: "#FFFF00" }),
      ],
    };
    const ems = styleEmissions(content);
    expect(ems.map((e) => e.styleKey)).toEqual([1, 2]);
    // key 1: a character override, nothing blocked.
    expect(ems[0].props).toEqual([
      { path: "characterFontStyle", value: { type: "text", value: "Bold" } },
    ]);
    expect(ems[0].blocked).toEqual([]);
  });

  it("fill background + borders are REPORTED as blocked (fallback lane), not faked", () => {
    const content: LoweredContent = {
      cols: [],
      rows: [],
      rules: { h: [], v: [] },
      merges: [],
      styles: [
        defaultStyle,
        mk({ key: 1, fillRgb: "#FFFF00", borderTop: true, borderBottom: true }),
      ],
    };
    const [emission] = styleEmissions(content);
    expect(emission.styleKey).toBe(1);
    // No character-level props (fill/border are cell facets), and BOTH are
    // reported blocked — the honest fallback-lane boundary (no real table cell).
    expect(emission.props).toEqual([]);
    expect(emission.blocked).toEqual(["fillBackground", "border"]);
  });

  it("a cell with text colour AND a blocked fill emits the char prop and the block", () => {
    const content: LoweredContent = {
      cols: [],
      rows: [],
      rules: { h: [], v: [] },
      merges: [],
      styles: [defaultStyle, mk({ key: 1, textRgb: "#0000FF", fillRgb: "#FFFF00" })],
    };
    const [emission] = styleEmissions(content);
    expect(emission.props).toEqual([
      {
        path: "characterFillColor",
        value: { type: "colorRef", value: "#0000FF" },
      },
    ]);
    expect(emission.blocked).toEqual(["fillBackground"]);
  });

  it("lowerToMutations surfaces the prepared style emissions", () => {
    const content = fixture();
    content.styles = [defaultStyle, mk({ key: 1, bold: true })];
    // Point one cell at the styled key so the table is meaningful.
    content.rows[0].cells[0].styleKey = 1;
    const { styles, batch } = lowerToMutations(content, placement, binding);
    expect(styles).toHaveLength(1);
    expect(styles[0].styleKey).toBe(1);
    // The phase-1 batch is UNCHANGED — styles ride alongside, the honest
    // tab-text degradation (frame + rules + binding) does not gain a fake
    // style op.
    const ops = (batch as { args: { ops: Array<{ op: string }> } }).args.ops;
    expect(ops.every((o) => o.op !== "setStyleProperty")).toBe(true);
  });

  it("an unstyled region surfaces an empty styles list", () => {
    const content = fixture(); // no `styles` field
    expect(lowerToMutations(content, placement, binding).styles).toEqual([]);
  });
});
