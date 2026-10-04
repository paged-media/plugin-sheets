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

import type {
  BundleHost,
  ElementId,
  PageId,
  SceneTreeNode,
} from "@paged-media/plugin-api";
import {
  chartGeometryToMutations,
  makeBinding,
  type ChartPlacement,
} from "../../sheet-host-model/src";

import type { SheetEngine } from "./engine";
import { storyIdsSnapshot } from "./lower";
import { readKnownSwatchIds } from "./swatch-mints";

/** The default chart-frame content box, pt (a sensible publishing size; the
 *  user repositions/resizes after — the geometry is regenerated to fit). */
const [CHART_W_PT, CHART_H_PT] = [360, 240];
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
/** A placed chart (Wave 4): where it went and the page items it made, so a
 *  refresh can replace them. */
export interface PlacedChart {
  chartIndex: number;
  pageId: PageId;
  bounds: [number, number, number, number];
  /** Every element the lowering created (paths + label frames). Empty when
   *  the host has no scene-tree read — such a chart cannot be refreshed. */
  elementIds: ElementId[];
  contentVersion: number;
  /** The geometry it was drawn from (JSON) — a refresh re-lowers only when
   *  the chart's geometry actually changed. */
  geometryKey: string;
}

/** The chart-frame size a placed chart is drawn at. */
export const CHART_SIZE_PT: [number, number] = [CHART_W_PT, CHART_H_PT];

/** Options for [`lowerChartToFrame`]. */
export interface ChartLowerOptions {
  /** Where the chart lands (top-left honoured; the size is the chart's).
   *  Default: 24 pt inset on the active page. */
  placement?: { pageId: PageId; bounds: [number, number, number, number] };
  contentVersion?: number;
  /** Called with the placed chart's record on success. */
  onLowered?: (placed: PlacedChart) => void;
}

/** Every addressable element id in a scene tree, keyed by kind:id. */
function treeIds(nodes: readonly SceneTreeNode[], out = new Map<string, ElementId>()) {
  for (const n of nodes) {
    if (n.id && typeof n.id.id === "string") out.set(`${n.id.kind}:${n.id.id}`, n.id);
    if (n.children) treeIds(n.children, out);
  }
  return out;
}

/** The scene tree's element ids, or null when the host cannot answer. */
async function readTreeIds(host: BundleHost): Promise<Map<string, ElementId> | null> {
  if (typeof host.document.tree !== "function") return null;
  try {
    return treeIds(await host.document.tree());
  } catch {
    return null;
  }
}

/** Remove a placed chart's elements (one undoable batch of deleteFrame —
 *  the door resolves paths and text frames alike). False when there is
 *  nothing recorded to remove or the host refused. */
export async function removePlacedChart(
  host: BundleHost,
  placed: PlacedChart,
): Promise<boolean> {
  if (placed.elementIds.length === 0) return false;
  const r = await host.document.mutate({
    op: "batch",
    args: {
      ops: placed.elementIds.map((id) => ({
        op: "deleteFrame" as const,
        args: { frameId: id.id as string },
      })),
    },
  });
  if (!r.applied) host.log.warn("chart refresh: removing the old chart was rejected", r);
  return r.applied;
}

export async function lowerChartToFrame(
  host: BundleHost,
  engine: SheetEngine,
  chartIndex: number,
  opts?: ChartLowerOptions,
): Promise<boolean> {
  const pageId = opts?.placement?.pageId ?? (await activePageId(host));
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

  const top = opts?.placement?.bounds[0] ?? CHART_INSET_PT;
  const left = opts?.placement?.bounds[1] ?? CHART_INSET_PT;
  const placement: ChartPlacement = {
    pageId,
    bounds: [top, left, top + geom.heightPt, left + geom.widthPt],
  };
  // The binding marks the frame group as a chart of this sheet (the title /
  // chart index ride as the range slot — a chart binds to its parsed index,
  // re-resolved on recalc). contentVersion 0: T0 has no revision counter.
  const contentVersion = opts?.contentVersion ?? 0;
  const binding = makeBinding(sheetName, `chart:${chartIndex}`, contentVersion);

  // The colour swatches this chart references ride INSIDE the phase-1
  // batch, at deterministic content-addressed ids. Read what the document
  // already carries so an existing colour is REFERENCED, not re-created:
  // core refuses a duplicate `createSwatch` and the refusal fails the whole
  // batch, so before this read a second `lowerChartToFrame` — of the same
  // chart OR of any other chart, since every chart shares the axis grey —
  // landed NOTHING (measured: scene tree unchanged at 18 nodes). A failed
  // read mints nothing rather than gamble the batch; the art still lands,
  // unpainted. See `swatch-mints.ts`.
  const { batch, texts } = chartGeometryToMutations(
    geom,
    placement,
    binding,
    await readKnownSwatchIds(host),
  );
  if (
    batch.op === "batch" &&
    (batch as { args: { ops: unknown[] } }).args.ops.length === 0
  ) {
    host.log.warn("lowerChart: empty chart geometry — nothing to lower");
    return false;
  }

  // Phase 1 — every vector path + the label frames + binding, one undoable batch.
  // Snapshot the stories FIRST: the label frames' stories are resolved by
  // diffing this collection across the batch (see below).
  const storiesBefore = await storyIdsSnapshot(host);
  // Wave 4: the scene tree before/after names every element this batch
  // made, so the chart can be REPLACED when the data changes.
  const treeBefore = opts?.onLowered ? await readTreeIds(host) : null;
  const outcome = await host.document.mutate(batch);
  if (!outcome.applied) {
    host.log.warn("lowerChart: phase-1 batch rejected", outcome);
    return false;
  }
  if (opts?.onLowered) {
    const treeAfter = treeBefore ? await readTreeIds(host) : null;
    const elementIds =
      treeBefore && treeAfter
        ? [...treeAfter].filter(([k]) => !treeBefore.has(k)).map(([, id]) => id)
        : [];
    opts.onLowered({
      chartIndex,
      pageId,
      bounds: placement.bounds,
      elementIds,
      contentVersion,
      geometryKey: JSON.stringify(geom),
    });
  }

  // Phase 2 — pour each label's text into its frame. The batch created one
  // text frame per label, in `texts` order, and each frame was born with a
  // story of its own, so the stories that are NEW after the batch are the
  // labels' stories, in mint order (the engine numbers them ascending).
  //
  // This used to hit-test the label's anchor point. The text hit path
  // cannot see an EMPTY frame (`storyId: null`, verified live — the table
  // lowering learned it first), so the hit fell through to whatever text
  // frame lay UNDER the anchor — on a page with prose, the page's own
  // heading — and the label's text was poured into THAT story at offset 0
  // ("Q2Q2The chart wall"), while the label frame stayed empty. A resolution
  // that can answer with someone else's story is not a resolution; the diff
  // can only answer with a story this batch minted.
  const fresh = await newStoryIdsInMintOrder(host, storiesBefore);
  if (fresh.length !== texts.length) {
    host.log.warn(
      `lowerChart: the batch minted ${fresh.length} stor${fresh.length === 1 ? "y" : "ies"} for ${texts.length} label(s); labels left empty`,
    );
  }
  for (const [i, label] of texts.entries()) {
    if (label.text.length === 0) continue;
    const storyId = fresh.length === texts.length ? fresh[i] : null;
    if (!storyId) continue;
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

/** The story ids that exist now and did not before, in MINT order. Minted
 *  ids are `Story/u<n>` with ascending decimal `n` (the engine's
 *  `story_id_floor` numbering); anything else sorts after them. */
async function newStoryIdsInMintOrder(
  host: BundleHost,
  before: ReadonlySet<string>,
): Promise<string[]> {
  const after = await host.document.collection<{ selfId: string }>("stories");
  const num = (id: string): number => {
    const m = /^Story\/u(\d+)$/.exec(id);
    return m ? Number(m[1]) : Number.POSITIVE_INFINITY;
  };
  return after
    .map((s) => s.selfId)
    .filter((id) => !before.has(id))
    .sort((a, b) => num(a) - num(b) || a.localeCompare(b));
}
