// The two-phase page lower (S-03; spec §8.2). The engine lowers the
// range to the IR (Rust), the host-model translator turns it into the
// phase-1 batch + the phase-2 text (pure), and THIS module drives the
// host writes — the only place in the bundle that calls
// host.document.mutate.
//
// WHY TWO PHASES (S-03). The wire has no `insertTable`, and `insertText`
// keys off a `storyId` that only EXISTS after the frame is created. So
// the page surface degrades to the spec §2.2 fallback (tab-aligned text
// + drawn rules) and splits in two:
//
//   Phase 1 — mutate(batch): insertTextFrame + insertLine per rule +
//     setPluginMetadata(binding) as ONE undoable step. The outcome
//     carries `createdId` (the new frame's ElementId).
//   Phase 2 — resolve the frame's storyId, then mutate(insertText) the
//     joined cell text at offset 0.
//
// RESOLVING THE STORY (the read door). plugin-api exposes no direct
// frame→story lookup (SceneTreeNode/collections don't carry it); the
// available door that DOES is `host.document.hitTest`, whose HitResult
// carries `storyId`. So we hit-test the new frame's centre to recover
// its story. This mirrors how a created element is re-resolved through a
// read door (plugin-web re-reads its created frame via getMetadata; here
// the needed datum is the story, and hitTest is the door that yields it).

import type { BundleHost, ElementId, PageId } from "@paged-media/plugin-api";
import {
  defaultPlacement,
  lowerToMutations,
  makeBinding,
} from "@paged-media/sheet-host-model";

import type { SheetEngine } from "./engine";

/** The frame center, page-local pt, from `[top, left, bottom, right]`. */
function center(
  bounds: [number, number, number, number],
): [number, number] {
  const [top, left, bottom, right] = bounds;
  return [(left + right) / 2, (top + bottom) / 2];
}

/** The active page id (meta first, else the first page). Mirrors
 *  plugin-web's `activePageId`. */
async function activePageId(host: BundleHost): Promise<PageId | null> {
  const meta = await host.document.meta();
  if (meta.activePage) return meta.activePage;
  const pages = await host.document.collection<{ selfId: string }>("pages");
  return pages.length > 0 ? pages[0].selfId : null;
}

/** Raw frame id from a created ElementId (the hitTest filter / text ops
 *  key off the string id). */
function frameIdOf(id: ElementId): string | null {
  if (id.kind === "textFrame" || id.kind === "rectangle") {
    return id.id as string;
  }
  return null;
}

/**
 * Lower `sheet`/`range` to a fresh page frame. Engine computes the IR;
 * the translator (pure) shapes the mutations; this drives the two-phase
 * host writes. Returns the created frame's raw id, or null on any failure
 * (mutate-never-throws: outcomes are checked, not caught).
 */
export async function lowerSelectionToFrame(
  host: BundleHost,
  engine: SheetEngine,
  sheet: number,
  range: string,
): Promise<string | null> {
  const pageId = await activePageId(host);
  if (!pageId) {
    host.log.warn("lower: no page to place the sheet frame into");
    return null;
  }

  // Engine-computed IR (all spreadsheet semantics in Rust).
  const content = engine.getRangeLowered(sheet, range, {
    includeGridRules: true,
  });
  const sheetInfo = engine.listSheets().find((s) => s.id === sheet);
  const sheetName = sheetInfo ? sheetInfo.name : String(sheet);

  const placement = defaultPlacement(pageId, content);
  // contentVersion 0: T0 has no workbook revision counter (the engine
  // gains one when save-back lands); the binding still round-trips.
  const binding = makeBinding(sheetName, range, 0);
  const { batch, text } = lowerToMutations(content, placement, binding);

  // Phase 1 — frame + rules + binding, one undoable batch.
  const outcome = await host.document.mutate(batch);
  if (!outcome.applied || !outcome.createdId) {
    host.log.warn("lower: phase-1 batch rejected", outcome);
    return null;
  }
  const frameId = frameIdOf(outcome.createdId);
  if (!frameId) {
    host.log.warn("lower: created element is not a frame target");
    return null;
  }

  // Phase 2 — resolve the new frame's story via the hitTest read door,
  // then pour the text. Empty regions skip the pour (nothing to insert).
  if (text.length > 0) {
    const hit = await host.document.hitTest(pageId, center(placement.bounds));
    const storyId = hit?.storyId ?? null;
    if (!storyId) {
      host.log.warn(
        "lower: could not resolve the created frame's story; frame placed " +
          "without text (S-03 phase-2 read-door gap)",
      );
      return frameId;
    }
    const pour = await host.document.mutate({
      op: "insertText",
      args: { storyId, offset: 0, text },
    });
    if (!pour.applied) host.log.warn("lower: phase-2 insertText rejected", pour);
  }

  await host.selection.set([outcome.createdId]);
  return frameId;
}
