// The importXlsx command handler. There is no host file-picker surface
// (S-11), so the command just opens the workbook panel — the panel owns
// the `<input type="file" accept=".xlsx">` (the React expert-leaf escape
// hatch; the panel owns its own DOM, like plugin-web's source panel).
// The actual parse runs in Rust (sheet-xlsx) via the engine facade.

import type { BundleHost } from "@paged-media/plugin-api";

/** Open the workbook panel so the user can pick an `.xlsx` (S-11: the
 *  panel owns the file input). */
export function importXlsx(host: BundleHost, panelId: string): void {
  host.shell.openPanel(panelId);
}
