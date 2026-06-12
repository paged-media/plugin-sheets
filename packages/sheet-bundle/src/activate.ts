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

import type { BundleHandle, BundleHost } from "@paged-media/plugin-api";
import { contributePanel } from "@paged-media/plugin-sdk";
import { parseBinding } from "@paged-media/sheet-host-model";

import manifest from "../manifest.json";

import { pickAndImport, XLSX_MIME } from "./import-xlsx";
import { createWorkbookSession } from "./session";
import { makeWorkbookPanel } from "./panels/workbook-panel";
import { makeGridPanel } from "./panels/grid-panel";
import { makeDatasetsPanel } from "./panels/datasets-panel";

const PANEL_ID = "media.paged.sheet.panel.workbook";
const GRID_PANEL_ID = "media.paged.sheet.panel.grid";
const DATASETS_PANEL_ID = "media.paged.sheet.panel.datasets";

/** The raw id string of a frame-like `ElementId` (textFrame / rectangle
 *  carry a string `id`), or null. Structural so it needs no wire import. */
function frameIdOf(id: unknown): string | null {
  if (typeof id === "object" && id !== null) {
    const e = id as { id?: unknown };
    if (typeof e.id === "string") return e.id;
  }
  return null;
}

export function activate(host: BundleHost): BundleHandle {
  const session = createWorkbookSession(host);

  // S-08: restore the last persisted workbook from host.blob, if any. A
  // cheap no-op (one blob read) when nothing was persisted or no blob
  // store is wired — the engine boots only when there are bytes to load.
  void session.restore();

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
      onEnter: (ctx) => {
        const id = frameIdOf(ctx.id);
        if (id) void session.showGridInFrame(id);
      },
      // K-1 — the editor delivers a pointer in FRAME-CONTENT coordinates
      // (it owns the page→content inversion via the frame's HitResult
      // bounds + item_transform; §8.5 — the plugin never compensates).
      // Map it to a cell + select it (re-renders the in-frame grid with
      // the selection chrome).
      onContentPointerDown: (e) => {
        session.selectCellInFrame(e.contentPoint[0], e.contentPoint[1]);
      },
      // K-1 — a printable key types into the selected cell (an in-frame
      // edit buffer the grid re-renders); Enter commits, Esc cancels,
      // Backspace deletes. The shell routes Enter/Esc HERE (not to the
      // context commit/cancel) while `isDirty` is true — so an in-progress
      // cell edit owns those keys. Cmd/Ctrl combos never reach this.
      onContentKey: (e) => {
        if (e.key === "Enter") session.commitCellEdit();
        else if (e.key === "Escape") session.cancelCellEdit();
        else if (e.key === "Backspace") session.backspaceCellEdit();
        else if (e.key.length === 1) session.typeCellChar(e.key);
      },
      // The context is "dirty" while a cell edit is open — gates the shell's
      // Enter/Esc routing (to the cell) + a future discard prompt (§8.0).
      isDirty: () => session.isCellEditing(),
      // ADR-012 Tier 1 — this context OWNS undo while active: the shell
      // routes Cmd-Z / Cmd-Shift-Z (and Edit/Undo) to the session's
      // journal of committed cell edits (workbook grain), never the
      // document stack; the modal exit is the document's one-step grain
      // (Tier 2). The journal dies with the session (cleared on exit).
      onUndo: () => session.undoCellEdit(),
      onRedo: () => session.redoCellEdit(),
      onCanUndo: () => session.canUndoCellEdit(),
      onCanRedo: () => session.canRedoCellEdit(),
      onExit: () => {
        session.clearCellEditJournal();
        session.hideGridInFrame();
      },
    });
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

  return {
    dispose() {
      session.dispose();
    },
  };
}

export { manifest, PANEL_ID, GRID_PANEL_ID, DATASETS_PANEL_ID };
