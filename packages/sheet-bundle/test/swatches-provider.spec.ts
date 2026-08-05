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

// ADR 023 phase D — the paged.sheet SWATCHES binding provider: the
// DOCUMENT-SCOPED lane, which is the axis the Layers proof does not
// touch. Wiring + refusal semantics against a hand-rolled fake host; no
// editor, no engine.

import { describe, expect, it, vi } from "vitest";

import type { BundleHost, Mutation, SwatchSummary } from "@paged-media/plugin-api";
import { paletteEntry } from "../../sheet-host-model/src";

import { sheetBundle } from "../src";
import {
  makeSwatchesBindingProvider,
  SERVED_OPS,
} from "../src/binding-provider/swatches-provider";
import { supportsBindingProviders } from "../src/binding-provider/adr023-seam";

const CHART_BLUE = paletteEntry("chart", "4E79A7");
const BAR_GREY = paletteEntry("dataBar", "638EC6");

/** A fake host exposing just `document.mutate` / `document.collection`
 *  and `log`, recording every mutation the provider forwards. */
function fakeHost(documentSwatches: SwatchSummary[] = []) {
  const mutations: Mutation[] = [];
  const warnings: string[] = [];
  let collectionThrows = false;
  const host = {
    log: {
      debug() {},
      info() {},
      warn(msg: string) {
        warnings.push(msg);
      },
      error() {},
    },
    document: {
      async mutate(m: Mutation) {
        mutations.push(m);
        return { applied: true as const, createdId: null, pageIds: [] };
      },
      async collection<T>(): Promise<readonly T[]> {
        if (collectionThrows) throw new Error("no document");
        return documentSwatches as unknown as readonly T[];
      },
    },
  } as unknown as BundleHost;
  return {
    host,
    mutations,
    warnings,
    breakCollection() {
      collectionThrows = true;
    },
  };
}

describe("sheet_plugin_swatch_provider: what it declares", () => {
  it("declares the swatches COLLECTION and the editSwatch OP — and no paths", () => {
    const fake = fakeHost();
    const { provider } = makeSwatchesBindingProvider(fake.host, {
      workbookPalette: () => [CHART_BLUE],
    });
    expect(provider.provides.collections).toEqual(["swatches"]);
    expect(provider.provides.ops).toEqual(["editSwatch"]);
    // NO paths, and that is the scope axis in one assertion: core models
    // no swatch `PropertyPath` (there is no `swatchName`, no swatch
    // colour), so a swatch provider has nothing honest to declare in the
    // property lane and implements none of it.
    expect(provider.provides.paths).toBeUndefined();
    expect(
      (provider as { readProperty?: unknown }).readProperty,
    ).toBeUndefined();
  });

  it("leaves createSwatch / deleteSwatch UNDECLARED, so the host disables them", () => {
    // Undeclared is the contract's own way to suppress a control. A
    // workbook palette gains a colour by USE, not by declaration, and a
    // chart series colour cannot be deleted from a colour panel.
    expect(SERVED_OPS).not.toContain("createSwatch");
    expect(SERVED_OPS).not.toContain("deleteSwatch");
    expect(SERVED_OPS).not.toContain("importSwatchLibrary");
    expect(SERVED_OPS).not.toContain("editColorGroup");
  });
});

describe("sheet_plugin_swatch_provider: the document-scoped read", () => {
  it("serves the workbook palette in CORE's swatch row shape", async () => {
    const fake = fakeHost();
    const { provider } = makeSwatchesBindingProvider(fake.host, {
      workbookPalette: () => [CHART_BLUE, BAR_GREY],
    });
    const answer = await provider.readCollection!({ collection: "swatches" });
    expect(answer.kind).toBe("rows");
    expect((answer as unknown as { rows: SwatchSummary[] }).rows).toEqual([
      {
        selfId: "Color/uPagedSheetChart4E79A7",
        name: "paged.sheet chart 4E79A7",
        kind: "process",
      },
      {
        selfId: "Color/uPagedSheetDataBar638EC6",
        name: "paged.sheet data bar 638EC6",
        kind: "process",
      },
    ]);
  });

  it("takes NO target — the lane is document-scoped by construction", async () => {
    // The request shape carries only a collection name. There is no
    // selection to address, which is exactly what makes this consumer a
    // different shape from Layers and from Character/Paragraph.
    const fake = fakeHost();
    const { provider } = makeSwatchesBindingProvider(fake.host, {
      workbookPalette: () => [CHART_BLUE],
    });
    const answer = await provider.readCollection!({ collection: "swatches" });
    expect(answer.kind).toBe("rows");
  });

  it("DECLINES a collection it does not own", async () => {
    const fake = fakeHost();
    const { provider } = makeSwatchesBindingProvider(fake.host, {
      workbookPalette: () => [CHART_BLUE],
    });
    const answer = await provider.readCollection!({ collection: "layers" });
    expect(answer.kind).toBe("decline");
  });

  it("DECLINES (never empty rows) when the workbook mints no swatches", async () => {
    // The planarRegions lesson: a refusal that looks like a result is a
    // bug generator. Here it decides whether the user sees the
    // DOCUMENT's swatches or an empty colour panel.
    const fake = fakeHost();
    const { provider } = makeSwatchesBindingProvider(fake.host, {
      workbookPalette: () => [],
    });
    const answer = await provider.readCollection!({ collection: "swatches" });
    expect(answer.kind).toBe("decline");
  });

  it("DECLINES rather than throwing when the palette read fails", async () => {
    const fake = fakeHost();
    const { provider } = makeSwatchesBindingProvider(fake.host, {
      workbookPalette: () => {
        throw new Error("engine gone");
      },
    });
    const answer = await provider.readCollection!({ collection: "swatches" });
    expect(answer.kind).toBe("decline");
    expect(fake.warnings.join(" ")).toContain("palette read failed");
  });
});

describe("sheet_plugin_swatch_provider: the structural write", () => {
  async function primed(
    documentSwatches: SwatchSummary[] = [],
    palette = [CHART_BLUE],
  ) {
    const fake = fakeHost(documentSwatches);
    const { provider } = makeSwatchesBindingProvider(fake.host, {
      workbookPalette: () => palette,
    });
    // The host reads before it writes; the provider only claims rows it
    // handed out.
    await provider.readCollection!({ collection: "swatches" });
    return { fake, provider };
  }

  it("MINTS ON FIRST EDIT: an unminted palette colour becomes createSwatch", async () => {
    const { fake, provider } = await primed([]);
    const spec = {
      selfId: CHART_BLUE.selfId,
      name: "Brand blue",
      space: "RGB",
      value: [1, 2, 3],
      model: "Process",
    };
    const answer = await provider.applyMutation!({
      op: "editSwatch",
      args: { swatchId: CHART_BLUE.selfId, spec },
    } as Mutation);

    expect(answer.kind).toBe("applied");
    expect(fake.mutations).toHaveLength(1);
    // core's `createSwatch` honours `spec.selfId` AND refuses a
    // duplicate, so the existence branch is load-bearing.
    expect(fake.mutations[0].op).toBe("createSwatch");
    expect(
      (fake.mutations[0].args as { spec: { selfId: string; name: string } })
        .spec,
    ).toMatchObject({ selfId: CHART_BLUE.selfId, name: "Brand blue" });
  });

  it("EDITS a palette colour the document already carries", async () => {
    const { fake, provider } = await primed([
      { selfId: CHART_BLUE.selfId, name: "old", kind: "process" },
    ]);
    const answer = await provider.applyMutation!({
      op: "editSwatch",
      args: {
        swatchId: CHART_BLUE.selfId,
        spec: { selfId: CHART_BLUE.selfId, name: "Brand blue" },
      },
    } as Mutation);

    expect(answer.kind).toBe("applied");
    expect(fake.mutations).toHaveLength(1);
    expect(fake.mutations[0].op).toBe("editSwatch");
  });

  it("writes through host.document.mutate — the contract's undo rule", async () => {
    const { fake, provider } = await primed([]);
    const spy = vi.spyOn(fake.host.document, "mutate");
    await provider.applyMutation!({
      op: "editSwatch",
      args: {
        swatchId: CHART_BLUE.selfId,
        spec: { selfId: CHART_BLUE.selfId, name: "x" },
      },
    } as Mutation);
    expect(spy).toHaveBeenCalledTimes(1);
  });

  it("DECLINES a swatch id that is not in the workbook palette", async () => {
    const { fake, provider } = await primed([]);
    const answer = await provider.applyMutation!({
      op: "editSwatch",
      args: { swatchId: "Color/Black", spec: { selfId: "Color/Black" } },
    } as Mutation);
    // A decline, not a refusal: core may well own this swatch, and the
    // host must be free to send it there.
    expect(answer.kind).toBe("decline");
    expect(fake.mutations).toHaveLength(0);
  });

  it("DECLINES rows it never handed out (it claims only what it served)", async () => {
    const fake = fakeHost();
    const { provider } = makeSwatchesBindingProvider(fake.host, {
      workbookPalette: () => [CHART_BLUE],
    });
    // No readCollection first ⇒ no rows claimed ⇒ core answers.
    const answer = await provider.applyMutation!({
      op: "editSwatch",
      args: {
        swatchId: CHART_BLUE.selfId,
        spec: { selfId: CHART_BLUE.selfId },
      },
    } as Mutation);
    expect(answer.kind).toBe("decline");
  });

  it("REFUSES (applied:false), not declines, when it owned the op but could not honour it", async () => {
    // The distinction the contract insists on: a decline would send the
    // same op to core behind the provider's back. `applied:false` names
    // the failure and stops there.
    const { fake, provider } = await primed([]);
    fake.breakCollection();
    const answer = await provider.applyMutation!({
      op: "editSwatch",
      args: {
        swatchId: CHART_BLUE.selfId,
        spec: { selfId: CHART_BLUE.selfId },
      },
    } as Mutation);
    expect(answer.kind).toBe("applied");
    expect(
      (answer as unknown as { outcome: { applied: boolean } }).outcome.applied,
    ).toBe(false);
    expect(fake.mutations).toHaveLength(0);
  });

  it("DECLINES an op it does not serve (the belt to the registry's braces)", async () => {
    const { provider } = await primed([]);
    const answer = await provider.applyMutation!({
      op: "deleteSwatch",
      args: { swatchId: CHART_BLUE.selfId },
    } as Mutation);
    expect(answer.kind).toBe("decline");
  });
});

describe("sheet_plugin_swatch_provider: the honest door", () => {
  it("reports NO binding-provider support when the host lacks the door", () => {
    const host = {
      contribute: {},
      supports: () => false,
      log: { debug() {}, info() {}, warn() {}, error() {} },
    } as unknown as BundleHost;
    expect(supportsBindingProviders(host)).toBe(false);
  });

  it("reports NO support when the door exists but no registry is wired", () => {
    const host = {
      contribute: { bindingProvider: () => ({ dispose() {}, invalidate() {} }) },
      supports: (f: string) => f !== "bindings.provider@1",
      log: { debug() {}, info() {}, warn() {}, error() {} },
    } as unknown as BundleHost;
    expect(supportsBindingProviders(host)).toBe(false);
  });

  it("activate() registers the provider for the `sheet` context when both hold", () => {
    const registered: { contextType: string }[] = [];
    let disposed = 0;
    const host = {
      manifest: sheetBundle.manifest,
      log: { debug() {}, info() {}, warn() {}, error() {} },
      supports: (f: string) =>
        f === "contribute.objectType@1" ||
        f === "contribute.editContext@1" ||
        f === "bindings.provider@1",
      contribute: {
        panel: () => ({ dispose() {} }),
        command: () => ({ dispose() {} }),
        objectType: () => ({ dispose() {} }),
        editContext: () => ({ dispose() {} }),
        bindingProvider: (contextType: string) => {
          registered.push({ contextType });
          return {
            dispose() {
              disposed += 1;
            },
            invalidate() {},
          };
        },
      },
      shell: { openPanel() {}, closePanel() {} },
    } as unknown as BundleHost;

    const handle = sheetBundle.activate(host);
    expect(registered.map((r) => r.contextType)).toEqual(["sheet"]);
    // The handle is allocated outside a facade-tracked registration, so
    // dispose has to tear it down explicitly.
    handle.dispose();
    expect(disposed).toBe(1);
  });

  it("activate() registers NO provider on a host without the registry", () => {
    const registered: string[] = [];
    const host = {
      manifest: sheetBundle.manifest,
      log: { debug() {}, info() {}, warn() {}, error() {} },
      supports: (f: string) =>
        f === "contribute.objectType@1" || f === "contribute.editContext@1",
      contribute: {
        panel: () => ({ dispose() {} }),
        command: () => ({ dispose() {} }),
        objectType: () => ({ dispose() {} }),
        editContext: () => ({ dispose() {} }),
        bindingProvider: (contextType: string) => {
          registered.push(contextType);
          return { dispose() {}, invalidate() {} };
        },
      },
      shell: { openPanel() {}, closePanel() {} },
    } as unknown as BundleHost;

    sheetBundle.activate(host).dispose();
    // The degradation is honest: on a pre-ADR host the shared Swatches
    // panel keeps reading core, and nothing throws.
    expect(registered).toEqual([]);
  });
});
