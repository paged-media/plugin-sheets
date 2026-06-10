// The XLSX import entry points. With the host file picker LIVE (S-11),
// `pickAndImport` opens `host.shell.pickFile({ accept: [".xlsx"] })` and
// feeds the chosen bytes to the session; when no picker is wired it falls
// back to opening the workbook panel (the panel's `<input type="file">` is
// the no-picker path). The actual parse runs in Rust (sheet-xlsx) via the
// engine facade — the TS side only moves bytes.

import type { BundleHost } from "@paged-media/plugin-api";

import type { WorkbookSession } from "./session";

/** The OOXML spreadsheet MIME type — used for the picker `accept` and the
 *  importer/exporter contributions. */
export const XLSX_MIME =
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/** Open the workbook panel (the "show me the workbook" affordance + the
 *  no-picker fallback target). */
export function importXlsx(host: BundleHost, panelId: string): void {
  host.shell.openPanel(panelId);
}

/** Pick an `.xlsx` via the host file picker (S-11) and import it into the
 *  session. Falls back to opening the workbook panel when no picker is
 *  wired (the panel owns the `<input type="file">` no-picker path). */
export async function pickAndImport(
  host: BundleHost,
  session: WorkbookSession,
  panelId: string,
): Promise<void> {
  if (!host.supports("shell.pickFile@1")) {
    host.shell.openPanel(panelId);
    return;
  }
  const files = await host.shell.pickFile({
    accept: [".xlsx", XLSX_MIME],
    multiple: false,
  });
  const file = files[0];
  if (!file) return; // cancelled
  await session.import(file.bytes, file.name);
  host.shell.openPanel(panelId);
}
