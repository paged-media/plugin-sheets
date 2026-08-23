/**
 * paged.sheet — the menu bar entries.
 *
 * A top-level `Sheet` menu, plus the one verb that belongs in the host's `Object` menu: importing a workbook MINTS a page item, which is what Object ▸ Insert already means.
 *
 * Registered through `contribute.menu()` (plugin-api 0.2.33). Before it
 * there was no menu door at all, so every verb here lived behind Cmd+K
 * and nowhere else.
 *
 * TWO PATHS DELIBERATELY DO NOT MATCH THEIR COMMAND TITLES.
 * `lowerToFrame` is registered as "Lower selection to frame" and
 * `lowerChartToFrame` as "Lower chart to frame" — compiler vocabulary
 * for the step that actually puts the sheet on the page. A menu is read
 * by someone deciding what to do next, so these read as "Place on
 * page". The command titles are a separate fix; a menu that repeated
 * them would spread the problem rather than route around it.
 *
 * `Object ▸ Insert spreadsheet…` is the SAME path the host curates as a
 * courtesy. That is intentional and is what `fallbackFor` exists for:
 * the plugin's own entry supersedes the host's, so the item stays put
 * for the user and the host stops speaking on this bundle's behalf.
 * */

import type { BundleHost, Disposable } from "@paged-media/plugin-api";

const C = "media.paged.sheet.command";

/** `[path, command suffix, group]`. */
const ENTRIES: [path: string, suffix: string, group: string][] = [
  ["Object/Insert spreadsheet…", "importXlsx", "insert-plugin"],
  ["Sheet/Open sheet grid", "openGrid", "grid"],
  ["Sheet/Show grid in frame", "showGridInFrame", "grid-frame"],
  ["Sheet/Hide grid in frame", "hideGridInFrame", "grid-frame"],
  ["Sheet/Place selection on page", "lowerToFrame", "place"],
  ["Sheet/Place chart on page", "lowerChartToFrame", "place"],
  ["Sheet/Sort range…", "sortRange", "edit"],
  ["Sheet/Find & replace…", "findReplace", "edit"],
  ["Sheet/Copy cells", "copySelection", "clipboard"],
  ["Sheet/Paste into sheet", "pasteSelection", "clipboard"],
  ["Sheet/Style from cell", "styleFromCell", "style"],
  ["Sheet/New sheet from dataset", "sheetFromDataset", "dataset"],
];

/**
 * Register every entry; one Disposable drops them all. Degrades on a
 * host older than plugin-api 0.2.33 by contributing nothing and saying
 * so, rather than throwing and taking the bundle down over a menu.
 */
export function contributeMenu(host: BundleHost): Disposable {
  const contribute = host.contribute as BundleHost["contribute"] & {
    menu?: (c: {
      path: string;
      command: string;
      order?: number;
      group?: string;
    }) => Disposable;
  };
  if (typeof contribute.menu !== "function") {
    host.log.info(
      "host predates contribute.menu (plugin-api 0.2.33) — " +
        `${ENTRIES.length} menu entries not contributed; every command ` +
        "remains reachable through the command palette",
    );
    return { dispose() {} };
  }

  const handles: Disposable[] = [];
  const perGroup = new Map<string, number>();
  for (const [path, suffix, group] of ENTRIES) {
    const n = (perGroup.get(group) ?? 0) + 1;
    perGroup.set(group, n);
    handles.push(
      contribute.menu({ path, command: `${C}.${suffix}`, group, order: n * 10 }),
    );
  }
  host.log.info(`contributed ${handles.length} menu entries`);
  return {
    dispose() {
      for (const h of handles) h.dispose();
      handles.length = 0;
    },
  };
}

/** Exported for the bundle's own test. */
export const MENU_ENTRIES = ENTRIES;
export const MENU_COMMAND_PREFIX = C;
