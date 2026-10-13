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
// ONE BATCH (Wave 9): every vector path (insertPath) + one insertTextFrame
// per label + the binding metadata on the first created element + each
// label's text, poured through the frame's C-15 handle — and, on a refresh,
// the deletes of the chart it replaces. The elements it made come back in
// the 66 `minted` list (a scene-tree diff on an older host). A host that
// refuses in-batch handles gets the two-phase lane: the plain batch, then
// the labels poured into the stories the batch minted (a stories diff).

import type {
  BundleHost,
  ElementId,
  Mutation,
  PageId,
} from "@paged-media/plugin-api";
import {
  chartGeometryToMutations,
  labelStyleOps,
  makeBinding,
  type ChartGeometry,
  type ChartPlacement,
  type ChartTextLabel,
  type LabelMetrics,
  type TextPrim,
} from "../../sheet-host-model/src";

import type { SheetEngine } from "./engine";
import { selectionBox, storyIdsSnapshot } from "./lower";
import { doors66, noteMinted, readTreeIds } from "./protocol66";
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
  /** Every element the lowering created (paths + label frames) — the
   *  group's MEMBERS, never the group itself. Empty when the host has no
   *  scene-tree read — such a chart cannot be refreshed. */
  elementIds: ElementId[];
  /** The group the chart's elements were placed in (one object to select,
   *  move and delete), or null when the host placed it ungrouped. */
  groupId?: string | null;
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
  /** A chart this one REPLACES (a refresh): its elements are deleted in the
   *  same batch, so the replacement is one undo step. */
  replaces?: PlacedChart;
}

/** The ops that remove a placed chart: its group dissolved (core has no
 *  delete for a group, and deleting the members alone leaves an EMPTY group
 *  behind — measured), then every element deleted (`deleteFrame` resolves
 *  paths and text frames alike). `withGroup: false` skips the dissolve, for a
 *  chart whose group the user has already taken apart. */
export function chartRemovalOps(placed: PlacedChart, withGroup = true): Mutation[] {
  return [
    ...(withGroup && placed.groupId
      ? [{ op: "dissolveGroup" as const, args: { groupId: placed.groupId } }]
      : []),
    ...placed.elementIds.map((id) => ({
      op: "deleteFrame" as const,
      args: { frameId: id.id as string },
    })),
  ];
}

/** Remove a placed chart's elements (one undoable batch). False when there
 *  is nothing recorded to remove or the host refused. A chart whose group
 *  is gone (the user ungrouped it) is removed element by element. */
export async function removePlacedChart(
  host: BundleHost,
  placed: PlacedChart,
): Promise<boolean> {
  if (placed.elementIds.length === 0) return false;
  let r = await host.document.mutate({ op: "batch", args: { ops: chartRemovalOps(placed) } });
  if (!r.applied && placed.groupId) {
    r = await host.document.mutate({
      op: "batch",
      args: { ops: chartRemovalOps(placed, false) },
    });
  }
  if (!r.applied) host.log.warn("chart refresh: removing the old chart was rejected", r);
  return r.applied;
}

/** The within-batch handle the chart's group is named by. */
const CHART_GROUP_HANDLE = "chart";

/** Gap (pt) between a placed chart and the selection it is placed beside. */
export const CHART_GAP_PT = 12;

/** Where a chart lands when something is selected: BESIDE it, never on it —
 *  below it when the chart fits the page there, else to its right, else
 *  below it regardless (a chart hanging past the page edge is visible and
 *  can be moved; one lying over the table it charts was the film's bug). The
 *  bounds carry the top-left only; the size is the chart's. Null with
 *  nothing usable selected (the caller's default placement applies). */
export async function chartPlacementBesideSelection(
  host: BundleHost,
): Promise<{ pageId: PageId; bounds: [number, number, number, number] } | null> {
  const box = await selectionBox(host);
  if (!box) return null;
  const [w, h] = CHART_SIZE_PT;
  let size: [number, number] = [Number.POSITIVE_INFINITY, Number.POSITIVE_INFINITY];
  try {
    const pages = await host.document.collection<{ selfId: string; sizePt?: [number, number] }>("pages");
    size = pages.find((p) => p.selfId === box.pageId)?.sizePt ?? size;
  } catch {
    /* no page size: any spot "fits" — below the selection */
  }
  const fits = (top: number, left: number) => top + h <= size[1] && left + w <= size[0];
  const below = { top: box.bottom + CHART_GAP_PT, left: box.left };
  const right = { top: box.top, left: box.right + CHART_GAP_PT };
  const at = fits(below.top, below.left) ? below : fits(right.top, right.left) ? right : below;
  return { pageId: box.pageId, bounds: [at.top, at.left, at.top, at.left] };
}

/** Every label of `geom`, measured at its own size with the host's shaper —
 *  ONE call per distinct size (`measureStrings`), in the face an unstyled
 *  run is laid out in (`""`, the engine's default face, which is what the
 *  labels are poured as). A label the host cannot measure is `null`: the
 *  translator then sizes it from the chart engine's own estimate. */
async function measureLabels(
  host: BundleHost,
  geom: ChartGeometry,
): Promise<(LabelMetrics | null)[]> {
  const labels = geom.prims.filter((p): p is TextPrim => p.kind === "text");
  const out: (LabelMetrics | null)[] = labels.map(() => null);
  const bySize = new Map<number, number[]>();
  labels.forEach((l, i) => bySize.set(l.sizePt, [...(bySize.get(l.sizePt) ?? []), i]));
  for (const [size, at] of bySize) {
    try {
      const measured = await host.text.measureStrings(
        "",
        null,
        at.map((i) => labels[i].s),
        size,
      );
      at.forEach((i, k) => {
        const m = measured[k];
        if (m && Number.isFinite(m.advance) && m.advance > 0) {
          out[i] = { advance: m.advance, ascender: m.ascender, descender: m.descender };
        }
      });
    } catch (err) {
      host.log.debug("lowerChart: label measurement failed — estimated sizes", err);
    }
  }
  return out;
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
    await measureLabels(host, geom),
  );
  if (
    batch.op === "batch" &&
    (batch as { args: { ops: unknown[] } }).args.ops.length === 0
  ) {
    host.log.warn("lowerChart: empty chart geometry — nothing to lower");
    return false;
  }

  // ONE batch: every vector path, the label frames, the binding, the GROUP
  // that makes them one object — and each label's text, poured through the
  // frame's C-15 handle, set at the chart's size and justified to its anchor
  // (`bindCreated` right after each create, the group and the pours at the
  // end so no `$created` the style/binding ops name moves). One mutate and
  // one undo step for the whole chart; no story resolution at all.
  const ops = (batch as { args: { ops: Mutation[] } }).args.ops;
  const withHandles: Mutation[] = [];
  const pours: Mutation[] = [];
  const members: ElementId[] = [];
  let label = 0;
  let path = 0;
  for (const op of ops) {
    withHandles.push(op);
    if (op.op === "insertPath") {
      const handle = `p${path}`;
      path += 1;
      withHandles.push({ op: "bindCreated", args: { handle } });
      members.push({ kind: "polygon", id: `$h:${handle}` });
      continue;
    }
    if (op.op !== "insertTextFrame") continue;
    const handle = `l${label}`;
    const text: ChartTextLabel | undefined = texts[label];
    label += 1;
    withHandles.push({ op: "bindCreated", args: { handle } });
    members.push({ kind: "textFrame", id: `$h:${handle}` });
    if (!text || text.text.length === 0) continue;
    pours.push(
      { op: "insertText", args: { storyId: `$h:${handle}`, offset: 0, text: text.text } },
      ...labelStyleOps(`$h:${handle}`, text),
    );
  }
  const group: Mutation[] =
    members.length >= 2
      ? [
          { op: "createGroup", args: { memberIds: members } },
          { op: "bindCreated", args: { handle: CHART_GROUP_HANDLE } },
        ]
      : [];

  // Wave 4: a refresh REPLACES the chart, so it needs every element the
  // batch made. The 66 `minted` list names them; a host that does not send
  // it gets the scene-tree diff (read before, read after).
  const doors = doors66(host);
  const treeBefore =
    opts?.onLowered && doors.minted !== true ? await readTreeIds(host) : null;
  const removals = opts?.replaces ? chartRemovalOps(opts.replaces) : [];
  const place = [...withHandles, ...group, ...pours];
  let outcome = await host.document.mutate({
    op: "batch",
    args: { ops: [...removals, ...place] },
  });
  if (!outcome.applied && opts?.replaces) {
    // The old chart may not be as it was placed (the user ungrouped it, or
    // deleted a piece): take away what is left of it, then place.
    await removePlacedChart(host, opts.replaces);
    outcome = await host.document.mutate({ op: "batch", args: { ops: place } });
  }
  if (!outcome.applied) {
    // A host without in-batch handles: the plain batch, then the labels.
    host.log.debug("lowerChart: one-batch chart refused — two-phase", outcome);
    const storiesBefore = await storyIdsSnapshot(host);
    outcome = await host.document.mutate(batch);
    if (!outcome.applied) {
      host.log.warn("lowerChart: phase-1 batch rejected", outcome);
      return false;
    }
    await pourLabels(host, storiesBefore, texts);
  }
  const minted = noteMinted(host, outcome);
  if (opts?.onLowered) {
    let made: ElementId[] = [];
    if (minted) {
      made = minted.map((m) => m.element);
    } else {
      const before = treeBefore ?? null;
      const treeAfter = before ? await readTreeIds(host) : null;
      made =
        before && treeAfter
          ? [...treeAfter].filter(([k]) => !before.has(k)).map(([, id]) => id)
          : [];
    }
    const groupId = made.find((id) => id.kind === "group")?.id ?? null;
    opts.onLowered({
      chartIndex,
      pageId,
      bounds: placement.bounds,
      elementIds: made.filter((id) => id.kind !== "group"),
      groupId: typeof groupId === "string" ? groupId : null,
      contentVersion,
      geometryKey: JSON.stringify(geom),
    });
  }

  if (outcome.applied && outcome.createdId) await host.selection.set([outcome.createdId]);
  return true;
}

/** The two-phase label pour (a host without in-batch handles): each label's
 *  text into its frame's story. The batch created one text frame per label,
 *  in `texts` order, and each frame was born with a story of its own, so the
 *  stories that are NEW after the batch are the labels' stories, in mint
 *  order (the engine numbers them ascending).
 *
 *  This used to hit-test the label's anchor point. The text hit path cannot
 *  see an EMPTY frame (`storyId: null`), so the hit fell through to whatever
 *  text frame lay UNDER the anchor — on a page with prose, the page's own
 *  heading — and the label's text was poured into THAT story. A resolution
 *  that can answer with someone else's story is not a resolution; the diff
 *  can only answer with a story this batch minted. */
async function pourLabels(
  host: BundleHost,
  storiesBefore: ReadonlySet<string>,
  texts: readonly ChartTextLabel[],
): Promise<void> {
  const fresh = await newStoryIdsInMintOrder(host, storiesBefore);
  if (fresh.length !== texts.length) {
    host.log.warn(
      `lowerChart: the batch minted ${fresh.length} stor${fresh.length === 1 ? "y" : "ies"} for ${texts.length} label(s); labels left empty`,
    );
    return;
  }
  for (const [i, label] of texts.entries()) {
    if (label.text.length === 0) continue;
    // The text, set at the chart's size and justified to its anchor — one
    // mutate per label, as before.
    const pour = await host.document.mutate({
      op: "batch",
      args: {
        ops: [
          { op: "insertText", args: { storyId: fresh[i], offset: 0, text: label.text } },
          ...labelStyleOps(fresh[i], label),
        ],
      },
    });
    if (!pour.applied) {
      host.log.debug("lowerChart: a label insertText was rejected", pour);
    }
  }
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
