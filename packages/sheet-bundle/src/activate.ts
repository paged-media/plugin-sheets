// The paged.sheet bundle entry. T0 scope (the honest slice): import an
// XLSX into an in-memory engine, pick a sheet + range, and LOWER it to a
// page frame as tab-aligned text + drawn rules (the spec §2.2
// degradation, S-03), bound via plugin metadata. Sheets mode (S-01), the
// grid surface (S-02), threading/pagination (S-05), and persistence
// (S-08) are NOT implemented — the panel + BREAKAGE_LOG say so.
//
// Wiring mirrors plugin-draw/plugin-web: contributePanel for the workbook
// panel + the two commands (importXlsx opens the panel; lowerToFrame runs
// the session lower). The host tracks every registration; the session is
// the one thing allocated OUTSIDE a facade-tracked registration, so
// dispose tears it down.

import type { BundleHandle, BundleHost } from "@paged-media/plugin-api";
import { contributePanel } from "@paged-media/plugin-sdk";

import manifest from "../manifest.json";

import { pickAndImport, XLSX_MIME } from "./import-xlsx";
import { createWorkbookSession } from "./session";
import { makeWorkbookPanel } from "./panels/workbook-panel";
import { makeGridPanel } from "./panels/grid-panel";

const PANEL_ID = "media.paged.sheet.panel.workbook";
const GRID_PANEL_ID = "media.paged.sheet.panel.grid";

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

export { manifest, PANEL_ID, GRID_PANEL_ID };
