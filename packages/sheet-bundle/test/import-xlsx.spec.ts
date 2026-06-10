// sheet.plugin.import.pick (S-11) — pickAndImport drives the HOST file
// picker (host.shell.pickFile) and feeds the chosen bytes to the session;
// it falls back to opening the panel when no picker is wired. Pure host +
// session mocks (no engine, no wasm).

import { describe, expect, it, vi } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";

import { pickAndImport, XLSX_MIME } from "../src/import-xlsx";
import type { WorkbookSession } from "../src/session";

const PANEL = "media.paged.sheet.panel.workbook";

function fakeSession() {
  const imports: Array<{ bytes: Uint8Array; name: string }> = [];
  const session = {
    import: vi.fn(async (bytes: Uint8Array, name: string) => {
      imports.push({ bytes, name });
    }),
  } as unknown as WorkbookSession;
  return { session, imports };
}

function fakeHost(opts: {
  pickFile?: (o?: {
    accept?: readonly string[];
    multiple?: boolean;
  }) => Promise<readonly { name: string; bytes: Uint8Array; mimeType: string }[]>;
}) {
  const opened: string[] = [];
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) => f === "shell.pickFile@1" && !!opts.pickFile,
    shell: {
      openPanel: (id: string) => opened.push(id),
      closePanel() {},
      pickFile: opts.pickFile ?? (async () => []),
    },
  } as unknown as BundleHost;
  return { host, opened };
}

describe("sheet_plugin_import_pick", () => {
  it("picks an .xlsx and imports its bytes into the session", async () => {
    const bytes = new Uint8Array([1, 2, 3]);
    const pickFile = vi.fn(async (_o?: { accept?: readonly string[] }) => [
      { name: "budget.xlsx", bytes, mimeType: XLSX_MIME },
    ]);
    const { host, opened } = fakeHost({ pickFile });
    const { session, imports } = fakeSession();

    await pickAndImport(host, session, PANEL);

    // The accept filter offered .xlsx (extension + MIME).
    expect(pickFile).toHaveBeenCalledOnce();
    const arg = pickFile.mock.calls[0][0];
    expect(arg?.accept).toContain(".xlsx");
    // The chosen bytes reached the session, then the panel was shown.
    expect(imports).toEqual([{ bytes, name: "budget.xlsx" }]);
    expect(opened).toEqual([PANEL]);
  });

  it("does nothing on cancel (empty pick)", async () => {
    const pickFile = vi.fn(async () => []);
    const { host, opened } = fakeHost({ pickFile });
    const { session, imports } = fakeSession();

    await pickAndImport(host, session, PANEL);

    expect(imports).toEqual([]);
    expect(opened).toEqual([]);
  });

  it("falls back to opening the panel when no host picker is wired", async () => {
    const { host, opened } = fakeHost({}); // supports('shell.pickFile@1') = false
    const { session, imports } = fakeSession();

    await pickAndImport(host, session, PANEL);

    expect(imports).toEqual([]);
    expect(opened).toEqual([PANEL]);
  });
});
