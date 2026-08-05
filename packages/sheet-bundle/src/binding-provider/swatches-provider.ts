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

// ADR 023 phase D — paged.sheet as the SWATCHES binding provider: the
// DOCUMENT-SCOPED proof consumer, the axis neither Layers nor
// Character/Paragraph touches.
//
// WHY COLOUR IS A DIFFERENT SHAPE, not a second Layers.
//
//   · Layers is an element COLLECTION addressed by row identity, and a
//     row's editable state IS core `PropertyPath`s (`layerName`,
//     `layerVisible`, `layerLocked`).
//   · Character/Paragraph is SCALAR paths over a RANGE, whose value can
//     be MIXED.
//   · Swatches is a DOCUMENT-SCOPED RESOURCE the panel edits DIRECTLY.
//     There is no selection to address — `readCollection` takes no
//     target BY CONSTRUCTION — and core models a swatch's whole mutable
//     surface as STRUCTURAL OPS (`createSwatch` / `editSwatch` /
//     `deleteSwatch`, each carrying a complete `SwatchSpec`). The
//     `PropertyPath` union has no `swatchName` and no swatch colour.
//
// That last fact is why this provider declares NO `paths` and implements
// NO `readProperty`: there is nothing in core's path vocabulary to
// declare. A provider that serves only the document-scoped lanes is
// exactly what the contract's "every lane is OPTIONAL" is for.
//
// WHAT PAGED.SHEET SERVES, and why it is genuinely its own.
//
// While you are inside a lowered sheet frame, the host Swatches panel
// shows THE WORKBOOK PALETTE: the colours this spreadsheet actually
// uses, as real document swatches at the deterministic ids the lowering
// already mints (`Color/uPagedSheetChart<HEX>`,
// `Color/uPagedSheetDataBar<HEX>` — see `sheet-host-model/palette.ts`).
// This is not a second rendering of the document's swatch list; it is a
// different list, which is what makes the retarget falsifiable rather
// than decorative.
//
// It is also not aspirational. ADR 023 chose colour as the third proof
// BECAUSE paged.sheet was already minting document swatches from
// production code with no panel to drive them from
// (`lower-to-mutations.ts` for data bars, `chart.ts` for chart series).
// This is that work getting its surface.
//
// WHAT IT DOES NOT SERVE — the honest half, stated rather than
// discovered:
//
//   · CELL TEXT colour (`LoweredStyle.textRgb`). CELL FILL used to be
//     listed here too, for a reason a render later confirmed: both
//     lowering lanes passed the raw hex into a `{type:"colorRef"}` value
//     and core resolves a colorRef by SWATCH ID (`Graphic::resolve`), so
//     `#FFFF00` named nothing — a `cellFillColor` over a raw hex paints
//     ZERO pixels, a `characterFillColor` over one renders the DEFAULT
//     colour. The lowering now mints a real swatch per distinct cell
//     colour, so cell FILL is served (it is in `workbookPalette`). Cell
//     TEXT still is not: `cellTextSwatchOps` builds its mints but no
//     driver applies `StyleEmission` yet, so a chip would still name a
//     swatch no document has. It joins when a caller applies them.
//   · `createSwatch` — "a new colour" is not a workbook concept: a
//     colour enters a workbook palette by being USED, not by being
//     declared. Undeclared, so the host DISABLES the panel's "+ New"
//     rather than quietly writing into the document's list while the
//     panel is showing this one.
//   · `deleteSwatch` — the same, harder: a chart series colour cannot be
//     removed from a colour panel, and deleting the DOCUMENT swatch
//     while the workbook still references it would leave the page
//     showing a colour the panel says is gone.
//   · `importSwatchLibrary` / `editColorGroup` — document-library and
//     document-grouping verbs, undeclared for the same reason.
//
// WHAT IT DOES SERVE FOR WRITES: `editSwatch`, and it is the one verb
// that means something here — recolour or rename a workbook colour. The
// provider MINTS ON FIRST EDIT: a palette entry the document does not
// carry yet is created at its deterministic id (core's `createSwatch`
// honours `spec.selfId` and REFUSES a duplicate, so the existence check
// is load-bearing, not defensive noise), and one it already carries is
// edited. Either way the write lands through `host.document.mutate` —
// the undo rule the contract states and cannot enforce.
//
// NOTE ON WHAT THE SEAM COULD NOT CARRY, recorded here because this is
// where it was found: `SwatchSummary` is `{selfId, name, kind}` and
// carries NO colour, while the host's chip is resolved by a separate
// core RPC (`client.colorPreview(selfId)`) that the binding-provider
// contract has no lane for. So a palette entry the document has not yet
// minted has no resolvable chip until it is edited once. The host marks
// that state honestly (`data-swatch-preview="unresolved"`) instead of
// painting a plausible grey.

import type { BundleHost, Mutation, SwatchSummary } from "@paged-media/plugin-api";
import {
  paletteEntryToSpec,
  paletteEntryToSummary,
  type PaletteEntry,
} from "../../../sheet-host-model/src";

import type {
  BindingCollection,
  BindingProvider,
  BindingWrite,
} from "./adr023-seam";

/** What the provider needs from the workbook session — narrowed to one
 *  method so the provider is unit-testable without an engine, and so the
 *  session stays the only thing that knows how to talk to wasm. */
export interface WorkbookPaletteSource {
  /** Every colour paged.sheet mints as a document swatch, deduped and
   *  deterministic. Empty when no workbook is loaded. */
  workbookPalette(): readonly PaletteEntry[];
}

/** The ONE op this provider takes first refusal on. Everything else the
 *  host Swatches panel can send stays undeclared, which is the
 *  contract's own way to make the host disable a control rather than let
 *  it write somewhere the user is not looking. */
export const SERVED_OPS: readonly string[] = ["editSwatch"];

export interface SwatchesBindingProvider {
  provider: BindingProvider;
  /** The rows last served — exposed for conformance, not for the host. */
  rows(): readonly SwatchSummary[];
}

/**
 * Build the Swatches provider for the `sheet` edit context.
 *
 * LIFETIME IS BORROWED, not managed here: phase A's adapter wraps the
 * edit context's own `onEnter`/`onExit`, so the shell's context stack is
 * the single source of "who is active". There is deliberately no
 * `enter`/`exit` on this provider — unlike the Layers one, it needs no
 * scope root, because a document resource is not addressed relative to
 * the element you entered on. That absence IS the scope axis.
 */
export function makeSwatchesBindingProvider(
  host: BundleHost,
  source: WorkbookPaletteSource,
): SwatchesBindingProvider {
  /** Palette id → entry for the rows last served. The host treats a row
   *  id as opaque; this map is where its vocabulary comes back to ours. */
  let byId = new Map<string, PaletteEntry>();
  let served: SwatchSummary[] = [];

  const buildRows = (): SwatchSummary[] | null => {
    let palette: readonly PaletteEntry[];
    try {
      palette = source.workbookPalette();
    } catch (err) {
      host.log.warn("swatches provider: palette read failed", err);
      return null;
    }
    if (palette.length === 0) return null;
    byId = new Map(palette.map((e) => [e.selfId, e]));
    served = palette.map(paletteEntryToSummary);
    return served;
  };

  /** Is `swatchId` already a document swatch? Answers `null` when the
   *  read itself failed, which is NOT the same as "no" — a failed read
   *  must not cause a create that core would then refuse as a duplicate. */
  const documentHas = async (swatchId: string): Promise<boolean | null> => {
    try {
      const rows =
        await host.document.collection<SwatchSummary>("swatches");
      return rows.some((s) => s.selfId === swatchId);
    } catch (err) {
      host.log.warn("swatches provider: document swatch read failed", err);
      return null;
    }
  };

  const provider: BindingProvider = {
    provides: {
      // No `paths`: core models no swatch PropertyPath, so there is
      // nothing honest to declare. See the module header.
      collections: ["swatches"],
      ops: SERVED_OPS,
    },

    readCollection(request): BindingCollection {
      if (request.collection !== "swatches") {
        return { kind: "decline", reason: "only the swatches collection" };
      }
      const rows = buildRows();
      if (rows === null) {
        // A DECLINE, not empty rows — the `planarRegions` lesson. Here
        // it decides whether the user sees the DOCUMENT's swatches or an
        // empty colour panel, and a workbook with no palette (no charts,
        // no data bars) has nothing to say about colour, so core is the
        // truthful answer.
        return {
          kind: "decline",
          reason: "this workbook mints no document swatches",
        };
      }
      return { kind: "rows", rows };
    },

    async applyMutation(mutation): Promise<BindingWrite> {
      const m = mutation as unknown as {
        op: string;
        args?: Record<string, unknown>;
      };
      if (m.op !== "editSwatch") {
        // Undeclared ops never reach here (the registry gates on
        // `provides.ops`); this is the belt to that braces.
        return { kind: "decline", reason: `unhandled op "${m.op}"` };
      }
      const swatchId = m.args?.swatchId;
      if (typeof swatchId !== "string") {
        return { kind: "decline", reason: "editSwatch without a swatchId" };
      }
      const entry = byId.get(swatchId);
      if (!entry) {
        // Not one of our rows — let it go to core untouched. A DECLINE,
        // because core may well own this swatch.
        return {
          kind: "decline",
          reason: `"${swatchId}" is not in the workbook palette`,
        };
      }
      const spec = m.args?.spec;
      if (typeof spec !== "object" || spec === null) {
        return { kind: "decline", reason: "editSwatch without a spec" };
      }
      const exists = await documentHas(swatchId);
      if (exists === null) {
        // We OWNED this op and could not honour it. That is
        // `applied:false` with the reason, never a decline — a decline
        // would send the same op to core behind our back.
        return {
          kind: "applied",
          outcome: {
            applied: false,
            error: "could not read the document's swatches",
          },
        };
      }
      // MINT ON FIRST EDIT. The palette entry is a real document swatch
      // id by construction; the first edit is what puts it in the
      // document. `createSwatch` honours `spec.selfId` and refuses a
      // duplicate, so the branch is required, not cosmetic.
      const write: Mutation = exists
        ? ({ op: "editSwatch", args: { swatchId, spec } } as Mutation)
        : ({
            op: "createSwatch",
            // The panel's own spec, with the palette's id pinned on: the
            // panel already carries `selfId`, and pinning it again means
            // a spec that somehow lost it still mints at the right id.
            args: { spec: { ...paletteEntryToSpec(entry), ...spec } },
          } as Mutation);
      const outcome = await host.document.mutate(write);
      return { kind: "applied", outcome };
    },
  };

  return { provider, rows: () => served };
}
