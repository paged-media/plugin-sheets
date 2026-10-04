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

// When the host's panels re-read paged.sheet's binding providers (ADR 023).
//
// `invalidate()` makes the host re-read the provider, and a re-read costs
// engine work (the text provider lowers the selected range). It used to
// fire on EVERY session change signal — a selection change, a keystroke
// commit anywhere in the workbook, a 1000-cell paste far off-screen. The
// session now says what changed (`SessionChange`), and each provider is
// invalidated only by what it reads:
//
//   text provider (Character / Paragraph) — the selected cells' base
//     styles: invalidated when the selection it reads MOVES, or a value
//     edit lands INSIDE it (an empty cell that gets content joins the
//     styled set), or on anything unclassified.
//   swatches provider — the workbook palette (charts + data bars, none of
//     which a value edit or a selection move changes): invalidated only
//     on unclassified changes.
//
// "Unclassified" (`other`) is the safe default: imports, sheet ops, style
// writes, structural edits — anything may have changed, so both re-read.

import type { Disposable } from "@paged-media/plugin-api";

import type { CellRegion, SessionChange } from "../session";

/** A provider handle — only `invalidate` matters here. */
interface Invalidatable {
  invalidate(): void;
}

/** The session surface this needs. */
export interface InvalidationSource {
  onDidChange(listener: (change: SessionChange) => void): Disposable;
  textSelectionRange(): { sheet: number; range: string } | null;
}

/** Inclusive 0-based bounds of an A1 range (`B2`, `A1:C10`, `$A$1:$B$2`),
 *  or null when it does not parse (the caller then assumes an overlap). */
export function a1Bounds(
  range: string,
): { firstRow: number; firstCol: number; lastRow: number; lastCol: number } | null {
  const ref = /^\$?([A-Za-z]{1,3})\$?(\d+)$/;
  const parts = range.split(":");
  if (parts.length < 1 || parts.length > 2) return null;
  const cells: [number, number][] = [];
  for (const p of parts) {
    const m = ref.exec(p.trim());
    if (!m) return null;
    let col = 0;
    for (const ch of m[1].toUpperCase()) col = col * 26 + (ch.charCodeAt(0) - 64);
    cells.push([Number(m[2]) - 1, col - 1]);
  }
  const [a, b] = [cells[0], cells[cells.length - 1]];
  return {
    firstRow: Math.min(a[0], b[0]),
    firstCol: Math.min(a[1], b[1]),
    lastRow: Math.max(a[0], b[0]),
    lastCol: Math.max(a[1], b[1]),
  };
}

/** Does any of `regions` overlap the selection `sel`? */
export function touchesSelection(
  sel: { sheet: number; range: string } | null,
  regions: readonly CellRegion[],
): boolean {
  if (!sel) return false;
  const b = a1Bounds(sel.range);
  return regions.some(
    (r) =>
      r.sheet === sel.sheet &&
      (b === null ||
        (r.firstRow <= b.lastRow &&
          r.lastRow >= b.firstRow &&
          r.firstCol <= b.lastCol &&
          r.lastCol >= b.firstCol)),
  );
}

/** Wire provider invalidation to the session's change signal. */
export function subscribeProviderInvalidation(
  session: InvalidationSource,
  providers: { swatches: Invalidatable | null; text: Invalidatable | null },
): Disposable {
  const keyOf = (sel: { sheet: number; range: string } | null) =>
    sel ? `${sel.sheet}!${sel.range}` : "";
  let lastKey = keyOf(session.textSelectionRange());
  return session.onDidChange((change) => {
    const sel = session.textSelectionRange();
    const key = keyOf(sel);
    const moved = key !== lastKey;
    lastKey = key;
    if (change.kind === "other") {
      providers.swatches?.invalidate();
      providers.text?.invalidate();
      return;
    }
    if (moved || (change.kind === "cells" && touchesSelection(sel, change.regions))) {
      providers.text?.invalidate();
    }
  });
}
