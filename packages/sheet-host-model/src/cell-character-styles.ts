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

// Cell TEXT formatting on a native table (Wave 4). The table lane poured
// bare text: a bold header, a 9 pt footnote row, a red negative came out in
// the document default — while the column widths were MEASURED in the
// styled face, so the widths described text nobody rendered.
//
// A table cell's text has no direct property door (the `storyRange` element
// id cannot address a cell), but `applyStyle` takes a cell qualifier. So
// each distinct cell formatting becomes a document CHARACTER STYLE at a
// deterministic, content-addressed id (the facets `styleProps` already
// derives: face, family, size, text colour), minted once, and applied to
// the cell's text. Same mint discipline as the swatches: read what the
// document carries, mint only the absent ids, mint NOTHING on a failed read
// (a duplicate create fails the whole batch).
//
// PURE: content in, mutations out.

import type { Mutation } from "@paged-media/plugin-api";

import type { LoweredContent, LoweredStyle } from "./lowered";
import { columnOrder } from "./lower-to-table";
import { styleProps, type StyleProp } from "./lower-to-mutations";
import { textOffsetLength } from "./table-refresh";

/** InDesign's "no character style" — what an unformatted cell resets to. */
export const NO_CHARACTER_STYLE = "CharacterStyle/$ID/[No character style]";

/** FNV-1a, 32-bit, hex — a short stable content address. */
function fnv(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h.toString(16).padStart(8, "0");
}

/** The deterministic character-style id for a set of facets. */
export function cellCharacterStyleId(props: readonly StyleProp[]): string {
  return `CharacterStyle/paged.sheet ${fnv(JSON.stringify(props))}`;
}

/** A readable name for the style (what the Character Styles panel lists). */
function styleName(style: LoweredStyle): string {
  const parts = [
    style.fontName ?? null,
    style.bold && style.italic ? "Bold Italic" : style.bold ? "Bold" : style.italic ? "Italic" : null,
    style.fontSizePt != null ? `${style.fontSizePt} pt` : null,
    style.textRgb ?? null,
  ].filter((p): p is string => p !== null);
  return `Sheet · ${parts.join(" ") || "text"}`;
}

/** The character style (id + facets + display name) per style key; keys
 *  whose style carries no character facet map to no entry. */
export function cellCharacterStyles(
  content: LoweredContent,
): Map<number, { id: string; name: string; props: StyleProp[] }> {
  const out = new Map<number, { id: string; name: string; props: StyleProp[] }>();
  for (const style of content.styles ?? []) {
    if (style.key === 0) continue;
    const props = styleProps(style);
    if (props.length === 0) continue;
    out.set(style.key, { id: cellCharacterStyleId(props), name: styleName(style), props });
  }
  return out;
}

/** createCharacterStyle + one setStyleProperty per facet for every style
 *  the content uses and the document lacks. `knownStyleIds === null` (the
 *  read failed) mints nothing. Text colours name `cellText` swatches — mint
 *  those first (`cellTextSwatchOps`). */
export function cellCharacterStyleMints(
  content: LoweredContent,
  knownStyleIds: ReadonlySet<string> | null,
): Mutation[] {
  if (knownStyleIds === null) return [];
  const used = new Set<number>();
  for (const row of content.rows) {
    for (const cell of row.cells) {
      if (cell.text.length > 0 && cell.styleKey) used.add(cell.styleKey);
    }
  }
  const ops: Mutation[] = [];
  const minted = new Set<string>();
  for (const [key, st] of cellCharacterStyles(content)) {
    if (!used.has(key) || knownStyleIds.has(st.id) || minted.has(st.id)) continue;
    minted.add(st.id);
    ops.push({ op: "createCharacterStyle", args: { selfId: st.id, name: st.name } });
    for (const p of st.props) {
      ops.push({
        op: "setStyleProperty",
        args: { collection: "character", styleId: st.id, path: p.path, value: p.value },
      });
    }
  }
  return ops;
}

/** The character style a cell's text should carry (`NO_CHARACTER_STYLE`
 *  for an unformatted cell). Keyed `row:col` at TABLE positions. */
export function cellCharacterStyleMap(content: LoweredContent): Map<string, { style: string; text: string }> {
  const styles = cellCharacterStyles(content);
  const colPos = new Map<number, number>();
  columnOrder(content).forEach((m, i) => colPos.set(m, i));
  const out = new Map<string, { style: string; text: string }>();
  content.rows.forEach((row, r) => {
    for (const cell of row.cells) {
      const c = colPos.get(cell.col);
      if (c === undefined || cell.text.length === 0) continue;
      const st = cell.styleKey ? styles.get(cell.styleKey) : undefined;
      out.set(`${r}:${c}`, { style: st?.id ?? NO_CHARACTER_STYLE, text: cell.text });
    }
  });
  return out;
}

/** applyStyle (cell-qualified, whole cell text) for each populated cell —
 *  all of them, or only `cells` (`row:col` keys). Unformatted cells are
 *  skipped unless `includeDefault` (a refresh resets a cell that LOST its
 *  formatting; a fresh pour has nothing to reset). */
export function cellCharacterStyleApplies(
  content: LoweredContent,
  storyId: string,
  tableId: string,
  opts: { cells?: ReadonlySet<string>; includeDefault?: boolean } = {},
): Mutation[] {
  const ops: Mutation[] = [];
  for (const [key, { style, text }] of cellCharacterStyleMap(content)) {
    if (opts.cells && !opts.cells.has(key)) continue;
    if (style === NO_CHARACTER_STYLE && !opts.includeDefault) continue;
    const [row, col] = key.split(":").map(Number);
    ops.push({
      op: "applyStyle",
      args: {
        storyId,
        start: 0,
        end: textOffsetLength(text),
        style,
        scope: "character",
        cell: { tableId, row, col },
      },
    });
  }
  return ops;
}

/** The cells a refresh must re-style: the character style changed, or the
 *  text was re-poured into a cell where either the old or the new text is
 *  formatted (re-poured text may inherit the old run's format). A table
 *  with no cell formatting on either side needs none. */
export function cellsNeedingRestyle(prev: LoweredContent, next: LoweredContent): Set<string> {
  const a = cellCharacterStyleMap(prev);
  const b = cellCharacterStyleMap(next);
  const out = new Set<string>();
  for (const [k, v] of b) {
    const old = a.get(k);
    const oldStyle = old?.style ?? NO_CHARACTER_STYLE;
    if (oldStyle !== v.style) out.add(k);
    else if (old?.text !== v.text && v.style !== NO_CHARACTER_STYLE) out.add(k);
  }
  return out;
}
