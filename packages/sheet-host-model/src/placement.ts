// Default frame placement — where a freshly lowered sheet frame lands on
// the page. PURE geometry over the lowered IR's total size: no host
// import, no engine call. The translator (lower-to-mutations.ts) takes
// the bounds this returns as the frame's page-local box and adds it as
// the origin for the content-space rule offsets (spec §8.5).

import type { PageId } from "@paged-media/plugin-api";

import { totalHeightPt, totalWidthPt, type LoweredContent } from "./lowered";

/** Page-local frame bounds, `[top, left, bottom, right]` in pt — the
 *  same order every wire frame op uses (`insertTextFrame` etc.). */
export type Bounds = [number, number, number, number];

/** A resolved placement: the page to insert into + the frame's bounds. */
export interface Placement {
  pageId: PageId;
  bounds: Bounds;
}

/** Fixed inset from the page origin for a default-placed frame (pt). */
export const DEFAULT_INSET_PT = 24;

/** Clamp so a huge range doesn't produce an off-page frame on first
 *  drop — the user repositions/reflows after (T1 pagination splits a
 *  tall range across linked frames; T0 places one bounded frame). */
export const MAX_WIDTH_PT = 540;
export const MAX_HEIGHT_PT = 720;

/** Default placement for a newly lowered frame: a box sized to the
 *  content's total width/height (summed from the IR), clamped to a
 *  sane maximum, at a fixed inset from the page's top-left. Returns
 *  page-local `[top, left, bottom, right]`. */
export function defaultPlacement(
  pageId: PageId,
  content: LoweredContent,
): Placement {
  const width = Math.min(totalWidthPt(content), MAX_WIDTH_PT);
  const height = Math.min(totalHeightPt(content), MAX_HEIGHT_PT);
  const top = DEFAULT_INSET_PT;
  const left = DEFAULT_INSET_PT;
  return {
    pageId,
    bounds: [top, left, top + height, left + width],
  };
}
