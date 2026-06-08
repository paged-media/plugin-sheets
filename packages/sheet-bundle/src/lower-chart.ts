// Lower a CHART to a paged.draw vector frame (spec §8.4 / §2.1). The engine
// generates the pure geometry IR (sheet-chart, Rust); the host-model
// translator turns it into native paged.draw mutations (insertPath +
// insertTextFrame); THIS module drives the host writes — the chart analogue
// of lower.ts.
//
// §2.1: paged.draw is a CORE SDK surface reached through the native wire ops,
// NEVER another plugin. A lowered chart is document-native vector art.
//
// TWO-PHASE (mirrors lower.ts). Phase 1 is ONE undoable batch: every vector
// path (insertPath) + one insertTextFrame per label + the binding metadata
// on the first created element. Phase 2 pours each label's text into its new
// frame's story — resolved through the hitTest read door (the only door that
// yields a created frame's storyId), exactly as the sheet lower does.

import type { BundleHost, PageId } from "@paged-media/plugin-api";
import {
  chartGeometryToMutations,
  makeBinding,
  type ChartPlacement,
} from "@paged-media/sheet-host-model";

import type { SheetEngine } from "./engine";

/** The default chart-frame content box, pt (a sensible publishing size; the
 *  user repositions/resizes after — the geometry is regenerated to fit). */
const CHART_W_PT = 360;
const CHART_H_PT = 240;
/** Fixed inset from the page origin for a freshly lowered chart frame. */
const CHART_INSET_PT = 24;

/** The active page id (meta first, else the first page). Mirrors lower.ts. */
async function activePageId(host: BundleHost): Promise<PageId | null> {
  const meta = await host.document.meta();
  if (meta.activePage) return meta.activePage;
  const pages = await host.document.collection<{ selfId: string }>("pages");
  return pages.length > 0 ? pages[0].selfId : null;
}

/**
 * Lower chart `chartIndex` to a fresh page frame of vector art. Engine
 * generates the IR (all chart semantics in Rust); the translator (pure)
 * shapes the mutations; this drives the two-phase host writes. Returns true
 * on a successful phase-1 apply, false on any failure (mutate-never-throws:
 * outcomes are checked, not caught).
 */
export async function lowerChartToFrame(
  host: BundleHost,
  engine: SheetEngine,
  chartIndex: number,
): Promise<boolean> {
  const pageId = await activePageId(host);
  if (!pageId) {
    host.log.warn("lowerChart: no page to place the chart frame into");
    return false;
  }

  // Engine-computed geometry IR (the chart subsystem lives in Rust).
  let geom;
  try {
    geom = engine.getChartGeometry(chartIndex, CHART_W_PT, CHART_H_PT);
  } catch (err) {
    host.log.warn("lowerChart: engine.getChartGeometry failed", err);
    return false;
  }

  const charts = engine.listCharts();
  const info = charts.find((c) => c.index === chartIndex);
  const sheetName =
    info != null
      ? (engine.listSheets().find((s) => s.id === info.hostSheet)?.name ??
        String(info.hostSheet))
      : String(chartIndex);

  const top = CHART_INSET_PT;
  const left = CHART_INSET_PT;
  const placement: ChartPlacement = {
    pageId,
    bounds: [top, left, top + geom.heightPt, left + geom.widthPt],
  };
  // The binding marks the frame group as a chart of this sheet (the title /
  // chart index ride as the range slot — a chart binds to its parsed index,
  // re-resolved on recalc). contentVersion 0: T0 has no revision counter.
  const binding = makeBinding(sheetName, `chart:${chartIndex}`, 0);

  const { batch, texts } = chartGeometryToMutations(geom, placement, binding);
  if (
    batch.op === "batch" &&
    (batch as { args: { ops: unknown[] } }).args.ops.length === 0
  ) {
    host.log.warn("lowerChart: empty chart geometry — nothing to lower");
    return false;
  }

  // Phase 1 — every vector path + the label frames + binding, one undoable batch.
  const outcome = await host.document.mutate(batch);
  if (!outcome.applied) {
    host.log.warn("lowerChart: phase-1 batch rejected", outcome);
    return false;
  }

  // Phase 2 — pour each label's text into its frame. The batch created the
  // label frames in `texts` order; we resolve each frame's story via the
  // hitTest read door at the label's anchor point, then insertText. A label
  // whose story can't be resolved is skipped (the honest S-03-style gap).
  for (const label of texts) {
    if (label.text.length === 0) continue;
    const hit = await host.document.hitTest(pageId, label.at);
    const storyId = hit?.storyId ?? null;
    if (!storyId) {
      host.log.debug(
        "lowerChart: could not resolve a label's story; label left empty",
      );
      continue;
    }
    const pour = await host.document.mutate({
      op: "insertText",
      args: { storyId, offset: 0, text: label.text },
    });
    if (!pour.applied) {
      host.log.debug("lowerChart: a label insertText was rejected", pour);
    }
  }

  if (outcome.createdId) await host.selection.set([outcome.createdId]);
  return true;
}
