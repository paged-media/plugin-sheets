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
  ExporterContribution,
  ImporterContribution,
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
      f === "contribute.importer@1" || f === "contribute.exporter@1",
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
    openedPanels,
    disposedCount: () => disposed,
  };
}

describe("sheet_plugin_bundle_activate", () => {
  it("registers the workbook + grid panels under their declared ids", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    expect(fake.panels.map((p) => p.id)).toEqual([
      "media.paged.sheet.panel.workbook",
      "media.paged.sheet.panel.grid",
    ]);
    expect(fake.panels[0].title).toBe("Workbook");
    expect(fake.panels[0].defaultDock).toBe("right");
    expect(fake.panels[1].title).toBe("Grid");
    expect(fake.panels[1].defaultDock).toBe("right");
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
    ]);
  });

  it("openGrid command opens the grid panel", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    const openCmd = fake.commands.find((c) => c.id.endsWith("openGrid"));
    openCmd?.handler(undefined);
    expect(fake.openedPanels).toEqual(["media.paged.sheet.panel.grid"]);
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

  it("dispose tears the session down (no throw — honesty smoke test)", () => {
    const fake = fakeHost();
    const handle = sheetBundle.activate(fake.host);
    expect(() => handle.dispose()).not.toThrow();
  });
});
