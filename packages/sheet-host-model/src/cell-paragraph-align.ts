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

// Horizontal cell alignment on a native table (Wave 9). The engine resolves
// every cell's alignment in Rust (an explicit style wins; otherwise Excel's
// General rule — numbers right, booleans/errors centred, text left) and the
// lowered cell carries it — but the table lane poured bare text, so every
// number in a placed table sat flush LEFT.
//
// A table cell's paragraph has no direct property door (`paragraphJustification`
// addresses a story range, which cannot reach into a cell), but `applyStyle`
// takes a cell qualifier with `scope: "paragraph"`. So the two non-default
// alignments become two document PARAGRAPH STYLES at fixed ids (justification
// only), minted once with the swatch/character-style discipline (read first,
// mint only the absent ids, mint NOTHING on a failed read), and applied to the
// cell's paragraph. Left / General need nothing: they are the default.
//
// PURE: content in, mutations out.

import type { Mutation } from "@paged-media/plugin-api";

import type { Align, LoweredContent } from "./lowered";
import { columnOrder } from "./lower-to-table";
import { textOffsetLength } from "./table-refresh";

/** InDesign's "no paragraph style" — what a left-aligned cell resets to. */
export const NO_PARAGRAPH_STYLE = "ParagraphStyle/$ID/[No paragraph style]";

/** The paragraph style (id, name, IDML justification) per non-default
 *  alignment. */
export const ALIGN_PARAGRAPH_STYLES: Readonly<
  Partial<Record<Align, { id: string; name: string; justification: string }>>
> = {
  center: {
    id: "ParagraphStyle/paged.sheet align center",
    name: "Sheet · align center",
    justification: "CenterAlign",
  },
  right: {
    id: "ParagraphStyle/paged.sheet align right",
    name: "Sheet · align right",
    justification: "RightAlign",
  },
};

/** The paragraph style each populated cell's paragraph should carry, keyed
 *  `row:col` at TABLE positions (`NO_PARAGRAPH_STYLE` for left/general). */
export function cellAlignMap(content: LoweredContent): Map<string, { style: string; text: string }> {
  const colPos = new Map<number, number>();
  columnOrder(content).forEach((m, i) => colPos.set(m, i));
  const out = new Map<string, { style: string; text: string }>();
  content.rows.forEach((row, r) => {
    for (const cell of row.cells) {
      const c = colPos.get(cell.col);
      if (c === undefined || cell.text.length === 0) continue;
      out.set(`${r}:${c}`, {
        style: ALIGN_PARAGRAPH_STYLES[cell.align]?.id ?? NO_PARAGRAPH_STYLE,
        text: cell.text,
      });
    }
  });
  return out;
}

/** Whether any populated cell needs a non-default alignment (a caller skips
 *  the paragraph-style read when none does). */
export function needsAlignStyles(content: LoweredContent): boolean {
  for (const { style } of cellAlignMap(content).values()) {
    if (style !== NO_PARAGRAPH_STYLE) return true;
  }
  return false;
}

/** createParagraphStyle + its justification for every alignment style the
 *  content uses and the document lacks. `knownStyleIds === null` (the read
 *  failed) mints nothing — the cells then stay left, the batch still lands. */
export function cellAlignStyleMints(
  content: LoweredContent,
  knownStyleIds: ReadonlySet<string> | null,
): Mutation[] {
  if (knownStyleIds === null) return [];
  const used = new Set([...cellAlignMap(content).values()].map((v) => v.style));
  const ops: Mutation[] = [];
  for (const st of Object.values(ALIGN_PARAGRAPH_STYLES)) {
    if (!st || !used.has(st.id) || knownStyleIds.has(st.id)) continue;
    ops.push({ op: "createParagraphStyle", args: { selfId: st.id, name: st.name } });
    ops.push({
      op: "setStyleProperty",
      args: {
        collection: "paragraph",
        styleId: st.id,
        path: "paragraphJustification",
        value: { type: "text", value: st.justification },
      },
    });
  }
  return ops;
}

/** applyStyle (cell-qualified, paragraph scope) for each populated cell —
 *  all of them, or only `cells`. Left/general cells are skipped unless
 *  `includeDefault` (a refresh resets a cell that LOST its alignment).
 *  `known` (the document's paragraph styles; null = unread/failed) drops
 *  applies naming a style that neither exists nor is minted alongside. */
export function cellAlignApplies(
  content: LoweredContent,
  storyId: string,
  tableId: string,
  opts: { cells?: ReadonlySet<string>; includeDefault?: boolean; available?: ReadonlySet<string> } = {},
): Mutation[] {
  const ops: Mutation[] = [];
  for (const [key, { style, text }] of cellAlignMap(content)) {
    if (opts.cells && !opts.cells.has(key)) continue;
    if (style === NO_PARAGRAPH_STYLE && !opts.includeDefault) continue;
    if (style !== NO_PARAGRAPH_STYLE && opts.available && !opts.available.has(style)) continue;
    const [row, col] = key.split(":").map(Number);
    ops.push({
      op: "applyStyle",
      args: {
        storyId,
        start: 0,
        end: textOffsetLength(text),
        style,
        scope: "paragraph",
        cell: { tableId, row, col },
      },
    });
  }
  return ops;
}

/** The cells a refresh must re-align: the alignment changed, or the text was
 *  re-poured into a cell whose new alignment is not the default. */
export function cellsNeedingRealign(prev: LoweredContent, next: LoweredContent): Set<string> {
  const a = cellAlignMap(prev);
  const out = new Set<string>();
  for (const [k, v] of cellAlignMap(next)) {
    const old = a.get(k);
    if ((old?.style ?? NO_PARAGRAPH_STYLE) !== v.style) out.add(k);
    else if (old?.text !== v.text && v.style !== NO_PARAGRAPH_STYLE) out.add(k);
  }
  return out;
}
