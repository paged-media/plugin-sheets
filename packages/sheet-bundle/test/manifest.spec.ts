// sheet.plugin.manifest.valid — the manifest contract the bundle ships:
// the namespace line (id + contributed ids prefixed by it), the T0
// capability posture, and the declared wasm artifact within the 8 MiB
// budget. The plugin-cli `validate:manifest` is the authoritative gate
// (CI runs it); this asserts the same invariants in-process so the row
// carries a real test lane.

import { describe, expect, it } from "vitest";

import manifest from "../manifest.json";

const ID = "media.paged.sheet";
const WASM_BUDGET = 8 * 1024 * 1024;

describe("sheet_plugin_manifest_valid", () => {
  it("declares the plugin id + apiVersion the bundle targets", () => {
    expect(manifest.id).toBe(ID);
    expect(manifest.apiVersion).toBe("^0.2");
  });

  it("every contributed id is namespaced under the plugin id", () => {
    const contributed = [
      ...(manifest.contributes.panels ?? []),
      ...(manifest.contributes.commands ?? []),
    ];
    expect(contributed.length).toBeGreaterThan(0);
    for (const cid of contributed) {
      expect(cid.startsWith(`${ID}.`)).toBe(true);
    }
  });

  it("the contributed ids match what activate() registers", () => {
    expect(manifest.contributes.panels).toEqual([
      "media.paged.sheet.panel.workbook",
      "media.paged.sheet.panel.grid",
      "media.paged.sheet.panel.datasets",
    ]);
    expect(manifest.contributes.commands).toEqual([
      "media.paged.sheet.command.importXlsx",
      "media.paged.sheet.command.lowerToFrame",
      "media.paged.sheet.command.lowerChartToFrame",
      "media.paged.sheet.command.openGrid",
      "media.paged.sheet.command.showGridInFrame",
      "media.paged.sheet.command.hideGridInFrame",
      "media.paged.sheet.command.sortRange",
      "media.paged.sheet.command.findReplace",
      "media.paged.sheet.command.sheetFromDataset",
      "media.paged.sheet.command.copySelection",
      "media.paged.sheet.command.pasteSelection",
    ]);
  });

  it("T0 capability posture: read broad, write scoped, no network", () => {
    expect(manifest.capabilities.document.read).toBe("broad");
    expect(manifest.capabilities.document.write).toBe("scoped");
    expect(manifest.capabilities.network).toBe(false);
  });

  it("declares the clipboard FULL grant (K-6 / S-14) — text + tabular for range copy/paste", () => {
    // "full" authorizes BOTH halves of host.clipboard (the cell-grid
    // interchange); "vector"/"none" would deny the tabular grid.
    expect(manifest.capabilities.clipboard).toBe("full");
  });

  it("declares the data-provider CONSUMER capability (S-15) — consume, not publish", () => {
    // The consumer gate: the host refuses discover/get unless the manifest
    // declares consume ∋ "dataset". paged.sheet never publishes.
    expect(manifest.capabilities.dataProviders).toEqual({
      consume: ["dataset"],
    });
  });

  it("declares the sheet-engine wasm artifact within the 8 MiB budget", () => {
    const wasm = manifest.capabilities.wasm;
    expect(wasm).toHaveLength(1);
    expect(wasm[0].name).toBe("sheet-engine");
    expect(wasm[0].path).toBe("bin/sheet_js_bg.wasm");
    expect(wasm[0].purpose).toBe("compute");
    expect(wasm[0].maxBytes).toBe(WASM_BUDGET);
  });
});
