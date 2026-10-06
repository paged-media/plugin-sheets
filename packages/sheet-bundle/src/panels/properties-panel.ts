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

// ADR 323 (object-model design §4 "Panels") — the workbook's object
// properties as a SCHEMA panel of property rows: the host renders each row
// with its one schema-driven `PropertyField` over `host.objects` (widget
// from the kind's schema row, one undo step per commit, "Bind to data…" on
// every field). The bundle declares fields, never widgets. The Workbook and
// Grid panels keep their own controls.
//
//   · Workbook — the one workbook (an explicit address).
//   · Sheet / Named range / Chart — a list (publishObjectList) and the
//     picked object's rows, addressed `{ bind }`.
//   · Cell — the grid's ACTIVE cell, published from the session.

import type { Address, BundleHost, Disposable, PanelSchema, PanelSchemaRow } from "@paged-media/plugin-api";
import { propertyRows, publishObjectList } from "@paged-media/plugin-sdk";

import { PLUGIN_ID, WORKBOOK_ADDRESS, cellAddressOf } from "../object-model";
import type { WorkbookSession } from "../session";

export const PROPERTIES_PANEL_ID = "media.paged.sheet.panel.properties";

type FieldKind = "workbook" | "sheet" | "cell" | "namedRange" | "chart";

/** The schema-driven panel fields (ADR 323 / Wave 4), in panel order. Every
 *  entry is a writable row of its kind (a spec holds it to that); tables
 *  are read-only and have none. */
export const SHEET_PANEL_FIELDS: ReadonlyArray<{ kind: FieldKind; path: string; group: string }> = [
  { kind: "workbook", path: "name", group: "Workbook" },
  { kind: "workbook", path: "iterative", group: "Workbook" },
  { kind: "sheet", path: "name", group: "Sheet" },
  { kind: "sheet", path: "freezeRows", group: "Sheet" },
  { kind: "sheet", path: "freezeCols", group: "Sheet" },
  { kind: "cell", path: "value", group: "Cell" },
  { kind: "cell", path: "formula", group: "Cell" },
  { kind: "cell", path: "numberFormat", group: "Cell" },
  { kind: "cell", path: "fontName", group: "Cell" },
  { kind: "cell", path: "fontSize", group: "Cell" },
  { kind: "cell", path: "bold", group: "Cell" },
  { kind: "cell", path: "italic", group: "Cell" },
  { kind: "cell", path: "underline", group: "Cell" },
  { kind: "cell", path: "fontColor", group: "Cell" },
  { kind: "cell", path: "fill", group: "Cell" },
  { kind: "cell", path: "hAlign", group: "Cell" },
  { kind: "cell", path: "vAlign", group: "Cell" },
  { kind: "cell", path: "wrap", group: "Cell" },
  { kind: "namedRange", path: "name", group: "Named range" },
  { kind: "namedRange", path: "refersTo", group: "Named range" },
  { kind: "namedRange", path: "scope", group: "Named range" },
  { kind: "chart", path: "kind", group: "Chart" },
  { kind: "chart", path: "title", group: "Chart" },
  { kind: "chart", path: "legend", group: "Chart" },
  { kind: "chart", path: "categoryAxisTitle", group: "Chart" },
  { kind: "chart", path: "valueAxisTitle", group: "Chart" },
  { kind: "chart", path: "valueAxisMin", group: "Chart" },
  { kind: "chart", path: "valueAxisMax", group: "Chart" },
];

const qualified = (kind: FieldKind) => `plugin:${PLUGIN_ID}/${kind}`;

/** Published bindings the lists and their property rows share. */
export const BIND = {
  cell: "sheet.properties.cell",
  sheets: "sheet.properties.sheets",
  sheet: "sheet.properties.sheet",
  names: "sheet.properties.namedRanges",
  name: "sheet.properties.namedRange",
  charts: "sheet.properties.charts",
  chart: "sheet.properties.chart",
} as const;

const pathsOf = (group: string) => SHEET_PANEL_FIELDS.filter((f) => f.group === group).map((f) => f.path);
const kindOf = (group: string) => SHEET_PANEL_FIELDS.find((f) => f.group === group)!.kind;

const list = (rows: string, select: string): PanelSchemaRow => ({
  widget: "paged.list",
  list: { items: { kind: "binding", bind: rows }, labelField: "name", secondaryField: "secondary", selectionBinding: select },
});

const section = (group: string, address: Parameters<typeof propertyRows>[2], lead: PanelSchemaRow[] = []) => ({
  title: group,
  rows: [...lead, ...propertyRows(qualified(kindOf(group)), pathsOf(group), address)],
});

/** The panel, from `SHEET_PANEL_FIELDS`. */
export const SHEET_PROPERTIES_PANEL: PanelSchema = {
  id: PROPERTIES_PANEL_ID,
  title: "Sheet properties",
  icon: "panel-canvas",
  defaultDock: "right",
  sections: [
    section("Cell", { bind: BIND.cell }),
    section("Sheet", { bind: BIND.sheet }, [list(BIND.sheets, BIND.sheet)]),
    section("Workbook", { addresses: [WORKBOOK_ADDRESS] }),
    { ...section("Named range", { bind: BIND.name }, [list(BIND.names, BIND.name)]), collapsible: true },
    { ...section("Chart", { bind: BIND.chart }, [list(BIND.charts, BIND.chart)]), collapsible: true },
  ],
};

/** The grid's active cell as an address, or null. */
export function activeCellAddress(session: WorkbookSession): Address | null {
  const sheet = session.state().activeSheet;
  const at = session.activeCell();
  if (sheet === null || !at) return null;
  const name = session.sheets().find((s) => s.id === sheet)?.name;
  return name ? cellAddressOf(name, at.row, at.col) : null;
}

/** Keep the panel's bindings live: the active cell (from the session) and
 *  the sheet / named-range / chart lists (from `host.objects`). */
export function publishPropertyBindings(host: BundleHost, session: WorkbookSession): Disposable {
  const publishCell = () => {
    const a = activeCellAddress(session);
    if (a === null) host.bindings.delete(BIND.cell);
    else if (host.bindings.get(BIND.cell) !== a) host.bindings.publish(BIND.cell, a);
  };
  publishCell();
  const sub = session.onDidChange(publishCell);
  const lists = [
    publishObjectList(host, { rows: BIND.sheets, select: BIND.sheet, selector: () => qualified("sheet") }),
    publishObjectList(host, {
      rows: BIND.names,
      select: BIND.name,
      selector: () => qualified("namedRange"),
      secondaryPath: "refersTo",
    }),
    publishObjectList(host, {
      rows: BIND.charts,
      select: BIND.chart,
      selector: () => qualified("chart"),
      labelPath: "title",
      secondaryPath: "kind",
    }),
  ];
  // A workbook loading / switching sheets changes the lists the object
  // model does not announce: re-read on the session's structural changes.
  const listSub = session.onDidChange((c) => {
    if (c.kind === "other") for (const l of lists) void l.refresh();
  });
  return {
    dispose() {
      sub.dispose();
      listSub.dispose();
      for (const l of lists) l.dispose();
    },
  };
}
