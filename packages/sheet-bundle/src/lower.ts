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
  cellFillSwatchOps,
  defaultPlacement,
  joinText,
  lowerToMutations,
  makeBinding,
  pageTableMutations,
  tableCellOps,
  tableDecorOps,
  tableInsertOp,
  tableRefreshOps,
  cellCharacterStyleApplies,
  cellCharacterStyleMints,
  cellCharacterStyles,
  cellsNeedingRestyle,
  cellTextSwatchOps,
  type LoweredContent,
  type Page,
  type Placement,
} from "../../sheet-host-model/src";

import type { FrameBox, SheetEngine } from "./engine";
import { readKnownSwatchIds } from "./swatch-mints";

/**
 * The point size an un-styled lowered cell RENDERS at.
 *
 * Not a preference — a coupling. `styleProps` emits
 * `characterFontSize` only when the workbook's style carries a size, so a
 * cell without one is poured as a bare run and the engine sizes it from
 * the document default (`PipelineOptions::default_point_size`, 12 pt;
 * `[No paragraph style]` in a generated fixture agrees). Measuring at any
 * other number sizes the column for text nobody renders.
 *
 * It was 11 — Excel's Calibri default, the size the writer WISHED for and
 * never wrote. Every column came out 12/11 = 9% too narrow, which the
 * flat `CELL_INSET_PT` hid for every string under ~44 pt. Measured
 * through the real host door on the showcase's workbook: "Region"
 * measured 35.84 pt at 11 and renders 39.11 pt at 12 — 0.73 pt of the
 * inset left, so it fits; "Revenue" measured 44.91 and renders 49.02 in a
 * 48.91 pt column, overflowing by 0.11 pt, so the second column's own
 * header wrapped.
 */
export const DEFAULT_CELL_POINT_SIZE = 12;

/** Per-column width (pt) from the document's font metrics (S-13). For
 *  each column, measure the widest formatted cell text via the host
 *  shaper and add a small inset; fall back to the IR's char-based width
 *  when the shaper is unwired or yields nothing. Keeps the page table and
 *  any future grid view resolving to the SAME widths (the §8.3
 *  cross-surface-consistency requirement).
 *
 *  The measurement must ask for the face and size the POUR will actually
 *  use, or the width is a measurement of a different document. Both
 *  fallbacks below are that rule: `""` for the family (the engine's own
 *  default-face resolution, which is what a bare run gets) and
 *  {@link DEFAULT_CELL_POINT_SIZE} for the size. */
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
        style?.fontSizePt ?? DEFAULT_CELL_POINT_SIZE,
      );
      const measured = metrics.advance + CELL_INSET_PT;
      return measured > 0 ? measured : col.widthPt;
    }),
  );
}

/** The active page id (meta first, else the first page). Mirrors
 *  plugin-web's `activePageId`. */
async function activePageId(host: BundleHost): Promise<PageId | null> {
  const meta = await host.document.meta();
  if (meta.activePage) return meta.activePage;
  const pages = await host.document.collection<{ selfId: string }>("pages");
  return pages.length > 0 ? pages[0].selfId : null;
}

/** Where the current selection sits (Wave 4 — new frames land at the
 *  selection instead of a fixed 24 pt from the page origin): the page and
 *  page-local top-left of the FIRST selected element, its content box
 *  carried through its item transform. Null with nothing selected, or when
 *  the element is on the pasteboard (no page). */
export async function selectionAnchor(
  host: BundleHost,
): Promise<{ pageId: PageId; top: number; left: number } | null> {
  let selected: ElementId[];
  try {
    selected = host.selection.get();
  } catch {
    return null;
  }
  const first = selected.find(
    (id) => id.kind !== "storyRange" && id.kind !== "table" && id.kind !== "tableCell",
  );
  if (!first) return null;
  let geom: Awaited<ReturnType<BundleHost["document"]["elementGeometry"]>>;
  try {
    geom = await host.document.elementGeometry([first]);
  } catch {
    return null;
  }
  const g = geom[0];
  if (!g || !g.pageId) return null;
  const [top, left] = g.bounds;
  const [a, b, c, d, tx, ty] = g.itemTransform ?? [1, 0, 0, 1, 0, 0];
  return { pageId: g.pageId, left: a * left + c * top + tx, top: b * left + d * top + ty };
}

/** A placement for `content` at the current selection, or the default one
 *  on the active page when nothing usable is selected. */
export async function placementForContent(
  host: BundleHost,
  content: LoweredContent,
): Promise<Placement | null> {
  const at = await selectionAnchor(host);
  if (at) {
    const sized = defaultPlacement(at.pageId, content).bounds;
    const h = sized[2] - sized[0];
    const w = sized[3] - sized[1];
    return { pageId: at.pageId, bounds: [at.top, at.left, at.top + h, at.left + w] };
  }
  const pageId = await activePageId(host);
  return pageId ? defaultPlacement(pageId, content) : null;
}

/** The bare `table_id` STRING from a created Table ElementId. `insertTable`
 *  mints a Table addressed by `{ kind: "table", id: { story_id, table_id } }`
 *  (the wire ElementId::Table is a STRUCTURED id, not a string). The
 *  cell-addressing ops — `insertText.cell.tableId` and the `tableCell`
 *  ElementId — need the bare `table_id`. Reading `createdId.id` as a string
 *  (the old bug) nested the whole `{story_id, table_id}` object as `table_id`,
 *  so the engine rejected the cell pour with "invalid type: map, expected a
 *  string" and the table rendered BLANK. Narrow on `kind` to extract it. */
function tableIdOf(created: ElementId): string {
  if (created.kind === "table") {
    return created.id.table_id;
  }
  // Defensive: a host minting a bare-string table id (none does today).
  return created.id as unknown as string;
}

/** Raw frame id from a created ElementId (the hitTest filter / text ops
 *  key off the string id). */
function frameIdOf(id: ElementId): string | null {
  if (id.kind === "textFrame" || id.kind === "rectangle") {
    return id.id as string;
  }
  return null;
}

/** Phase 3 — pour a native table's cell content in the engine's TWO apply
 *  lanes, NOT one batch. The cell text is a TEXT edit (`insertText` with the
 *  `TextCellAddr` qualifier); the decor (merges → `setCellSpan`, style fills
 *  + grid rules → `tableCell`-scoped `setElementProperty`) is the FRAME /
 *  property lane. `Operation::Batch` carries only frame ops — text isn't an
 *  Operation — so combining them made the engine reject the WHOLE batch
 *  (`Mutation::Batch` NotImplemented), leaving the table BLANK. Pour each
 *  cell's text via its own mutate (text lane), then the decor as ONE batch
 *  (frame lane). The two-lane split costs the single-undo atomicity the
 *  combined batch had, but it is the only shape the engine applies. */
async function pourCellContent(
  host: BundleHost,
  content: LoweredContent,
  storyId: string,
  tableId: string,
): Promise<void> {
  // TEXT lane — one InsertText per non-empty cell (insertText can't ride an
  // Operation::Batch, so it can't be batched with the decor or each other).
  const pour = tableCellOps(content, storyId, tableId);
  const pourOps = pour.op === "batch" ? pour.args.ops : [pour];
  for (const op of pourOps) {
    const r = await host.document.mutate(op);
    if (!r.applied) host.log.warn("lower: cell text pour rejected", r);
  }
  // FRAME lane — the cell-fill swatch mints + the decor (spans + fills +
  // edge strokes) as ONE batch. The mints LEAD: a `cellFillColor` colorRef
  // names a swatch id, and a cell whose swatch does not exist is left
  // UNPAINTED by the renderer (verified by render, not by reading).
  const decor = tableDecorOps(content, storyId, tableId);
  if (decor.unmappedRules > 0) {
    host.log.warn(
      `lower: ${decor.unmappedRules} grid rule(s) aligned to no cell boundary ` +
        "(not drawn natively)",
    );
  }
  // The swatch READ only happens when there is something to mint (an
  // unstyled region — what `getRangeLowered` emits today — costs no extra
  // host round-trip). `readKnownSwatchIds` returns `null` when the read
  // FAILED, and `swatchMintOps` mints nothing for `null`: minting blind
  // risks a duplicate, and core's refusal of a duplicate `createSwatch`
  // fails the WHOLE batch, taking the fills and edge strokes with it. So a
  // failed read degrades the fills to unpainted (the pre-fix behaviour),
  // never to lost decor. Same ruling the ADR-023 Swatches provider makes
  // before an `editSwatch`.
  const wanted = cellFillSwatchOps(content);
  const mints =
    wanted.length === 0
      ? []
      : cellFillSwatchOps(content, await readKnownSwatchIds(host));
  const ops = [...mints, ...decor.ops];
  if (ops.length > 0) {
    const r = await host.document.mutate({ op: "batch", args: { ops } });
    if (!r.applied) host.log.warn("lower: cell decor batch rejected", r);
  }
  // The cells' text formatting (bold, size, face, colour) — the widths
  // were measured in it, so the pour now renders in it too.
  await styleCellText(host, content, storyId, tableId);
}

/** The document's character-style ids, or null when the read failed. */
async function readKnownCharacterStyleIds(
  host: BundleHost,
): Promise<Set<string> | null> {
  try {
    const rows = await host.document.collection<{ selfId: string }>("characterStyles");
    return new Set(rows.map((r) => r.selfId));
  } catch (err) {
    host.log.warn("cell text styles: character-style read failed", err);
    return null;
  }
}

/** Style the cells' TEXT (Wave 4): mint the character styles the content's
 *  cell formats need (+ the text-colour swatches they name), then apply one
 *  per cell (`only` = just these `row:col` cells, reset-to-none included —
 *  a refresh). Costs no read when no cell carries a character facet. */
async function styleCellText(
  host: BundleHost,
  content: LoweredContent,
  storyId: string,
  tableId: string,
  only?: ReadonlySet<string>,
): Promise<void> {
  if (cellCharacterStyles(content).size > 0) {
    const [styles, swatches] = await Promise.all([
      readKnownCharacterStyleIds(host),
      readKnownSwatchIds(host),
    ]);
    const mints = [
      ...cellTextSwatchOps(content, swatches),
      ...cellCharacterStyleMints(content, styles),
    ];
    if (mints.length > 0) {
      const r = await host.document.mutate({ op: "batch", args: { ops: mints } });
      if (!r.applied) host.log.warn("cell text styles: style mint rejected", r);
    }
  } else if (!only) {
    return;
  }
  const applies = cellCharacterStyleApplies(content, storyId, tableId, {
    cells: only,
    includeDefault: only !== undefined,
  });
  // One batch (text-lane ops batch since core v0.61).
  if (applies.length > 0) {
    const r = await host.document.mutate({ op: "batch", args: { ops: applies } });
    if (!r.applied) host.log.warn("cell text styles: applyStyle rejected", r);
  }
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
  /** The page the frame was placed on. */
  pageId?: PageId;
  /** What the table currently shows — the baseline a refresh diffs from
   *  (Wave 4: placed tables refresh in place after edits). */
  content?: LoweredContent;
  /** The column widths the table was sized with. */
  columnWidths?: number[];
  /** The workbook content version this table reflects (also in its
   *  binding metadata). */
  contentVersion?: number;
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
  /** Where the frame lands (Wave 4: the session places at the current
   *  selection); default `defaultPlacement` on the active page. */
  placement?: Placement;
  /** The workbook content version recorded in the binding (default 0). */
  contentVersion?: number;
  /** The page lowering of the range when the caller already has it (the
   *  session lowers once to size the placement) — saves a second engine
   *  call. */
  content?: LoweredContent;
}

/** The PAGE lowering of a range: real styles + conditional formatting
 *  (`getRangePage`, Wave 4), falling back to the frozen key-0 door on an
 *  engine that predates it. */
export function pageContent(
  engine: SheetEngine,
  sheet: number,
  range: string,
): LoweredContent {
  return engine.getRangePage
    ? engine.getRangePage(sheet, range, { includeGridRules: true })
    : engine.getRangeLowered(sheet, range, { includeGridRules: true });
}

/**
 * Lower `sheet`/`range` to a fresh page frame. Engine computes the IR;
 * the translators (pure) shape the mutations; this drives the host
 * writes. Returns the created frame's raw id, or null on any failure
 * (mutate-never-throws: outcomes are checked, not caught).
 */
/** Snapshot the document's story ids (the `stories` collection). */
export async function storyIdsSnapshot(host: BundleHost): Promise<Set<string>> {
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
  const pageId = opts?.placement?.pageId ?? (await activePageId(host));
  if (!pageId) {
    host.log.warn("lower: no page to place the sheet frame into");
    return null;
  }

  // Engine-computed IR (all spreadsheet semantics in Rust): the PAGE door —
  // the workbook's real fills/borders with conditional formatting folded on
  // top (Wave 4; until then this read the frozen key-0 door and every
  // placed table came out unstyled).
  const content = opts?.content ?? pageContent(engine, sheet, range);
  const sheetInfo = engine.listSheets().find((s) => s.id === sheet);
  const sheetName = sheetInfo ? sheetInfo.name : String(sheet);

  const placement = opts?.placement ?? defaultPlacement(pageId, content);
  // The session's workbook revision (Wave 4) — a refresh rewrites it.
  const contentVersion = opts?.contentVersion ?? 0;
  const binding = makeBinding(sheetName, range, contentVersion);

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
  const tableId = tableIdOf(tableOutcome.createdId);

  // S-04 — report the resolved native table so the session can address its
  // cells for "new style from cell". Only the native-table lane reports
  // (the tab-text fallback has no table to address).
  opts?.onLowered?.({
    frameId,
    storyId,
    tableId,
    sheet,
    range,
    pageId,
    content,
    columnWidths,
    contentVersion,
  });

  // Phase 3 — pour the cell text (TEXT lane) then the decor (FRAME lane);
  // two lanes, never one batch (see pourCellContent).
  await pourCellContent(host, content, storyId, tableId);

  await host.selection.set([outcome.createdId]);
  return frameId;
}

/** The retained tab-text lane (spec §2.2 degradation): the pure
 *  `lowerToMutations` batch (frame + drawn rules + binding), then the
 *  tab/newline text pour into the resolved story.
 *
 *  The data-bar swatch mints ride INSIDE that phase-1 batch, so the
 *  document's swatches are read first and an already-present colour is
 *  referenced rather than re-created: a duplicate `createSwatch` fails the
 *  whole batch, which here would cost the FRAME, the rules and the binding
 *  — not just a bar's colour. The read is skipped when the region carries
 *  no bars (today's `getRangeLowered` never emits any — Rust's
 *  `lower_range` is not the condfmt lowering — so this normally costs no
 *  extra host round-trip). */
async function lowerTabTextToFrame(
  host: BundleHost,
  content: LoweredContent,
  placement: { pageId: PageId; bounds: [number, number, number, number] },
  binding: ReturnType<typeof makeBinding>,
): Promise<string | null> {
  const known =
    (content.databars ?? []).length === 0
      ? undefined
      : await readKnownSwatchIds(host);
  const { batch, text } = lowerToMutations(content, placement, binding, known);

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
export interface ChainFrame {
  frameId: string;
  box: FrameBox;
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

/** The story a frame belongs to (Wave 4). There is no frame→story read
 *  door (the hitTest door answers `storyId: null` for an empty frame), so
 *  this walks the stories and asks each for its frame chain — the chain
 *  that contains `frameId` names the story. One `frameChain` read per story;
 *  null when no story threads the frame. */
export async function storyOfFrame(
  host: BundleHost,
  frameId: string,
): Promise<string | null> {
  const stories = await host.document.collection<{ selfId: string }>("stories");
  for (const s of stories) {
    const links = await host.document.frameChain(s.selfId);
    if (links.some((l) => l.frameId === frameId)) return s.selfId;
  }
  return null;
}

/** Apply a placed table's in-place refresh (`tableRefreshOps`) in the
 *  engine's apply lanes: the structure ops as one batch, the changed cells'
 *  text one op each (the text lane), then the decor batch led by the fill
 *  swatches it names. Returns the number of `mutate` calls. */
async function applyTableRefresh(
  host: BundleHost,
  storyId: string,
  tableId: string,
  prev: LoweredContent,
  prevWidths: readonly number[],
  next: LoweredContent,
  nextWidths: readonly number[],
): Promise<number> {
  // Nothing moved: no write at all (a re-pagination that lands on the same
  // split, an edit outside this table's range).
  if (
    JSON.stringify(prev) === JSON.stringify(next) &&
    JSON.stringify(prevWidths) === JSON.stringify(nextWidths)
  ) {
    return 0;
  }
  const ops = tableRefreshOps(prev, next, storyId, tableId, prevWidths, nextWidths);
  let calls = 0;
  if (ops.structure.length > 0) {
    calls += 1;
    const r = await host.document.mutate({
      op: "batch",
      args: { ops: ops.structure },
    });
    if (!r.applied) host.log.warn("refresh: table reshape rejected", r);
  }
  // The changed cells' text as ONE batch (core applies text ops inside a
  // Mutation batch as one undo step since v0.61).
  if (ops.text.length > 0) {
    calls += 1;
    const r = await host.document.mutate({ op: "batch", args: { ops: ops.text } });
    if (!r.applied) host.log.warn("refresh: cell text rejected", r);
  }
  const wanted = cellFillSwatchOps(next);
  const mints =
    wanted.length === 0
      ? []
      : cellFillSwatchOps(next, await readKnownSwatchIds(host));
  const decor = [...mints, ...ops.decor];
  if (decor.length > 0) {
    calls += 1;
    const r = await host.document.mutate({ op: "batch", args: { ops: decor } });
    if (!r.applied) host.log.warn("refresh: cell decor rejected", r);
  }
  const restyle = cellsNeedingRestyle(prev, next);
  if (restyle.size > 0) await styleCellText(host, next, storyId, tableId, restyle);
  return calls;
}

/**
 * Refresh a placed native table IN PLACE to `next` (Wave 4 — placed tables
 * follow edits). Diffs against the table's recorded content: only changed
 * cells are re-poured, rows/columns reshaped at the tail, removed fills and
 * edges reset — the table is never duplicated. Re-stamps the frame's binding
 * with `contentVersion`. Returns the updated record (the new baseline).
 */
export async function refreshLoweredTable(
  host: BundleHost,
  info: LoweredTableInfo,
  next: LoweredContent,
  contentVersion: number,
  sheetName: string,
): Promise<LoweredTableInfo> {
  const prev = info.content;
  const prevWidths = info.columnWidths ?? [];
  const nextWidths = await measureColumnWidths(host, next);
  if (prev) {
    await applyTableRefresh(
      host,
      info.storyId,
      info.tableId,
      prev,
      prevWidths,
      next,
      nextWidths,
    );
  }
  const r = await host.document.mutate({
    op: "setPluginMetadata",
    args: {
      elementId: { kind: "textFrame", id: info.frameId },
      key: BINDING_KEY,
      value: JSON.stringify(makeBinding(sheetName, info.range, contentVersion)),
    },
  });
  if (!r.applied) host.log.warn("refresh: binding re-stamp rejected", r);
  return { ...info, content: next, columnWidths: nextWidths, contentVersion };
}

/** One table a chain placement owns: the table id and what it shows. A
 *  `blank` table belongs to a frame the last pagination did not need (the
 *  wire has no table-delete op, so it is emptied, kept, and reused when the
 *  range needs more frames again). */
export interface ChainTable {
  tableId: string;
  content: LoweredContent;
  widths: number[];
  blank: boolean;
}

/** The result of a chain pagination pass. */
export interface ChainLowerResult {
  /** The story whose chain was paginated. */
  storyId: string;
  /** The resolved chain frames (in order). */
  chain: ChainFrame[];
  /** The pages the engine produced (one per filled frame). */
  pages: Page[];
  /** The tableId lowered into each page's frame (null where the table was
   *  rejected). */
  tableIds: (string | null)[];
  /** Every table this placement owns, in story order (the baseline the
   *  next pass refreshes from — see `subscribeChainReflow`'s `from`). */
  tables: ChainTable[];
}

/** Pagination options (forwarded to the engine). */
export interface ChainOptions {
  repeatedHeaderRows?: number;
  continuedMarker?: boolean;
  keepRowsTogether?: [number, number][];
}

/** The content an unused chain table is reshaped to: one empty row. */
function blankContent(like: LoweredContent): LoweredContent {
  return {
    cols: like.cols,
    rows: [{ index: 0, heightPt: 1, cells: [] }],
    rules: { h: [], v: [] },
    merges: [],
    styles: like.styles,
  };
}

/**
 * One pagination pass over a chain (Wave 2D / S-05; Wave 4 replace). Asks
 * the engine to paginate the range into the chain's content boxes (all
 * threading math in Rust), then for page i: REFRESHES the placement's i-th
 * table in place when it has one, else appends a new table (insertTable
 * appends to the story, so story order = page order). Tables the pass no
 * longer needs are emptied (no delete op on the wire). All pages go into the
 * CHAIN'S story — threaded frames share one story.
 */
async function paginatePass(
  host: BundleHost,
  engine: SheetEngine,
  sheet: number,
  range: string,
  storyId: string,
  chain: ChainFrame[],
  owned: ChainTable[],
  opts?: ChainOptions,
): Promise<ChainLowerResult> {
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
  const tables: ChainTable[] = [];
  const tableIds: (string | null)[] = [];
  for (const [i, page] of pages.entries()) {
    const widths = await measureColumnWidths(host, page.content);
    const prev = owned[i];
    if (prev) {
      await applyTableRefresh(
        host,
        storyId,
        prev.tableId,
        prev.content,
        prev.widths,
        page.content,
        widths,
      );
      tables.push({ tableId: prev.tableId, content: page.content, widths, blank: false });
      tableIds.push(prev.tableId);
      continue;
    }
    const ops = pageTableMutations(page, storyId, widths);
    const outcome = await host.document.mutate(ops.insert);
    if (!outcome.applied || !outcome.createdId) {
      host.log.warn("chain-lower: insertTable rejected", outcome);
      tableIds.push(null);
      continue;
    }
    const tableId = tableIdOf(outcome.createdId);
    await pourCellContent(host, page.content, storyId, tableId);
    tables.push({ tableId, content: page.content, widths, blank: false });
    tableIds.push(tableId);
  }
  // Tables the range no longer needs: emptied, kept for reuse.
  const spare = owned.slice(pages.length);
  if (spare.some((t) => !t.blank)) {
    host.log.warn(
      `chain-lower: ${spare.length} table(s) no longer needed were emptied — ` +
        "the wire has no table-delete op (RFI candidate deleteTable)",
    );
  }
  for (const t of spare) {
    if (t.blank) {
      tables.push(t);
      continue;
    }
    const blank = blankContent(t.content);
    await applyTableRefresh(host, storyId, t.tableId, t.content, t.widths, blank, t.widths);
    tables.push({ ...t, content: blank, blank: true });
  }
  return { storyId, chain, pages, tableIds, tables };
}

/**
 * Lower `sheet`/`range` ACROSS a host frame chain with live pagination
 * (Wave 2D, RFI C-2 / S-05; spec §8.2). Reads the real chain via
 * `host.document.frameChain(storyId)`, resolves each frame's content box via
 * `host.document.elementGeometry`, asks the engine to paginate the range into
 * those boxes (all threading math in Rust), and lowers each resulting `Page`
 * as a native table into the chain's story. Returns the pass result, or null
 * when no chain resolves.
 *
 * `chainStoryId` selects the chain (the story the threaded frames share).
 * The caller may supply a ready `chain` (the frames + boxes) to bypass the
 * host reads — same downstream lowering.
 */
export async function lowerPaginatedToChain(
  host: BundleHost,
  engine: SheetEngine,
  sheet: number,
  range: string,
  chainStoryId: string,
  opts?: ChainOptions & { chain?: ChainFrame[] },
): Promise<ChainLowerResult | null> {
  const chain = opts?.chain ?? (await resolveChain(host, chainStoryId));
  if (chain.length === 0) {
    host.log.warn(`chain-lower: story ${chainStoryId} threads no frames`);
    return null;
  }
  return paginatePass(host, engine, sheet, range, chainStoryId, chain, [], opts);
}

/** How long a burst of reflow events settles before ONE re-pagination. */
export const CHAIN_REFLOW_DEBOUNCE_MS = 150;

/** A live chain placement (Wave 4): re-paginates on reflow (debounced) and
 *  on demand (`refresh()`, after workbook edits), always REPLACING the
 *  placement's own tables rather than adding new ones. */
export interface ChainSubscription extends Disposable {
  /** Re-paginate now (serialised behind any pass in flight). */
  refresh(): Promise<ChainLowerResult | null>;
  /** The latest pass (null before the first). */
  current(): ChainLowerResult | null;
  /** Resolves when the pass in flight (if any) has finished. */
  idle(): Promise<ChainLowerResult | null>;
}

/**
 * Subscribe to live re-pagination for a chain (Wave 2D, S-05; §8.5). Every
 * `host.document.onDidChange` event that carries `reflow` for a frame IN the
 * chain schedules a re-pagination, DEBOUNCED (`debounceMs`) so a resize drag
 * paginates once at its end. Events with NO `reflow` are the §8.5 transform
 * case (move/scale/rotate is display-only) and are IGNORED. Each pass
 * refreshes the tables of the previous one in place (pass `from` — the
 * `lowerPaginatedToChain` result — so the first reflow replaces what that
 * placed instead of adding to it).
 */
export function subscribeChainReflow(
  host: BundleHost,
  engine: SheetEngine,
  sheet: number,
  range: string,
  chainStoryId: string,
  opts?: ChainOptions & { from?: ChainLowerResult | null; debounceMs?: number },
): ChainSubscription {
  let last: ChainLowerResult | null = opts?.from ?? null;
  let chainFrameIds = new Set<string>(last?.chain.map((c) => c.frameId) ?? []);
  if (chainFrameIds.size === 0) {
    void resolveChain(host, chainStoryId).then((chain) => {
      if (chainFrameIds.size === 0) chainFrameIds = new Set(chain.map((c) => c.frameId));
    });
  }
  let timer: ReturnType<typeof setTimeout> | null = null;
  let inflight: Promise<ChainLowerResult | null> = Promise.resolve(last);
  let disposed = false;

  const run = (): Promise<ChainLowerResult | null> => {
    inflight = inflight.then(async () => {
      if (disposed) return last;
      const chain = await resolveChain(host, chainStoryId);
      if (chain.length === 0) return last;
      last = await paginatePass(
        host,
        engine,
        sheet,
        range,
        chainStoryId,
        chain,
        last?.tables ?? [],
        opts,
      );
      chainFrameIds = new Set(last.chain.map((c) => c.frameId));
      return last;
    });
    return inflight;
  };

  const sub = host.document.onDidChange((e) => {
    // §8.5: no reflow → a pure transform → DO NOT re-paginate.
    if (!e.reflow) return;
    if (chainFrameIds.size > 0 && !chainFrameIds.has(e.reflow.frameId)) return;
    if (timer !== null) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = null;
      void run();
    }, opts?.debounceMs ?? CHAIN_REFLOW_DEBOUNCE_MS);
  });

  return {
    refresh: run,
    current: () => last,
    idle: () => inflight,
    dispose() {
      disposed = true;
      if (timer !== null) clearTimeout(timer);
      timer = null;
      sub.dispose();
    },
  };
}
