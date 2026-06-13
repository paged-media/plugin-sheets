// The NATIVE-TABLE page lower (S-03 RESOLVED; spec §8.2). The engine
// lowers the range to the IR (Rust), the host-model translators turn it
// into wire ops (pure), and THIS module drives the host writes — the
// only place in the bundle that calls host.document.mutate.
//
// TWO LANES:
//
//   native-table (DEFAULT) — three phases:
//     Phase 1 — mutate(batch): insertTextFrame + setPluginMetadata
//       (binding) as ONE undoable step; the outcome mints the frame id.
//       No drawn rules: the table carries its own cell edges.
//     Phase 2 — resolve the frame's storyId, then mutate(insertTable)
//       sized by font metrics (S-13); the outcome mints the tableId.
//     Phase 3 — mutate(batch): the cell pour (insertText per cell via
//       TextCellAddr) + decor (setCellSpan per merge, cellFillColor +
//       cell*EdgeStrokeWeight via tableCell-scoped setElementProperty).
//
//   tab-text (EXPLICIT FALLBACK, spec §2.2 degradation) — the retained
//     two-phase lane (lower-to-mutations.ts): frame + drawn rules +
//     binding, then the tab/newline text pour. Selected via the `lane`
//     option, and used at runtime when a host REJECTS insertTable (an
//     older wire) — the degradation stays available and tested.
//
// RESOLVING THE STORY (the read door). plugin-api exposes no direct
// frame→story lookup (SceneTreeNode/collections don't carry it); the
// available door that DOES is `host.document.hitTest`, whose HitResult
// carries `storyId`. So we hit-test the new frame's centre to recover
// its story. This mirrors how a created element is re-resolved through a
// read door (plugin-web re-reads its created frame via getMetadata; here
// the needed datum is the story, and hitTest is the door that yields it).

import type {
  BundleHost,
  Disposable,
  ElementId,
  PageId,
} from "@paged-media/plugin-api";
import {
  BINDING_KEY,
  defaultPlacement,
  joinText,
  lowerToMutations,
  makeBinding,
  pageTableMutations,
  tableContentBatch,
  tableInsertOp,
  type LoweredContent,
  type Page,
} from "@paged-media/sheet-host-model";

import type { FrameBox, SheetEngine } from "./engine";

/** Per-column width (pt) from the document's font metrics (S-13). For
 *  each column, measure the widest formatted cell text via the host
 *  shaper and add a small inset; fall back to the IR's char-based width
 *  when the shaper is unwired or yields nothing. Keeps the page table and
 *  any future grid view resolving to the SAME widths (the §8.3
 *  cross-surface-consistency requirement). */
async function measureColumnWidths(
  host: BundleHost,
  content: LoweredContent,
): Promise<number[]> {
  const styleOf = (key: number | undefined) =>
    key == null ? null : (content.styles ?? []).find((s) => s.key === key) ?? null;
  const CELL_INSET_PT = 4; // left+right padding inside a cell

  return Promise.all(
    content.cols.map(async (col) => {
      let widest = "";
      let style: ReturnType<typeof styleOf> = null;
      for (const row of content.rows) {
        for (const cell of row.cells) {
          if (cell.col === col.index && cell.text.length > widest.length) {
            widest = cell.text;
            style = styleOf(cell.styleKey);
          }
        }
      }
      if (widest.length === 0) return col.widthPt;
      const metrics = await host.text.measureString(
        style?.fontName ?? "",
        style?.bold || style?.italic ? "Bold" : null,
        widest,
        style?.fontSizePt ?? 11,
      );
      const measured = metrics.advance + CELL_INSET_PT;
      return measured > 0 ? measured : col.widthPt;
    }),
  );
}

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

/** Which translation lane the page lower drives. `native-table` (the
 *  default) emits a real Paged `<Table>`; `tab-text` is the retained
 *  spec §2.2 degradation (tab-aligned text + drawn rules). */
export type LowerLane = "native-table" | "tab-text";

/** What a successful native-table lowering produced — the frame, its
 *  resolved story, and the minted table id (S-04: the cell-style consumer
 *  addresses this table's cells). Reported via [`LowerLaneOptions.onLowered`]
 *  so the return type (the frame id string) stays unchanged for existing
 *  callers. */
export interface LoweredTableInfo {
  frameId: string;
  storyId: string;
  tableId: string;
  sheet: number;
  range: string;
}

/** Lane options for [`lowerSelectionToFrame`]. */
export interface LowerLaneOptions {
  /** Force a lane; default `"native-table"`. The tab-text fallback also
   *  engages at runtime when the host rejects `insertTable`. */
  lane?: LowerLane;
  /** Called when a NATIVE TABLE landed (not the tab-text fallback) with the
   *  resolved frame/story/table ids — lets the session record the lowered
   *  table so a later "new style from cell" can address its cells (S-04).
   *  Never called on the fallback (no native table to address). */
  onLowered?: (info: LoweredTableInfo) => void;
}

/**
 * Lower `sheet`/`range` to a fresh page frame. Engine computes the IR;
 * the translators (pure) shape the mutations; this drives the host
 * writes. Returns the created frame's raw id, or null on any failure
 * (mutate-never-throws: outcomes are checked, not caught).
 */
/** Snapshot the document's story ids (the `stories` collection). */
async function storyIdsSnapshot(host: BundleHost): Promise<Set<string>> {
  const items = await host.document.collection<{ selfId: string }>("stories");
  return new Set(items.map((s) => s.selfId));
}

/** Resolve a JUST-CREATED frame's story by DIFFING the stories
 *  collection across the insert. The hitTest read door reports
 *  `storyId: null` for an EMPTY text frame (verified against the real
 *  engine — the text hit path needs content), so the only working
 *  resolution today is the before/after diff: exactly one new story id
 *  belongs to the new frame. The proper frame→story read door is named
 *  in the cross-repo RFI (v43 batch candidate). */
async function newStoryId(
  host: BundleHost,
  before: ReadonlySet<string>,
): Promise<string | null> {
  const after = await host.document.collection<{ selfId: string }>("stories");
  const fresh = after
    .map((s) => s.selfId)
    .filter((id) => !before.has(id));
  return fresh.length === 1 ? fresh[0] : null;
}

export async function lowerSelectionToFrame(
  host: BundleHost,
  engine: SheetEngine,
  sheet: number,
  range: string,
  opts?: LowerLaneOptions,
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

  if (opts?.lane === "tab-text") {
    return lowerTabTextToFrame(host, content, placement, binding);
  }

  // Snapshot story ids BEFORE phase 1 — the new frame's story is the
  // diff (see newStoryId).
  const storiesBefore = await storyIdsSnapshot(host);

  // Phase 1 — the frame + its binding, one undoable step. NO drawn rules:
  // a native `<Table>` (S-03 RESOLVED, protocol v37) carries its own cell
  // edges (phase 3). The binding rides the batch-created frame via
  // `$created`.
  const outcome = await host.document.mutate({
    op: "batch",
    args: {
      ops: [
        { op: "insertTextFrame", args: { pageId, bounds: placement.bounds } },
        {
          op: "setPluginMetadata",
          args: {
            elementId: { kind: "textFrame", id: "$created" },
            key: BINDING_KEY,
            value: JSON.stringify(binding),
          },
        },
      ],
    },
  });
  if (!outcome.applied || !outcome.createdId) {
    host.log.warn("lower: phase-1 frame batch rejected", outcome);
    return null;
  }
  const frameId = frameIdOf(outcome.createdId);
  if (!frameId) {
    host.log.warn("lower: created element is not a frame target");
    return null;
  }

  // Resolve the new frame's story by DIFFING the stories collection
  // (snapshotted before phase 1) — the hitTest door cannot see an empty
  // frame's story (storyId:null, verified live); see newStoryId().
  const storyId = await newStoryId(host, storiesBefore);
  if (!storyId) {
    host.log.warn(
      "lower: could not resolve the created frame's story; frame placed " +
        "empty (stories-diff ambiguous)",
    );
    await host.selection.set([outcome.createdId]);
    return frameId;
  }

  // Phase 2 — create the native table in that story, sized by font
  // metrics (S-13). createdId is the new tableId.
  const columnWidths = await measureColumnWidths(host, content);
  const tableOutcome = await host.document.mutate(
    tableInsertOp(content, storyId, columnWidths),
  );
  if (!tableOutcome.applied || !tableOutcome.createdId) {
    // RUNTIME FALLBACK: a host whose wire predates insertTable rejects the
    // op — degrade to the spec §2.2 tab-text pour into the story we already
    // resolved (the frame + binding stand; rules are not retrofittable
    // without the table). Honest, logged, never silent.
    host.log.warn(
      "lower: insertTable rejected — falling back to the tab-text pour",
      tableOutcome,
    );
    const text = joinText(content);
    if (text.length > 0) {
      const pour = await host.document.mutate({
        op: "insertText",
        args: { storyId, offset: 0, text },
      });
      if (!pour.applied) {
        host.log.warn("lower: fallback text pour rejected", pour);
      }
    }
    await host.selection.set([outcome.createdId]);
    return frameId;
  }
  const tableId = tableOutcome.createdId.id as string;

  // S-04 — report the resolved native table so the session can address its
  // cells for "new style from cell". Only the native-table lane reports
  // (the tab-text fallback has no table to address).
  opts?.onLowered?.({ frameId, storyId, tableId, sheet, range });

  // Phase 3 — ONE batch: pour each cell's formatted text into its table
  // cell, then the decor (merges → setCellSpan; style fills/borders + grid
  // rules → tableCell-scoped cellFillColor / cell*EdgeStrokeWeight).
  const { batch: cellBatch, unmappedRules } = tableContentBatch(
    content,
    storyId,
    tableId,
  );
  if (unmappedRules > 0) {
    host.log.warn(
      `lower: ${unmappedRules} grid rule(s) aligned to no cell boundary ` +
        "(not drawn natively)",
    );
  }
  if (cellBatch.op === "batch" && cellBatch.args.ops.length > 0) {
    const pour = await host.document.mutate(cellBatch);
    if (!pour.applied) host.log.warn("lower: phase-3 cell pour rejected", pour);
  }

  await host.selection.set([outcome.createdId]);
  return frameId;
}

/** The retained tab-text lane (spec §2.2 degradation): the pure
 *  `lowerToMutations` batch (frame + drawn rules + binding), then the
 *  tab/newline text pour into the resolved story. */
async function lowerTabTextToFrame(
  host: BundleHost,
  content: LoweredContent,
  placement: { pageId: PageId; bounds: [number, number, number, number] },
  binding: ReturnType<typeof makeBinding>,
): Promise<string | null> {
  const { batch, text } = lowerToMutations(content, placement, binding);

  // Snapshot story ids before the frame insert (see newStoryId).
  const storiesBefore = await storyIdsSnapshot(host);

  const outcome = await host.document.mutate(batch);
  if (!outcome.applied || !outcome.createdId) {
    host.log.warn("lower(tab-text): phase-1 batch rejected", outcome);
    return null;
  }
  const frameId = frameIdOf(outcome.createdId);
  if (!frameId) {
    host.log.warn("lower(tab-text): created element is not a frame target");
    return null;
  }

  const storyId = await newStoryId(host, storiesBefore);
  if (!storyId) {
    host.log.warn(
      "lower(tab-text): could not resolve the created frame's story; " +
        "frame placed empty (stories-diff ambiguous)",
    );
    await host.selection.set([outcome.createdId]);
    return frameId;
  }

  if (text.length > 0) {
    const pour = await host.document.mutate({
      op: "insertText",
      args: { storyId, offset: 0, text },
    });
    if (!pour.applied) {
      host.log.warn("lower(tab-text): phase-2 text pour rejected", pour);
    }
  }

  await host.selection.set([outcome.createdId]);
  return frameId;
}

// ── Live multi-frame pagination across the host frame chain (Wave 2D,
// RFI C-2 / S-05; spec §8.2 "the killer feature"). The engine threads a
// tall range across the chain's content boxes (Rust); this flow reads the
// real chain via host.document.frameChain, resolves each frame's content
// box via host.document.elementGeometry, lowers each Page into ITS frame's
// story, and re-paginates when a content-box reflow event fires (§8.5: a
// pure transform — move/scale/rotate — never re-paginates; only a
// resizeFrame does, carried by DocumentChangeEvent.reflow).

/** A resolved chain frame: its raw frame id + content box (frame-content pt,
 *  §8.5 — the geometry door's bounds ARE the content box). */
interface ChainFrame {
  frameId: string;
  box: FrameBox;
}

/** The active page id of a story-bearing frame, recovered via hitTest's
 *  storyId on the frame center. */
async function frameStoryId(
  host: BundleHost,
  pageId: PageId,
  bounds: [number, number, number, number],
): Promise<string | null> {
  const hit = await host.document.hitTest(pageId, center(bounds));
  return hit?.storyId ?? null;
}

/** Resolve a frame's content box (frame-content pt) from its page geometry.
 *  `elementGeometry` returns `bounds: [top, left, bottom, right]` in
 *  content-box space (§8.5) — exactly the box pagination threads into. */
function boxOf(bounds: [number, number, number, number]): FrameBox {
  const [top, left, bottom, right] = bounds;
  return { widthPt: right - left, heightPt: bottom - top };
}

/**
 * Read the host frame chain starting from `storyId` and resolve each link's
 * content box (Wave 2D / S-05). Returns the ordered `ChainFrame[]` — the
 * input to pagination. A link with no resolvable geometry is dropped (the
 * caller under-provisioned; pagination tolerates a short chain). Empty when
 * the story threads no frames.
 */
export async function resolveChain(
  host: BundleHost,
  storyId: string,
): Promise<ChainFrame[]> {
  const links = await host.document.frameChain(storyId);
  if (links.length === 0) return [];

  const ids = links.map((l) => ({
    kind: "textFrame" as const,
    id: l.frameId,
  }));
  const geom = await host.document.elementGeometry(ids);
  const byId = new Map(geom.map((g) => [idOf(g.id), g.bounds]));

  const chain: ChainFrame[] = [];
  for (const link of links) {
    const bounds = byId.get(link.frameId);
    if (!bounds) continue; // no geometry → drop this link (honest shortfall)
    chain.push({ frameId: link.frameId, box: boxOf(bounds) });
  }
  return chain;
}

/** The raw id string of an ElementId (textFrame/rectangle carry a string
 *  id; others are out of scope for the chain). */
function idOf(id: ElementId): string | null {
  if (id.kind === "textFrame" || id.kind === "rectangle") {
    return id.id as string;
  }
  return null;
}

/**
 * Lower one paginated `Page` into a chain frame's story as a native table
 * (Wave 2D / S-05). Resolves the frame's storyId via hitTest, then drives
 * the two-phase native-table emission (`pageTableMutations`): insert (its
 * outcome mints the tableId) → pour the cells. Returns the resolved tableId
 * or null on any failure (mutate-never-throws: outcomes are checked).
 */
async function lowerPageToFrame(
  host: BundleHost,
  pageId: PageId,
  frame: ChainFrame,
  page: Page,
): Promise<string | null> {
  const storyId = await frameStoryId(host, pageId, frameBounds(frame));
  if (!storyId) {
    host.log.warn(
      `chain-lower: could not resolve story for frame ${frame.frameId}`,
    );
    return null;
  }

  const columnWidths = await measureColumnWidths(host, page.content);
  const ops = pageTableMutations(page, storyId, columnWidths);

  const tableOutcome = await host.document.mutate(ops.insert);
  if (!tableOutcome.applied || !tableOutcome.createdId) {
    host.log.warn("chain-lower: insertTable rejected", tableOutcome);
    return null;
  }
  const tableId = tableOutcome.createdId.id as string;

  const cellBatch = ops.cells(tableId);
  if (cellBatch.op === "batch" && cellBatch.args.ops.length > 0) {
    const pour = await host.document.mutate(cellBatch);
    if (!pour.applied) host.log.warn("chain-lower: cell pour rejected", pour);
  }
  return tableId;
}

/** A chain frame's page-local bounds for the hitTest center. The geometry
 *  door gave us the content box; we reconstruct a bounds tuple at the
 *  origin so the center lands inside the frame (hitTest uses page-local
 *  coords, but a frame's own center in content space coincides with the
 *  hittable interior — the existing single-frame flow uses the placement
 *  bounds the same way). */
function frameBounds(frame: ChainFrame): [number, number, number, number] {
  return [0, 0, frame.box.heightPt, frame.box.widthPt];
}

/** The result of a chain pagination pass. */
export interface ChainLowerResult {
  /** The story whose chain was paginated. */
  storyId: string;
  /** The resolved chain frames (in order). */
  chain: ChainFrame[];
  /** The pages the engine produced (one per filled frame). */
  pages: Page[];
  /** The tableId lowered into each page's frame (null where a frame's story
   *  could not be resolved or the table was rejected). */
  tableIds: (string | null)[];
}

/**
 * Lower `sheet`/`range` ACROSS a host frame chain with live pagination
 * (Wave 2D, RFI C-2 / S-05; spec §8.2). Reads the real chain via
 * `host.document.frameChain(storyId)`, resolves each frame's content box via
 * `host.document.elementGeometry`, asks the engine to paginate the range into
 * those boxes (all threading math in Rust), and lowers each resulting `Page`
 * into ITS frame's story as a native table. Returns the pass result, or null
 * when no chain resolves.
 *
 * `chainStoryId` selects the chain; pass the story of the active/first frame
 * (the caller resolves it from selection or a known frame). The caller may
 * instead supply a ready `chain` (the frames + boxes) to bypass the host
 * reads — same downstream lowering.
 */
export async function lowerPaginatedToChain(
  host: BundleHost,
  engine: SheetEngine,
  sheet: number,
  range: string,
  chainStoryId: string,
  opts?: {
    repeatedHeaderRows?: number;
    continuedMarker?: boolean;
    keepRowsTogether?: [number, number][];
    chain?: ChainFrame[];
  },
): Promise<ChainLowerResult | null> {
  const pageId = await activePageId(host);
  if (!pageId) {
    host.log.warn("chain-lower: no page to paginate into");
    return null;
  }

  const chain =
    opts?.chain ?? (await resolveChain(host, chainStoryId));
  if (chain.length === 0) {
    host.log.warn(`chain-lower: story ${chainStoryId} threads no frames`);
    return null;
  }

  // Engine-computed pagination (all spreadsheet + threading semantics in
  // Rust). Hand it the chain's content boxes in order.
  const pages = engine.paginate(
    sheet,
    range,
    chain.map((c) => c.box),
    {
      repeatedHeaderRows: opts?.repeatedHeaderRows,
      continuedMarker: opts?.continuedMarker,
      keepRowsTogether: opts?.keepRowsTogether,
    },
  );

  // Lower each page into the frame it targets (page.frameIndex indexes the
  // chain we handed the engine).
  const tableIds: (string | null)[] = [];
  for (const page of pages) {
    const frame = chain[page.frameIndex];
    if (!frame) {
      // The engine returned a page for a frame index past the chain — should
      // not happen (paginate only fills supplied frames), but stay honest.
      tableIds.push(null);
      continue;
    }
    tableIds.push(await lowerPageToFrame(host, pageId, frame, page));
  }

  return { storyId: chainStoryId, chain, pages, tableIds };
}

/**
 * Subscribe to live re-pagination for a chain (Wave 2D, S-05; §8.5). Every
 * `host.document.onDidChange` event that carries `reflow` for a frame IN the
 * active chain re-runs `lowerPaginatedToChain` (a content-box resize changed
 * the available height — re-split). Events with NO `reflow` are the §8.5
 * transform case (move/scale/rotate is display-only) and are IGNORED — they
 * never re-paginate. Returns the subscription `Disposable`; the chain is
 * re-resolved on every reflow (a resize can add/remove fitting rows but the
 * topology read stays cheap).
 */
export function subscribeChainReflow(
  host: BundleHost,
  engine: SheetEngine,
  sheet: number,
  range: string,
  chainStoryId: string,
  opts?: {
    repeatedHeaderRows?: number;
    continuedMarker?: boolean;
    keepRowsTogether?: [number, number][];
  },
): Disposable {
  // Track the chain frame ids so we only react to reflow of OUR frames.
  let chainFrameIds = new Set<string>();
  void resolveChain(host, chainStoryId).then((chain) => {
    chainFrameIds = new Set(chain.map((c) => c.frameId));
  });

  return host.document.onDidChange((e) => {
    // §8.5: no reflow → a pure transform → DO NOT re-paginate.
    if (!e.reflow) return;
    // Only re-paginate when the resized frame belongs to this chain. If we
    // have not yet resolved the chain (the async prime is in flight), fall
    // through and re-paginate — re-resolving the chain is the source of truth.
    if (chainFrameIds.size > 0 && !chainFrameIds.has(e.reflow.frameId)) return;

    void (async () => {
      const result = await lowerPaginatedToChain(
        host,
        engine,
        sheet,
        range,
        chainStoryId,
        opts,
      );
      if (result) {
        chainFrameIds = new Set(result.chain.map((c) => c.frameId));
      }
    })();
  });
}
