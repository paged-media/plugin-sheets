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
    contribute: {
      panel(c: PanelContribution): Disposable {
        panels.push(c);
        return track();
      },
      command(c: CommandContribution): Disposable {
        commands.push(c);
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

  it("registers the three commands under their declared ids", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    expect(fake.commands.map((c) => c.id)).toEqual([
      "media.paged.sheet.command.importXlsx",
      "media.paged.sheet.command.lowerToFrame",
      "media.paged.sheet.command.openGrid",
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

  it("importXlsx command opens the workbook panel (S-11: panel owns the file input)", () => {
    const fake = fakeHost();
    sheetBundle.activate(fake.host);
    const importCmd = fake.commands.find((c) =>
      c.id.endsWith("importXlsx"),
    );
    importCmd?.handler(undefined);
    expect(fake.openedPanels).toEqual(["media.paged.sheet.panel.workbook"]);
  });

  it("dispose tears the session down (no throw — honesty smoke test)", () => {
    const fake = fakeHost();
    const handle = sheetBundle.activate(fake.host);
    expect(() => handle.dispose()).not.toThrow();
  });
});
