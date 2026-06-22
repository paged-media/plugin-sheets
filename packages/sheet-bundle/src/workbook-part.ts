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

// `.paged` CONTAINER persistence for the imported workbook (file-format.md §4/§8).
//
// The XLSX workbook bytes are the sheet frame's SOURCE. They rode `host.blob`
// (OPFS, S-08) — per-BROWSER storage that does NOT travel with the document
// (you re-import after opening on another machine). They now ALSO persist as a
// `.paged` container part (`paged/media.paged.sheet/workbook.xlsx`) via
// `host.parts`: uncapped, binary-friendly, and it travels WITH the file. The
// blob stays as a per-browser cache + backward-compat read source; the part is
// the portable home + the read PREFERENCE.
//
// Singleton, mirroring S-08: the LAST imported workbook is the one persisted
// (per-plugin, not per-frame). Forward/backward-safe: a host with no container
// writer (`supports("storage.parts@1")` is false — an older editor) is a clean
// no-op, and a document with no part falls back to the blob, so existing
// documents keep working unchanged.

import type { BundleHost } from "@paged-media/plugin-api";

type PartsHost = Pick<BundleHost, "parts" | "supports">;

/** The workbook bytes + its display name, each a part relative to this
 *  plugin's `paged/media.paged.sheet/` namespace (the host prepends it). */
const WORKBOOK_PART = "workbook.xlsx";
const WORKBOOK_NAME_PART = "workbook.name";

/** The name a workbook restores under when its name part is missing. */
const DEFAULT_NAME = "workbook.xlsx";

/** Write the workbook + its display name to the container parts (the portable,
 *  uncapped home). Best-effort: no container writer ⇒ no-op (the blob remains
 *  the source). */
export async function writeWorkbookPart(
  host: PartsHost,
  bytes: Uint8Array,
  name: string,
): Promise<void> {
  if (!host.supports("storage.parts@1")) return;
  await host.parts.write(WORKBOOK_PART, bytes);
  await host.parts.write(WORKBOOK_NAME_PART, new TextEncoder().encode(name));
}

/** Read the workbook + name from the container parts, or `null` when absent /
 *  no container writer — the caller then falls back to the per-browser blob. */
export async function readWorkbookPart(
  host: PartsHost,
): Promise<{ bytes: Uint8Array; name: string } | null> {
  if (!host.supports("storage.parts@1")) return null;
  const bytes = await host.parts.read(WORKBOOK_PART);
  if (!bytes) return null;
  const nameBytes = await host.parts.read(WORKBOOK_NAME_PART);
  const name = nameBytes ? new TextDecoder().decode(nameBytes) : DEFAULT_NAME;
  return { bytes, name };
}
