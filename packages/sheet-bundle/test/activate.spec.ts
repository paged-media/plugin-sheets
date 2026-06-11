// sheet.plugin.bundle.activate — registration wiring against a minimal
// hand-rolled fake BundleHost (no editor, no engine): the bundle
// contributes ONE panel + the two commands, and dispose tears the
// session down cleanly (the honesty smoke test). Engine behavior is NOT
// exercised here — this is wiring only.

import { describe, expect, it } from "vitest";

import type {
  BundleHost,
  CommandContribution,
  Disposable,
  EditContextContribution,
  ExporterContribution,
  ImporterContribution,
  ObjectTypeContribution,
  PanelContribution,
} from "@paged-media/plugin-api";

import { sheetBundle } from "../src";

/** A minimal recording fake of the slice of BundleHost activate touches:
 *  contribute.panel / contribute.command, log, and shell.openPanel. Each
 *  registration returns a Disposable the fake tracks so we can assert
 *  teardown. */
function fakeHost() {
  const panels: PanelContribution[] = [];
  const commands: CommandContribution[] = [];
  const importers: ImporterContribution[] = [];
  const exporters: ExporterContribution[] = [];
  const objectTypes: ObjectTypeContribution[] = [];
  const editContexts: EditContextContribution[] = [];
  let disposed = 0;
  const track = (): Disposable => ({
    dispose() {
      disposed += 1;
    },
  });
  const openedPanels: string[] = [];
  const host = {
    manifest: sheetBundle.manifest,
    log: { debug() {}, info() {}, warn() {}, error() {} },
    // The IO contribution doors are wired; the host file picker is NOT,
    // so importXlsx exercises the panel-fallback path (S-11 fallback).
    supports: (f: string) =>
      f === "contribute.importer@1" ||
      f === "contribute.exporter@1" ||
      f === "contribute.objectType@1" ||
      f === "contribute.editContext@1",
    contribute: {
      panel(c: PanelContribution): Disposable {
        panels.push(c);
        return track();
      },
      command(c: CommandContribution): Disposable {
        commands.push(c);
        return track();
      },
      importer(c: ImporterContribution): Disposable {
        importers.push(c);
        return track();
      },
      exporter(c: ExporterContribution): Disposable {
        exporters.push(c);
        return track();
      },
      objectType(c: ObjectTypeContribution): Disposable {
        objectTypes.push(c);
        return track();
      },
      editContext(c: EditContextContribution): Disposable {
        editContexts.push(c);
        return track();
      },
    },
    shell: {
      openPanel(id: string) {
        openedPanels.push(id);
      },
      closePanel() {},
    },
  } as unknown as BundleHost;
  return {
    host,
    panels,
    commands,
    importers,
    exporters,
    objectTypes,
    editContexts,
    openedPanels,
    disposedCount: () => disposed,
  };
}

describe("sheet_plugin_bundle_activate", () => {
  it("registers the workbook + grid + datasets panels under their declared ids", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    expect(fake.panels.map((p) => p.id)).toEqual([
      "media.paged.sheet.panel.workbook",
      "media.paged.sheet.panel.grid",
      "media.paged.sheet.panel.datasets",
    ]);
    expect(fake.panels[0].title).toBe("Workbook");
    expect(fake.panels[0].defaultDock).toBe("right");
    expect(fake.panels[1].title).toBe("Grid");
    expect(fake.panels[1].defaultDock).toBe("right");
    expect(fake.panels[2].title).toBe("Datasets");
    expect(fake.panels[2].defaultDock).toBe("right");
  });

  it("registers the commands under their declared ids", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    expect(fake.commands.map((c) => c.id)).toEqual([
      "media.paged.sheet.command.importXlsx",
      "media.paged.sheet.command.lowerToFrame",
      "media.paged.sheet.command.lowerChartToFrame",
      "media.paged.sheet.command.openGrid",
      "media.paged.sheet.command.showGridInFrame",
      "media.paged.sheet.command.hideGridInFrame",
      "media.paged.sheet.command.sheetFromDataset",
    ]);
  });

  it("openGrid command opens the grid panel", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    const openCmd = fake.commands.find((c) => c.id.endsWith("openGrid"));
    openCmd?.handler(undefined);
    expect(fake.openedPanels).toEqual(["media.paged.sheet.panel.grid"]);
  });

  it("sheetFromDataset command opens the datasets panel (S-15)", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    const cmd = fake.commands.find((c) => c.id.endsWith("sheetFromDataset"));
    cmd?.handler(undefined);
    expect(fake.openedPanels).toEqual(["media.paged.sheet.panel.datasets"]);
  });

  it("registered ids match the manifest's contributes declaration", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    expect(fake.panels.map((p) => p.id)).toEqual(
      sheetBundle.manifest.contributes?.panels,
    );
    expect(fake.commands.map((c) => c.id)).toEqual(
      sheetBundle.manifest.contributes?.commands,
    );
  });

  it("importXlsx command falls back to opening the panel when no host picker (S-11 fallback)", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    const importCmd = fake.commands.find((c) =>
      c.id.endsWith("importXlsx"),
    );
    // The fake reports no shell.pickFile@1, so pickAndImport opens the
    // workbook panel (the panel's <input> is the no-picker path).
    importCmd?.handler(undefined);
    expect(fake.openedPanels).toEqual(["media.paged.sheet.panel.workbook"]);
  });

  it("registers the .xlsx importer + exporter (K-2 / S-06)", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    expect(fake.importers.map((i) => i.id)).toEqual([
      "media.paged.sheet.importer.xlsx",
    ]);
    expect(fake.importers[0].extensions).toEqual([".xlsx"]);
    expect(fake.exporters.map((e) => e.id)).toEqual([
      "media.paged.sheet.exporter.xlsx",
    ]);
    expect(fake.exporters[0].extension).toBe(".xlsx");
    // The declared ids match the manifest's contributes block.
    expect(fake.importers.map((i) => i.id)).toEqual(
      sheetBundle.manifest.contributes?.importers,
    );
    expect(fake.exporters.map((e) => e.id)).toEqual(
      sheetBundle.manifest.contributes?.exporters,
    );
  });

  it("registers the sheetFrame objectType + sheet editContext (K-1 entry)", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    // The objectType marks a lowered sheet frame (by its own binding
    // metadata) and routes its double-click to the "sheet" context.
    expect(fake.objectTypes.map((o) => o.type)).toEqual(["sheetFrame"]);
    const ot = fake.objectTypes[0];
    expect(ot.bakedFallback).toBe("rectangle");
    expect(ot.editContextType).toBe("sheet");
    // A candidate WITHOUT this plugin's binding metadata is not a match.
    // (parseBinding validates the envelope shape; null → no claim.)
    expect(
      ot.matches({
        id: { kind: "rectangle", id: "r1" },
        groupChain: [],
        metadata: null,
      }),
    ).toBe(false);
    // The editContext enters on double-click and shows/hides the grid.
    expect(fake.editContexts.map((e) => e.type)).toEqual(["sheet"]);
    expect(fake.editContexts[0].entry).toBe("doubleClick");
    // The declared types match the manifest's contributes block.
    expect(fake.objectTypes.map((o) => o.type)).toEqual(
      sheetBundle.manifest.contributes?.objectTypes?.map((o) => o.type),
    );
    expect(fake.editContexts.map((e) => e.type)).toEqual(
      sheetBundle.manifest.contributes?.editContexts?.map((e) => e.type),
    );
  });

  it("dispose tears the session down (no throw — honesty smoke test)", () => {
    const fake = fakeHost();
    const handle = sheetBundle.activate(fake.host);
    expect(() => handle.dispose()).not.toThrow();
  });
});
