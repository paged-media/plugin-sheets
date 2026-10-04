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

/** Delimited-text spreadsheets (Wave 4): extension → MIME. */
export const DELIMITED_MIMES = ["text/csv", "text/tab-separated-values"];

/** Is `name` a CSV/TSV file (by extension)? */
export function isDelimited(name: string): boolean {
  return /\.(csv|tsv|tab)$/i.test(name);
}

/** Import picked/dropped bytes by kind: CSV/TSV text (decoded as UTF-8,
 *  typed in Rust) or an XLSX package. */
export async function importBytes(
  session: WorkbookSession,
  bytes: Uint8Array,
  name: string,
): Promise<void> {
  if (isDelimited(name)) {
    await session.importCsv(new TextDecoder("utf-8").decode(bytes), name);
  } else {
    await session.import(bytes, name);
  }
}

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
    accept: [".xlsx", XLSX_MIME, ".csv", ".tsv", ...DELIMITED_MIMES],
    multiple: false,
  });
  const file = files[0];
  if (!file) return; // cancelled
  await importBytes(session, file.bytes, file.name);
  host.shell.openPanel(panelId);
}
