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

import { importXlsx } from "./import-xlsx";
import { createWorkbookSession } from "./session";
import { makeWorkbookPanel } from "./panels/workbook-panel";

const PANEL_ID = "media.paged.sheet.panel.workbook";

export function activate(host: BundleHost): BundleHandle {
  const session = createWorkbookSession(host);

  contributePanel(host, {
    id: PANEL_ID,
    title: "Workbook",
    icon: "panel-canvas",
    component: makeWorkbookPanel(host, session),
    defaultDock: "right",
  });

  host.contribute.command({
    id: "media.paged.sheet.command.importXlsx",
    title: "Import workbook (.xlsx)",
    category: "Sheet",
    handler: () => importXlsx(host, PANEL_ID),
  });
  host.contribute.command({
    id: "media.paged.sheet.command.lowerToFrame",
    title: "Lower selection to frame",
    category: "Sheet",
    handler: () => session.lowerSelection(),
  });

  host.log.info(`activated (apiVersion ${manifest.apiVersion})`);

  return {
    dispose() {
      session.dispose();
    },
  };
}

export { manifest, PANEL_ID };
