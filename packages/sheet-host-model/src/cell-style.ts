// "New cell style from selection" — PURE translation (S-04 cell-style
// consumer). Composes from existing platform doors (RFI verdict): given a
// lowered table cell's current properties (read via B-19
// `host.document.elementProperties`), build the mutations that MINT a cell
// style (`createCellStyle`) and POPULATE it (`setStyleProperty`,
// collection "cell") from that cell's appearance.
//
// ZERO spreadsheet semantics (CLAUDE.md hard rule): this only re-shapes
// already-decided document properties into style-collection writes. The
// engine owns the cell appearance; the platform owns style minting.
//
// HONESTY (the residual the RFI names, verified against the plugin-api
// contract — wire.d.ts CellStyleSummary): APPLYING the minted style back to
// the cell via `appliedCellStyle` is "wire-shape-only (UnsupportedProperty
// until the Table NodeId surface lands)". So this module mints + populates
// (which DO land); the caller attempts the apply and reports the platform's
// rejection rather than pretending it took.

import type { ElementId, Mutation, PropertyPath, Value } from "@paged-media/plugin-api";

/** A property read off a cell that we carry into a new cell style. The
 *  wire's `PropertyEntry` is `{ path, value? }`; we narrow to the cell-
 *  appearance paths a publishing-grade cell style cares about (fill +
 *  per-edge strokes + insets + vertical justification). */
const CELL_STYLE_PATHS: ReadonlySet<PropertyPath> = new Set<PropertyPath>([
  "cellFillColor",
  "cellFillTint",
  "cellInsetTop",
  "cellInsetLeft",
  "cellInsetBottom",
  "cellInsetRight",
  "cellVerticalJustification",
  "cellTopEdgeStrokeColor",
  "cellTopEdgeStrokeWeight",
  "cellTopEdgeStrokeTint",
  "cellBottomEdgeStrokeColor",
  "cellBottomEdgeStrokeWeight",
  "cellBottomEdgeStrokeTint",
  "cellLeftEdgeStrokeColor",
  "cellLeftEdgeStrokeWeight",
  "cellLeftEdgeStrokeTint",
  "cellRightEdgeStrokeColor",
  "cellRightEdgeStrokeWeight",
  "cellRightEdgeStrokeTint",
]);

/** Is `path` a cell-appearance property a new cell style should capture? */
export function isCellStylePath(path: PropertyPath): boolean {
  return CELL_STYLE_PATHS.has(path);
}

/** One property entry as read from `elementProperties` (the wire shape,
 *  narrowed). `value` is absent/null for indeterminate reads — those are
 *  skipped (we never bake an em-dash placeholder into a style). */
export interface ReadEntry {
  path: PropertyPath;
  value?: Value | null;
}

/** The mutations to mint + populate a cell style from a cell's read
 *  properties. `createOp` mints (with our own `selfId` — the null-createdId
 *  precedent so the caller never needs `createdId` back); `propertyOps`
 *  populate it. `capturedPaths` lists what was carried (for the honest UI
 *  count). The `applyOp` is the APPLY-BACK the caller attempts separately —
 *  it is wire-shape-only today (see the header), so it is offered, not
 *  bundled into the mint batch. */
export interface CellStylePlan {
  styleId: string;
  createOp: Mutation;
  propertyOps: Mutation[];
  capturedPaths: PropertyPath[];
  applyOp: (cellId: ElementId) => Mutation;
}

/** Build the mint + populate plan for a cell style named `name` with id
 *  `styleId` (caller-minted — `createCellStyle` may return `createdId:null`
 *  for collection creates, so we supply `selfId` and reference it, the draw
 *  gradient precedent). Only the cell-appearance entries with a concrete
 *  value are carried; indeterminate (`value` null/absent) and non-cell paths
 *  are skipped. PURE — no host, no IO. */
export function planCellStyleFromEntries(
  styleId: string,
  name: string,
  entries: readonly ReadEntry[],
): CellStylePlan {
  const createOp: Mutation = {
    op: "createCellStyle",
    args: { selfId: styleId, name },
  };

  const propertyOps: Mutation[] = [];
  const capturedPaths: PropertyPath[] = [];
  for (const e of entries) {
    if (!isCellStylePath(e.path)) continue;
    if (e.value === undefined || e.value === null) continue; // indeterminate
    propertyOps.push({
      op: "setStyleProperty",
      args: { collection: "cell", styleId, path: e.path, value: e.value },
    });
    capturedPaths.push(e.path);
  }

  return {
    styleId,
    createOp,
    propertyOps,
    capturedPaths,
    applyOp: (cellId: ElementId): Mutation => ({
      op: "setElementProperty",
      args: {
        elementId: cellId,
        path: "appliedCellStyle",
        // A style reference is carried as a `text` Value (the style's
        // selfId) — the same shape applied-style writes use elsewhere.
        value: { type: "text", value: styleId },
      },
    }),
  };
}
