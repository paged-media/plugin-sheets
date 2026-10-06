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

// The paged.sheet bundle entry. T0 scope (the honest slice): import an
// XLSX into an in-memory engine, pick a sheet + range, and LOWER it to a
// page frame as a NATIVE Paged <Table> (S-03 RESOLVED — insertTable +
// cell pour + spans + cell strokes/fills; the spec §2.2 tab-text
// degradation is retained as the explicit fallback lane), bound via
// plugin metadata. Sheets mode (S-01) and persistence (S-08) remain
// honest gaps — the panel says so.
//
// Wiring mirrors plugin-draw/plugin-web: contributePanel for the workbook
// panel + the two commands (importXlsx opens the panel; lowerToFrame runs
// the session lower). The host tracks every registration; the session is
// the one thing allocated OUTSIDE a facade-tracked registration, so
// dispose tears it down.

import type { BundleHandle, BundleHost, EditContextContribution } from "@paged-media/plugin-api";
import type { ContentWheelEvent, ContentWheelHook } from "./protocol66";
import { contributeMenu } from "./menu";
import { contributePanel } from "@paged-media/plugin-sdk";
import { parseBinding } from "../../sheet-host-model/src";

import manifest from "../manifest.json";

import {
  DELIMITED_MIMES,
  importBytes,
  pickAndImport,
  XLSX_MIME,
} from "./import-xlsx";
import {
  registerBindingProvider,
  type BindingProviderHandle,
} from "./binding-provider/adr023-seam";
import { makeSwatchesBindingProvider } from "./binding-provider/swatches-provider";
import { subscribeProviderInvalidation } from "./binding-provider/invalidation";
import { makeTextBindingProvider } from "./binding-provider/text-provider";
import { createWorkbookSession } from "./session";
import { makeWorkbookPanel } from "./panels/workbook-panel";
import { makeGridPanel } from "./panels/grid-panel";
import { makeDatasetsPanel } from "./panels/datasets-panel";

const PANEL_ID = "media.paged.sheet.panel.workbook";
const GRID_PANEL_ID = "media.paged.sheet.panel.grid";
const DATASETS_PANEL_ID = "media.paged.sheet.panel.datasets";

/** The raw id string of a frame-like `ElementId` (textFrame / rectangle
 *  carry a string `id`), or null. Structural so it needs no wire import. */
/** Whether keyboard focus is in a text field (keybindings stand down). */
function editableFocus(): boolean {
  const el = (globalThis as { document?: { activeElement?: unknown } }).document
    ?.activeElement as { tagName?: string; isContentEditable?: boolean } | null | undefined;
  if (!el) return false;
  return el.tagName === "INPUT" || el.tagName === "TEXTAREA" || !!el.isContentEditable;
}

function frameIdOf(id: unknown): string | null {
  if (typeof id === "object" && id !== null) {
    const e = id as { id?: unknown };
    if (typeof e.id === "string") return e.id;
  }
  return null;
}

/**
 * ADR 024 — the commands that need a workbook say so.
 *
 * Gated on THIS PLUGIN'S OWN STATE rather than on the host's active
 * edit context, deliberately. "A workbook is open" is the condition
 * that actually decides whether the verb can do anything; the context
 * is a proxy for it and a worse one, since the panel lane works on a
 * selected sheet frame without entering the frame at all. It also
 * needs no duck-typing of a host shape this bundle cannot import.
 *
 * The predicate closes over `session`, which is created below — safe
 * because `when` is only ever evaluated at invoke/render time, never
 * at registration.
 */
export function activate(host: BundleHost): BundleHandle {
  const session = createWorkbookSession(host, {
    // Cmd+F on the grid: the find field lives in the grid panel.
    onFindRequest: () => host.shell.openPanel(GRID_PANEL_ID),
  });
  /** See the note above `activate`: a workbook must be open for the
   *  clipboard verbs to have anything to act on. */
  const workbookIsOpen = () => session.state().engine !== null;
  // ADR 023 — the binding-provider handle is the SECOND thing allocated
  // outside a facade-tracked registration (the session is the first), so
  // dispose tears it down explicitly. `null` on a host with no registry.
  let swatchesProviderHandle: BindingProviderHandle | null = null;
  // ADR 023 phase D — the VALUE-axis provider: the host's Character and
  // Paragraph panels, answered from the workbook's cell styles while the
  // `sheet` context is active.
  let textProviderHandle: BindingProviderHandle | null = null;
  // The re-read signal. `host.document.onDidChange` fires on ENGINE
  // mutations, and picking a different CELL is not one — it moves no
  // engine page. Without this the host panels go quietly stale on the
  // most common interaction there is, which is the class of lie this
  // platform refuses. Coarse by design: "re-read", not a per-path diff.
  let providerInvalidateSub: { dispose(): void } | null = null;

  // S-08: restore the last persisted workbook from host.blob, if any. A
  // cheap no-op (one blob read) when nothing was persisted or no blob
  // store is wired — the engine boots only when there are bytes to load.
  void session.restore();
  // The workbook belongs to the DOCUMENT, and activation runs at app boot —
  // before any document is open. Every later open (File ▸ Open, File ▸ New,
  // a reopened .paged) restores that document's own workbook from its
  // container part. A host with `document.onDidOpen@1` reports each open
  // there; an older one only through the client's raw `documentLoaded`
  // broadcast; a host with neither falls back to the lazy restore on frame
  // entry.
  let unsubscribeDocs: (() => void) | null = null;
  if (typeof host.document?.onDidOpen === "function" && host.supports("document.onDidOpen@1")) {
    const sub = host.document.onDidOpen(() => void session.documentOpened());
    unsubscribeDocs = () => sub.dispose();
  } else {
    try {
      unsubscribeDocs = host.editor.client.subscribe((msg) => {
        if (msg.kind === "documentLoaded") void session.documentOpened();
      });
    } catch {
      /* no raw client: onEnter's ensureRestored covers it */
    }
  }

  contributePanel(host, {
    id: PANEL_ID,
    title: "Workbook",
    icon: "panel-canvas",
    component: makeWorkbookPanel(host, session),
    defaultDock: "right",
  });

  // The interim sheets-mode grid panel (spec §8.1, S-02 — NOT the in-frame
  // surface, which is still SDK-blocked). It shares the in-memory session.
  contributePanel(host, {
    id: GRID_PANEL_ID,
    title: "Grid",
    icon: "panel-canvas",
    component: makeGridPanel(host, session),
    defaultDock: "right",
  });

  // S-15 — the datasets panel (the data-provider CONSUMER side): lists the
  // governed datasets the platform offers (host.dataProviders.discover) and
  // sources a sheet from one (session.sourceFromDataset). Consumes ONLY the
  // neutral host.dataProviders surface (§2.1 — never paged.data directly);
  // degrades to an honest empty state when no registry is wired.
  contributePanel(host, {
    id: DATASETS_PANEL_ID,
    title: "Datasets",
    icon: "panel-canvas",
    component: makeDatasetsPanel(host, session),
    defaultDock: "right",
  });

  host.contribute.command({
    id: "media.paged.sheet.command.importXlsx",
    title: "Import workbook (.xlsx)",
    category: "Sheet",
    // S-11: the command now opens the HOST file picker (and falls back to
    // the panel's own input when no picker is wired).
    handler: () => void pickAndImport(host, session, PANEL_ID),
  });
  host.contribute.command({
    id: "media.paged.sheet.command.lowerToFrame",
    title: "Lower selection to frame",
    category: "Sheet",
    handler: () => session.lowerSelection(),
  });
  // Lower a parsed chart to a paged.draw vector frame (M2 charts track, spec
  // §8.4). T0 action lowers the FIRST chart in the workbook (the panel gains a
  // per-chart picker once the chart list UI lands); a chartless workbook is a
  // no-op the command logs.
  host.contribute.command({
    id: "media.paged.sheet.command.lowerChartToFrame",
    title: "Lower chart to frame",
    category: "Sheet",
    handler: async () => {
      const charts = session.listCharts();
      if (charts.length === 0) {
        host.log.warn("lowerChartToFrame: the workbook has no charts");
        return;
      }
      await session.lowerChart(charts[0].index);
    },
  });
  host.contribute.command({
    id: "media.paged.sheet.command.openGrid",
    title: "Open sheet grid",
    category: "Sheet",
    handler: () => host.shell.openPanel(GRID_PANEL_ID),
  });
  // C-1 / S-02 — render the live grid INSIDE the lowered frame on the
  // canvas (gridlines + cell fills + values) via host.contribute
  // .sceneLayer(). The honest companion to the lowered native table:
  // "show me the editable grid in place." Needs rendering ∋ sceneLayer
  // (declared) + the host's scene channel; degrades with a logged warning.
  host.contribute.command({
    id: "media.paged.sheet.command.showGridInFrame",
    title: "Show grid in frame",
    category: "Sheet",
    handler: () => void session.showGridInFrame(),
  });
  host.contribute.command({
    id: "media.paged.sheet.command.hideGridInFrame",
    title: "Hide grid in frame",
    category: "Sheet",
    handler: () => session.hideGridInFrame(),
  });
  // Phase 4b — "Sort range…" / "Find & replace…": both lead to the workbook
  // panel, where the minimal controls live (key column / direction / header;
  // needle / replacement / match toggles). The actual semantics are the
  // engine's (sheet.edit.*); the session routes + journals.
  host.contribute.command({
    id: "media.paged.sheet.command.sortRange",
    title: "Sort range…",
    category: "Sheet",
    handler: () => host.shell.openPanel(PANEL_ID),
  });
  host.contribute.command({
    id: "media.paged.sheet.command.findReplace",
    title: "Find & replace…",
    category: "Sheet",
    handler: () => host.shell.openPanel(PANEL_ID),
  });
  // S-15 — open the datasets panel to source a sheet from a governed
  // dataset (the consumer flow: discover → pick → seed). The actual
  // discover/get/seed lives in the session + panel; the command is the
  // menu/keyboard entry that surfaces the panel.
  host.contribute.command({
    id: "media.paged.sheet.command.sheetFromDataset",
    title: "Sheet from dataset",
    category: "Sheet",
    handler: () => host.shell.openPanel(DATASETS_PANEL_ID),
  });
  // K-6 / S-14 — COPY the selected range to the system clipboard as a
  // tabular payload (+ a TSV text fallback). The engine owns the formatted
  // values (getRangeValues); host.clipboard owns the OS clipboard. Degrades
  // honestly when no range is selected / no clipboard backend is wired.
  host.contribute.command({
    id: "media.paged.sheet.command.copySelection",
    title: "Copy sheet selection",
    category: "Sheet",
    when: workbookIsOpen,
    handler: async () => {
      const r = await session.copySelection();
      if (!r.ok) host.log.warn(`copySelection: ${r.message}`);
    },
  });
  // K-6 / S-14 — PASTE the system clipboard into the grid at the selection
  // anchor (tabular preferred, TSV fallback), each cell re-typed through the
  // journaled editCell lane as one grouped undo step.
  // ADR 024 — RETITLED and GATED, and the title was the worse half.
  //
  // This was called "Paste", and because the editor has no host
  // Copy/Paste command it was the ONLY match a user typing "paste" into
  // the palette could find. Outside a workbook it logged a warning and
  // nothing visible happened — a control named Paste that silently does
  // nothing, which is the most expensive kind of dead offer because the
  // user does not even learn that it failed.
  host.contribute.command({
    id: "media.paged.sheet.command.pasteSelection",
    title: "Paste into sheet",
    category: "Sheet",
    when: workbookIsOpen,
    handler: async () => {
      const r = await session.pasteAtSelection();
      if (!r.ok) host.log.warn(`pasteAtSelection: ${r.message}`);
    },
  });
  // S-04 — the palette entry for the panel's style-from-cell affordance
  // (session.newCellStyleFromSelection was implemented but commandless —
  // the editor-ui-coverage spec's finding). Auto-named here; the workbook
  // panel keeps the named form.
  host.contribute.command({
    id: "media.paged.sheet.command.styleFromCell",
    title: "New cell style from selection",
    category: "Sheet",
    handler: async () => {
      const r = await session.newCellStyleFromSelection("Cell style");
      host.log.info(
        r.ok
          ? `styleFromCell: captured ${r.capturedCount} propert${
              r.capturedCount === 1 ? "y" : "ies"
            }`
          : `styleFromCell: ${r.message}`,
      );
    },
  });

  // Wave 4 — "Paginate into threaded frames": the active range split
  // across the selected frame's thread, live (a resize re-paginates and
  // REPLACES the tables; edits refresh them). Built since Wave 2D, never
  // reachable until now.
  host.contribute.command({
    id: "media.paged.sheet.command.paginateToChain",
    title: "Paginate into threaded frames",
    category: "Sheet",
    when: workbookIsOpen,
    handler: async () => {
      const r = await session.paginateSelection();
      if (!r.ok) host.log.warn(`paginateToChain: ${r.message}`);
    },
  });
  // Wave 4 — a blank workbook to type into, without importing a file.
  host.contribute.command({
    id: "media.paged.sheet.command.newWorkbook",
    title: "New blank workbook",
    category: "Sheet",
    handler: async () => {
      await session.newWorkbook();
      host.shell.openPanel(PANEL_ID);
    },
  });
  // Wave 4 — insert/delete rows or columns at the grid selection (the
  // engine rewrites every reference; it refuses, with the reason, when
  // preserved content would be left addressing the wrong cells).
  for (const [suffix, title, kind] of [
    ["insertRows", "Insert rows", "insertRows"],
    ["deleteRows", "Delete rows", "deleteRows"],
    ["insertColumns", "Insert columns", "insertCols"],
    ["deleteColumns", "Delete columns", "deleteCols"],
  ] as const) {
    host.contribute.command({
      id: `media.paged.sheet.command.${suffix}`,
      title,
      category: "Sheet",
      when: workbookIsOpen,
      handler: () => {
        const r = session.structuralEdit(kind);
        if (!r.ok) host.log.warn(`${suffix}: ${r.message}`);
      },
    });
  }
  host.contribute.command({
    id: "media.paged.sheet.command.addSheet",
    title: "Add sheet",
    category: "Sheet",
    when: workbookIsOpen,
    handler: () => {
      const r = session.addSheet();
      if (!r.ok) host.log.warn(`addSheet: ${r.message}`);
    },
  });

  // Wave 5 — fill, clear and find verbs (palette + menu entries; the grid
  // keys reach the same session verbs).
  const inGrid = () => session.isInFrameActive() && !editableFocus();
  host.contribute.command({
    id: "media.paged.sheet.command.fillDown",
    title: "Fill down",
    category: "Sheet",
    when: workbookIsOpen,
    handler: () => {
      const r = session.fillDown();
      if (!r.ok) host.log.warn(`fillDown: ${r.message}`);
    },
  });
  host.contribute.command({
    id: "media.paged.sheet.command.fillRight",
    title: "Fill right",
    category: "Sheet",
    when: workbookIsOpen,
    handler: () => {
      const r = session.fillRight();
      if (!r.ok) host.log.warn(`fillRight: ${r.message}`);
    },
  });
  host.contribute.command({
    id: "media.paged.sheet.command.clearCells",
    title: "Clear cells",
    category: "Sheet",
    when: workbookIsOpen,
    handler: () => {
      const r = session.clearSelection();
      if (!r.ok) host.log.warn(`clearCells: ${r.message}`);
    },
  });
  host.contribute.command({
    id: "media.paged.sheet.command.findInSheet",
    title: "Find in sheet…",
    category: "Sheet",
    when: workbookIsOpen,
    handler: () => {
      host.shell.openPanel(GRID_PANEL_ID);
      session.requestFind();
    },
  });
  host.contribute.command({
    id: "media.paged.sheet.command.findNext",
    title: "Find next in sheet",
    category: "Sheet",
    when: workbookIsOpen,
    handler: () => {
      if (!session.findAgain()) host.log.info("findNext: no match (or no search yet)");
    },
  });
  // Keys while a sheet frame is entered and focus is not in a text field.
  // Cmd+D is the editor's Place and Tab its chrome toggle (both registered
  // first, so they win) — those reach the grid through the edit-context
  // key forwarding instead (the editor shim in the Wave 5 report).
  for (const [key, command] of [
    ["cmd+c", "media.paged.sheet.command.copySelection"],
    ["ctrl+c", "media.paged.sheet.command.copySelection"],
    ["cmd+v", "media.paged.sheet.command.pasteSelection"],
    ["ctrl+v", "media.paged.sheet.command.pasteSelection"],
    ["cmd+d", "media.paged.sheet.command.fillDown"],
    ["cmd+r", "media.paged.sheet.command.fillRight"],
    ["ctrl+r", "media.paged.sheet.command.fillRight"],
    ["cmd+f", "media.paged.sheet.command.findInSheet"],
    ["ctrl+f", "media.paged.sheet.command.findInSheet"],
  ] as const) {
    if (typeof host.contribute.keybinding !== "function") break;
    host.contribute.keybinding({ key, command, when: inGrid });
  }

  // K-1 entry — double-click a lowered sheet frame to ENTER "sheet" mode:
  // the live in-frame grid renders (C-1 sceneLayer); Esc / exit clears it.
  // The objectType marks a frame as a sheet by its OWN binding metadata
  // (x-paged:media.paged.sheet — the host resolves the candidate's
  // metadata from this plugin's envelope, so `parseBinding` validates it)
  // and routes the double-click to the "sheet" context instead of group
  // descent. The cell-pointer editing channel (onContentPointerDown +
  // selectCell) lands with the editor's content-pointer delivery (K-1
  // ViewportCanvas wire) — see k1-modal-session-plan.md.
  if (host.supports("contribute.objectType@1")) {
    host.contribute.objectType({
      type: "sheetFrame",
      bakedFallback: "rectangle",
      matches: (c) => parseBinding(c.metadata) !== null,
      editContextType: "sheet",
    });
  }
  if (host.supports("contribute.editContext@1")) {
    host.contribute.editContext({
      type: "sheet",
      entry: "doubleClick",
      // ADR 024 — NO CANVAS TOOL EDITS A SPREADSHEET, and saying so is
      // now possible: a declared-empty list restricts to nothing, where
      // it used to collapse into "unrestricted" and leave the whole rail
      // lit. Inside a sheet you edit by keyboard and by panel — the
      // grid takes pointer and key events through this context's own
      // hooks below, not through a tool.
      //
      // Not a trap: the rail treats a pick outside the set as an EXIT
      // (it commits the context, then activates), so reaching for the
      // pointer walks you out of the sheet rather than doing nothing.
      toolIds: [],
      // The workbook panel is this context's own surface. Deliberately
      // NOT the host panels its binding providers serve (Swatches,
      // Character/Paragraph) — naming those here would put host panel
      // ids in plugin code, which is the coupling ADR 023 removed from
      // the value lane, and the host already infers "serves" from
      // `provides`.
      panelIds: [PANEL_ID],
      onEnter: (ctx) => {
        const id = frameIdOf(ctx.id);
        if (!id) return;
        // A frame entered before its document's workbook is loaded (a host
        // that never announced the open, or a restore still in flight)
        // restores it first instead of reporting "no workbook".
        void session.ensureRestored().then(() => session.showGridInFrame(id));
      },
      // K-1 — the editor delivers a pointer in FRAME-CONTENT coordinates
      // (it owns the page→content inversion via the frame's HitResult
      // bounds + item_transform; §8.5 — the plugin never compensates).
      // Map it to a cell + select it (re-renders the in-frame grid with
      // the selection chrome).
      onContentPointerDown: (e) => {
        if (e.button !== 0) return;
        session.pointerDownInFrame(e.contentPoint[0], e.contentPoint[1], {
          shift: e.modifiers.shift,
        });
      },
      // Wave 5 — drag extends the selection (or drags the fill handle);
      // the release ends the gesture (a fill-handle release fills).
      onContentPointerMove: (e) => {
        session.pointerMoveInFrame(e.contentPoint[0], e.contentPoint[1]);
      },
      onContentPointerUp: (e) => {
        session.pointerUpInFrame(e.contentPoint[0], e.contentPoint[1]);
      },
      // A plain wheel over the entered frame scrolls the grid window (by
      // whole rows / columns); declining at an edge lets the canvas pan.
      // Shift turns a vertical wheel horizontal. Typed locally until the
      // contract names the hook (see protocol66.ts).
      onContentWheel: (e: ContentWheelEvent) => {
        const [dx, dy] = e.delta;
        return e.modifiers.shift && dx === 0
          ? session.wheelInFrame(dy, 0)
          : session.wheelInFrame(dx, dy);
      },
      // K-1 + Wave 5 — every key the shell forwards goes through the grid's
      // key map (grid-nav.ts): typing edits the active cell, Enter/Tab
      // commit and move, arrows move (shift extends), F2 edits in place,
      // Delete/Backspace clear the selection, Cmd+C/V/D/R/A/F/Z. NOTE the
      // editor forwards only printable keys, Backspace and Delete while no
      // cell edit is open — arrows, Tab, Enter, F2 and Cmd combos reach
      // here only mid-edit until the editor forwards them (shim recorded in
      // the Wave 5 report); the keybindings below cover Cmd+C/V/F/R today.
      onContentKey: (e) => {
        if (session.handleGridKey(e)) e.preventDefault?.();
      },
      // The context is "dirty" while a cell edit is open — gates the shell's
      // Enter/Esc routing (to the cell) + a future discard prompt (§8.0).
      isDirty: () => session.isCellEditing(),
      // ADR-012 Tier 1 — this context OWNS undo while active: the shell
      // routes Cmd-Z / Cmd-Shift-Z (and Edit/Undo) to the session's
      // journal of committed cell edits (workbook grain), never the
      // document stack; the modal exit is the document's one-step grain
      // (Tier 2). In-session Cmd-Z stops at the session's start.
      onUndo: () => session.undoCellEdit(),
      onRedo: () => session.redoCellEdit(),
      onCanUndo: () => session.canUndoCellEdit(),
      onCanRedo: () => session.canRedoCellEdit(),
      // Wave 5 — the journal SURVIVES the exit: the session's edits land on
      // the page as one refresh (Tier 2), and a later host Cmd-Z that undoes
      // that refresh unwinds the workbook with it (the session's
      // document-undo follower). Inside the next session Cmd-Z stops at its
      // own start.
      onExit: () => {
        session.hideGridInFrame();
      },
    } as EditContextContribution & ContentWheelHook);

    // ADR 023 phase D — paged.sheet answers the HOST's Swatches panel
    // while the `sheet` context is active: the WORKBOOK PALETTE (the
    // document swatches this workbook's charts + data bars mint) instead
    // of the document's own swatch list, plus first refusal on
    // `editSwatch`.
    //
    // Registered INSIDE the editContext branch on purpose — the
    // provider's lifetime is BORROWED from that context's
    // onEnter/onExit, so a host that cannot host the context cannot host
    // the provider either. The door itself probes separately and
    // degrades to "no provider" on a pre-ADR host.
    swatchesProviderHandle = registerBindingProvider(
      host,
      "sheet",
      makeSwatchesBindingProvider(host, session).provider,
    );

    // ADR 023 phase D — the CHARACTER/PARAGRAPH provider (the VALUE
    // axis). Registered on the SAME context and therefore with the same
    // borrowed lifetime; a separate registration only because the two
    // answer different lanes about the same selection.
    textProviderHandle = registerBindingProvider(
      host,
      "sheet",
      makeTextBindingProvider(host, {
        textSelectionRange: () => session.textSelectionRange(),
        lowerRange: (sheet, range) => {
          const engine = session.state().engine;
          if (!engine) return null;
          // The STYLED door, not the frozen page-lowering one: the
          // Character panel is about the workbook's real cell fonts, and
          // `getRangeLowered` emits a key-0-only table by contract.
          return engine.getRangeStyled(sheet, range, {
            includeGridRules: false,
          });
        },
      }).provider,
    );

    if (swatchesProviderHandle || textProviderHandle) {
      // Each provider re-reads only on changes that can move its answer
      // (see binding-provider/invalidation.ts) — not on every signal.
      providerInvalidateSub = subscribeProviderInvalidation(session, {
        swatches: swatchesProviderHandle,
        text: textProviderHandle,
      });
    }
  }

  // K-2 / S-06 — register the .xlsx IMPORTER so opening a spreadsheet
  // through the editor's File/Open or drag-drop routes its bytes HERE (the
  // host loads them into this in-memory session instead of the IDML
  // loader — it does NOT replace the document). Same path as the in-panel
  // import; degrades honestly if the host predates the door.
  if (host.supports("contribute.importer@1")) {
    host.contribute.importer({
      id: "media.paged.sheet.importer.xlsx",
      title: "Spreadsheet",
      extensions: [".xlsx"],
      mimeTypes: [XLSX_MIME],
      import: async ({ name, bytes }) => {
        await session.import(bytes, name);
        host.shell.openPanel(PANEL_ID);
      },
    });
    // Wave 4 — CSV / TSV open through the same door: the text becomes a
    // one-sheet workbook (typed in Rust by the host language's locale).
    host.contribute.importer({
      id: "media.paged.sheet.importer.csv",
      title: "Delimited text (CSV/TSV)",
      extensions: [".csv", ".tsv"],
      mimeTypes: DELIMITED_MIMES,
      import: async ({ name, bytes }) => {
        await importBytes(session, bytes, name);
        host.shell.openPanel(PANEL_ID);
      },
    });
  }
  // K-2 / S-06 — register the .xlsx EXPORTER: the Export Center pulls the
  // workbook bytes on demand (the host owns blob→download). Preservation-
  // first re-emit via session.saveWorkbook → engine.saveXlsx (§10.2).
  if (host.supports("contribute.exporter@1")) {
    host.contribute.exporter({
      id: "media.paged.sheet.exporter.xlsx",
      title: "Workbook (.xlsx)",
      extension: ".xlsx",
      mimeType: XLSX_MIME,
      export: () => session.saveWorkbook(),
    });
  }

  host.log.info(`activated (apiVersion ${manifest.apiVersion})`);

  // F1 — the menu bar. Before plugin-api 0.2.33 there was no menu door,
  // so every command in this bundle lived behind Cmd+K and nowhere else.
  const menuSub = contributeMenu(host);

  return {
    dispose() {
      providerInvalidateSub?.dispose();
      providerInvalidateSub = null;
      swatchesProviderHandle?.dispose();
      swatchesProviderHandle = null;
      textProviderHandle?.dispose();
      menuSub.dispose();
      textProviderHandle = null;
      unsubscribeDocs?.();
      session.dispose();
    },
  };
}

export { manifest, PANEL_ID, GRID_PANEL_ID, DATASETS_PANEL_ID };
