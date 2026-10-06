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

// ADR 323 — the sheet properties panel declares property rows (fields, not
// widgets); every row is a writable row of its kind's schema, the address
// forms are well-formed (plugin-sdk validatePanelSchema), and the active
// cell is published as an address the cell rows read.

import { describe, expect, it } from "vitest";

import type { PropertySchema } from "@paged-media/plugin-api";
import { propertyRowsOf, validatePanelSchema } from "@paged-media/plugin-sdk";

import manifest from "../manifest.json";
import { SHEET_KINDS, SHEET_SCHEMAS, type SheetKind } from "../src/object-model";
import {
  BIND,
  PROPERTIES_PANEL_ID,
  SHEET_PANEL_FIELDS,
  SHEET_PROPERTIES_PANEL,
  activeCellAddress,
  publishPropertyBindings,
} from "../src/panels/properties-panel";

const PREFIX = `plugin:${manifest.id}/`;
const schemaOf = (k: string): readonly PropertySchema[] | undefined => {
  const kind = k.slice(PREFIX.length) as SheetKind;
  return k.startsWith(PREFIX) && SHEET_KINDS.includes(kind) ? SHEET_SCHEMAS[kind] : undefined;
};

describe("sheet properties panel (property rows)", () => {
  it("validates against the kinds' schemas", () => {
    expect(validatePanelSchema(SHEET_PROPERTIES_PANEL, { pluginId: manifest.id, schemaOf })).toEqual([]);
  });

  it("every SHEET_PANEL_FIELDS entry is a writable row, rendered through a property row", () => {
    for (const f of SHEET_PANEL_FIELDS) {
      const row = SHEET_SCHEMAS[f.kind].find((r) => r.path === f.path);
      expect(row, `${f.kind}.${f.path}`).toBeDefined();
      expect(row!.access ?? "readWrite", `${f.kind}.${f.path}`).toBe("readWrite");
    }
    const rows = propertyRowsOf(SHEET_PROPERTIES_PANEL).map((r) => `${r.field.kind!.slice(PREFIX.length)}.${r.field.path}`);
    expect(rows.sort()).toEqual(SHEET_PANEL_FIELDS.map((f) => `${f.kind}.${f.path}`).sort());
    // Tables are read-only: no field.
    expect(SHEET_PANEL_FIELDS.some((f) => (f.kind as string) === "table")).toBe(false);
  });

  it("is declared in the manifest", () => {
    expect(manifest.contributes.panels).toContain(PROPERTIES_PANEL_ID);
  });

  it("publishes the grid's active cell as the cell rows' address", () => {
    let active: { row: number; col: number } | null = { row: 1, col: 2 };
    const listeners: Array<(c: { kind: string }) => void> = [];
    const session = {
      state: () => ({ activeSheet: 7 }),
      activeCell: () => active,
      sheets: () => [{ id: 7, name: "Sheet1", rows: 3, cols: 3 }],
      onDidChange: (l: (c: { kind: string }) => void) => (listeners.push(l), { dispose() {} }),
    };
    expect(activeCellAddress(session as never)).toBe(`${PREFIX}cell/Sheet1!C2`);
    const values = new Map<string, unknown>();
    const host = {
      bindings: {
        publish: (n: string, v: unknown) => values.set(n, v),
        get: (n: string) => values.get(n),
        delete: (n: string) => values.delete(n),
      },
      selection: { get: () => [], onDidChange: () => ({ dispose() {} }) },
      objects: { query: async () => [], get: async () => ({ kind: "absent" }), onDidChange: () => ({ dispose() {} }) },
    };
    const d = publishPropertyBindings(host as never, session as never);
    expect(values.get(BIND.cell)).toBe(`${PREFIX}cell/Sheet1!C2`);
    active = { row: 0, col: 0 };
    for (const l of listeners) l({ kind: "selection" });
    expect(values.get(BIND.cell)).toBe(`${PREFIX}cell/Sheet1!A1`);
    active = null;
    for (const l of listeners) l({ kind: "selection" });
    expect(values.has(BIND.cell)).toBe(false);
    d.dispose();
  });
});
