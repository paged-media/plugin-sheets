// sheet.plugin.lower.mutations (the bundle-side flow): the two-phase
// page lower (S-03) drives the host writes in order — phase 1 the batch
// (frame + rules + binding), phase 2 the insertText into the story the
// host's hitTest resolves. A fake host captures the mutate calls; a fake
// engine returns a small LoweredContent.

import { describe, expect, it } from "vitest";

import type {
  BundleHost,
  ElementId,
  Mutation,
  MutationOutcome,
} from "@paged-media/plugin-api";

import { lowerSelectionToFrame, type SheetEngine } from "../src";

// A fake engine: returns a fixed 2x1 region + one sheet.
function fakeEngine(): SheetEngine {
  return {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: () => ({ changed: [] }),
    getCellDisplay: () => "",
    getRangeLowered: () => ({
      cols: [
        { index: 0, widthPt: 50 },
        { index: 1, widthPt: 50 },
      ],
      rows: [
        {
          index: 0,
          heightPt: 18,
          cells: [
            { col: 0, text: "Item", align: "left" },
            { col: 1, text: "Qty", align: "right" },
          ],
        },
      ],
      rules: { h: [{ at: 18, from: 0, to: 100 }], v: [] },
      merges: [],
    }),
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 1, cols: 2 }],
    dispose() {},
  };
}

// A fake host capturing every mutate; createdId is a textFrame; hitTest
// returns a known storyId so phase 2 can address the story.
function fakeHost(createdId: ElementId, storyId: string | null) {
  const mutations: Mutation[] = [];
  const selections: ElementId[][] = [];
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    document: {
      async meta() {
        return { activePage: "Page/u1" } as never;
      },
      async collection() {
        return [] as never;
      },
      async mutate(m: Mutation): Promise<MutationOutcome> {
        mutations.push(m);
        // Only the first (batch) mints an element.
        if (m.op === "batch") {
          return { applied: true, createdId, pageIds: ["Page/u1"] };
        }
        return { applied: true, createdId: null, pageIds: ["Page/u1"] };
      },
      async hitTest() {
        return storyId
          ? ({ storyId, frameId: "frame1" } as never)
          : null;
      },
    },
    selection: {
      async set(ids: ElementId[]) {
        selections.push(ids);
        return ids;
      },
    },
  } as unknown as BundleHost;
  return { host, mutations, selections };
}

const CREATED: ElementId = { kind: "textFrame", id: "frame1" };

describe("sheet_plugin_lower_mutations: two-phase host flow", () => {
  it("phase 1 batch first, then phase 2 insertText into the resolved story", async () => {
    const { host, mutations, selections } = fakeHost(CREATED, "Story/u9");
    const id = await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1");

    expect(id).toBe("frame1");
    expect(mutations).toHaveLength(2);
    // Phase 1 — the batch.
    expect(mutations[0].op).toBe("batch");
    const ops = (mutations[0] as { args: { ops: Array<{ op: string }> } }).args
      .ops;
    expect(ops[0].op).toBe("insertTextFrame");
    expect(ops.some((o) => o.op === "insertLine")).toBe(true);
    expect(ops[ops.length - 1].op).toBe("setPluginMetadata");
    // Phase 2 — insertText into the hitTest-resolved story at offset 0.
    expect(mutations[1].op).toBe("insertText");
    const text = mutations[1] as {
      args: { storyId: string; offset: number; text: string };
    };
    expect(text.args.storyId).toBe("Story/u9");
    expect(text.args.offset).toBe(0);
    expect(text.args.text).toBe("Item\tQty");
    // The new frame is selected.
    expect(selections).toEqual([[CREATED]]);
  });

  it("phase 1 carries the binding for this plugin's namespace", async () => {
    const { host, mutations } = fakeHost(CREATED, "Story/u9");
    await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1");
    const ops = (mutations[0] as { args: { ops: Array<{ op: string; args: unknown }> } })
      .args.ops;
    const meta = ops.find((o) => o.op === "setPluginMetadata") as {
      args: { key: string; value: string };
    };
    expect(meta.args.key).toBe("x-paged:media.paged.sheet");
    const binding = JSON.parse(meta.args.value);
    expect(binding.data.sheet).toBe("Sheet1");
    expect(binding.data.range).toBe("A1:B1");
  });

  it("skips phase 2 (frame still placed) when the story can't be resolved", async () => {
    const { host, mutations } = fakeHost(CREATED, null);
    const id = await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1");
    expect(id).toBe("frame1"); // frame placed, honest about the gap
    expect(mutations.map((m) => m.op)).toEqual(["batch"]); // no insertText
  });

  it("returns null when the phase-1 batch is rejected", async () => {
    const mutations: Mutation[] = [];
    const host = {
      log: { debug() {}, info() {}, warn() {}, error() {} },
      document: {
        async meta() {
          return { activePage: "Page/u1" } as never;
        },
        async collection() {
          return [] as never;
        },
        async mutate(m: Mutation): Promise<MutationOutcome> {
          mutations.push(m);
          return { applied: false, error: "nope" };
        },
        async hitTest() {
          return null;
        },
      },
      selection: { async set(ids: ElementId[]) { return ids; } },
    } as unknown as BundleHost;
    const id = await lowerSelectionToFrame(host, fakeEngine(), 0, "A1:B1");
    expect(id).toBeNull();
    expect(mutations.map((m) => m.op)).toEqual(["batch"]);
  });
});
