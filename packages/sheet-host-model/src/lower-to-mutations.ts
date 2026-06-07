// THE load-bearing translator (spec §8.2; degradation S-03): the engine
// has ALREADY computed the lowered IR (column/row geometry, formatted
// cell text, grid rules, merges); this turns that pure data into host
// Mutations. ZERO spreadsheet semantics live here (CLAUDE.md hard rule)
// — it is arithmetic over already-decided geometry plus the host
// mutation vocabulary.
//
// TWO-PHASE (S-03). The wire has no `insertTable` op, and `insertText`
// keys off a `storyId` that exists only AFTER the frame applies. So the
// page lower degrades to the spec §2.2 fallback — tab-aligned text in a
// text frame + drawn rules — split across two phases:
//
//   Phase 1 (this function's `batch`): insertTextFrame + an insertLine
//     per grid rule + setPluginMetadata writing the binding onto the
//     batch-created frame (the protocol-v34 `$created` sentinel). ONE
//     undoable step; the host returns the created frame's id.
//   Phase 2 (the caller, lower.ts): resolve the frame's storyId from the
//     created id, then `insertText` the tab/newline-joined `text` this
//     function also returns.
//
// We return BOTH the batch and the text so the caller never re-derives
// the join. Geometry is content-space in the IR (offsets from the
// region's own top-left, spec §8.5); we add the frame's page-local
// bounds origin so rules land on the page.

import type { ElementId, Mutation, PageId } from "@paged-media/plugin-api";

import { BINDING_KEY, type Binding } from "./binding";
import type { Bounds } from "./placement";
import type { LoweredContent } from "./lowered";

/** The page + bounds the frame is placed at (from placement.ts). */
export interface LowerPlacement {
  pageId: PageId;
  bounds: Bounds;
}

/** What the translator yields: the phase-1 mutation batch and the
 *  phase-2 text to pour once the caller resolves the new frame's story. */
export interface LowerResult {
  /** One undoable `batch`: frame + rules + binding metadata. */
  batch: Mutation;
  /** The cells joined tab-within-row, newline-between-rows — the
   *  phase-2 `insertText` payload. */
  text: string;
}

/** The protocol-v34 batch-created sentinel: an `insertTextFrame` mints a
 *  textFrame, and a later op in the SAME batch addresses it by this
 *  placeholder id. The host resolves `$created` to the just-minted frame
 *  and the metadata gate verifies the key is this plugin's own namespace
 *  (the plugin-web insert.ts precedent). */
const CREATED_FRAME: ElementId = { kind: "textFrame", id: "$created" };

/** Build the tab/newline join of the lowered cells. Rows in IR order;
 *  within a row, cells are placed at their `col` so empty columns become
 *  empty tab fields (tab-aligned text is the S-03 degradation — the
 *  columns line up under monospace/tab stops). A trailing newline is
 *  NOT added (rows are SEPARATED by newlines, not terminated). */
export function joinText(content: LoweredContent): string {
  // The column order the text follows: the lowered column indices,
  // ascending. Empty leading/interior columns become empty fields so
  // every row aligns to the same tab grid.
  const colOrder = content.cols.map((c) => c.index);
  const lines = content.rows.map((row) => {
    const byCol = new Map<number, string>();
    for (const cell of row.cells) byCol.set(cell.col, cell.text);
    return colOrder.map((ci) => byCol.get(ci) ?? "").join("\t");
  });
  return lines.join("\n");
}

/** Translate lowered IR + a resolved placement + the frame binding into
 *  the phase-1 batch and the phase-2 text. Pure: no host import beyond
 *  wire TYPES. */
export function lowerToMutations(
  content: LoweredContent,
  placement: LowerPlacement,
  binding: Binding,
): LowerResult {
  const { pageId, bounds } = placement;
  const [top, left] = bounds;

  const ops: Mutation[] = [];

  // (1) The frame itself.
  ops.push({ op: "insertTextFrame", args: { pageId, bounds } });

  // (2) One drawn line per grid rule, in page-local coords. The IR
  // carries content-space offsets (relative to the region's top-left);
  // add the frame's [top, left] origin. An h-rule runs horizontally at
  // y = top + at, from x = left + from to x = left + to; a v-rule runs
  // vertically at x = left + at, from y = top + from to y = top + to.
  for (const rule of content.rules.h) {
    ops.push({
      op: "insertLine",
      args: {
        pageId,
        start: [left + rule.from, top + rule.at],
        end: [left + rule.to, top + rule.at],
      },
    });
  }
  for (const rule of content.rules.v) {
    ops.push({
      op: "insertLine",
      args: {
        pageId,
        start: [left + rule.at, top + rule.from],
        end: [left + rule.at, top + rule.to],
      },
    });
  }

  // (3) The binding, written onto the batch-created frame via the
  // `$created` sentinel. ONE undo removes the frame, its rules, AND the
  // binding (the plugin-web single-undo property). The value is the
  // JSON-stringified envelope, exactly as setPluginMetadata expects.
  ops.push({
    op: "setPluginMetadata",
    args: {
      elementId: CREATED_FRAME,
      key: BINDING_KEY,
      value: JSON.stringify(binding),
    },
  });

  return {
    batch: { op: "batch", args: { ops } },
    text: joinText(content),
  };
}
