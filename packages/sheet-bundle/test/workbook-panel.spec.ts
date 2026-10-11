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

// The workbook panel's layout (panel review 2026-10): collapsible sections
// with "Place on page" open, one home per duplicated setting (a pointer to
// Sheet properties / the Grid panel instead of a second set of controls),
// the "Place on page" verb, and the second-click confirm on actions that
// can't be undone. Also the datasets panel's display names.

import { createElement } from "react";
import { act, create, type ReactTestInstance, type ReactTestRenderer } from "react-test-renderer";
import { describe, expect, it, vi } from "vitest";

import type { BundleHost } from "@paged-media/plugin-api";

import { createWorkbookSession, makeWorkbookPanel, type SheetEngine } from "../src";
import { datasetLabel, makeDatasetsPanel } from "../src/panels/datasets-panel";

function fakeEngine(sheetCount = 2): SheetEngine {
  return {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: () => ({ changed: [] }),
    getCellDisplay: () => "",
    getCellInput: () => "",
    sortRange: () => ({ changed: [], edits: [] }),
    findAll: () => [],
    replaceAll: () => ({ occurrences: 0, changed: [], edits: [], skipped: [] }),
    getRangeLowered: () => ({ cols: [], rows: [], rules: { h: [], v: [] }, merges: [] }),
    getRangeStyled: () => ({ cols: [], rows: [], rules: { h: [], v: [] }, merges: [] }),
    getRangeValues: () => [],
    paginate: () => [],
    getGridScene: () => null as never,
    setGridSelection() {},
    listSheets: () =>
      Array.from({ length: sheetCount }, (_, id) => ({ id, name: `Sheet${id + 1}`, rows: 2, cols: 2 })),
    listCharts: () => [{ index: 0, kind: "column", title: "", seriesCount: 2 }],
    chartKinds: () => ["column", "line"],
    addChart: () => 0,
    listFreezePanes: () => [],
    listDataValidations: () => [],
    listComments: () => [],
    listFunctions: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  } as unknown as SheetEngine;
}

function fakeHost(features: string[] = []): { host: BundleHost; opened: string[] } {
  const opened: string[] = [];
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) => features.includes(f),
    shell: { openPanel: (id: string) => void opened.push(id) },
  } as unknown as BundleHost;
  return { host, opened };
}

const byData = (tree: ReactTestRenderer, key: string, value?: string): ReactTestInstance[] =>
  tree.root.findAll(
    (n) =>
      typeof n.type === "string" &&
      n.props[key] !== undefined &&
      (value === undefined || n.props[key] === value),
  );

const text = (n: ReactTestInstance): string => JSON.stringify(n.props.children);

function mount(features: string[] = [], sheetCount = 2) {
  const { host, opened } = fakeHost(features);
  const session = createWorkbookSession(host);
  const st = session.state();
  st.engine = fakeEngine(sheetCount);
  st.activeSheet = 0;
  st.selectedRange = "A1:B2";
  const Panel = makeWorkbookPanel(host, session);
  let tree!: ReactTestRenderer;
  act(() => {
    tree = create(createElement(Panel));
  });
  return { tree, session, opened };
}

describe("workbook panel layout", () => {
  it("groups the controls into collapsible sections; only Place on page starts open", () => {
    const { tree } = mount();
    const sections = byData(tree, "data-sheet-section");
    expect(sections.map((s) => s.props["data-sheet-section"])).toEqual([
      "place",
      "structure",
      "format",
      "names",
      "sort",
      "cell-style",
      "charts",
      "functions",
    ]);
    expect(sections.filter((s) => s.props.open).map((s) => s.props["data-sheet-section"])).toEqual([
      "place",
    ]);
  });

  it("says Place on page, for a range and for a chart", () => {
    const { tree } = mount();
    expect(text(byData(tree, "data-sheet-lower")[0])).toContain("Place on page");
    expect(text(byData(tree, "data-sheet-chart-lower")[0])).toContain("Place on page");
    expect(JSON.stringify(tree.toJSON())).not.toMatch(/Lower to frame/);
  });

  it("points to Sheet properties and the Grid instead of repeating their controls", () => {
    const { tree, opened } = mount(["contribute.schemaPanel@1"]);
    expect(byData(tree, "data-sheet-fmt-pointer")).toHaveLength(1);
    expect(byData(tree, "data-sheet-fmt-bold")).toHaveLength(0);
    expect(byData(tree, "data-sheet-fmt-numfmt")).toHaveLength(0);
    expect(byData(tree, "data-sheet-iterative")).toHaveLength(0);
    // Borders / layout have no other home: they stay.
    expect(byData(tree, "data-sheet-fmt-border", "outline")).toHaveLength(1);
    // Find lives in the Grid panel only.
    expect(byData(tree, "data-sheet-find-needle")).toHaveLength(0);
    act(() => byData(tree, "data-sheet-open-properties")[0].props.onClick());
    act(() => byData(tree, "data-sheet-open-grid")[0].props.onClick());
    expect(opened).toEqual(["media.paged.sheet.panel.properties", "media.paged.sheet.panel.grid"]);
  });

  it("keeps the format rows and iterative calculation where the host has no Sheet properties", () => {
    const { tree } = mount([]);
    expect(byData(tree, "data-sheet-fmt-pointer")).toHaveLength(0);
    expect(byData(tree, "data-sheet-fmt-bold")).toHaveLength(1);
    expect(byData(tree, "data-sheet-iterative")).toHaveLength(1);
  });

  it("asks for a second click before deleting a sheet, and the arm lapses", () => {
    vi.useFakeTimers();
    try {
      const { tree, session } = mount();
      const del = vi.spyOn(session, "deleteSheet").mockReturnValue({ ok: true });
      const button = () => byData(tree, "data-sheet-delete")[0];
      act(() => button().props.onClick());
      expect(del).not.toHaveBeenCalled();
      expect(text(button())).toContain("Confirm delete");
      act(() => void vi.advanceTimersByTime(5000));
      expect(text(button())).toContain("Delete sheet");
      act(() => button().props.onClick());
      act(() => button().props.onClick());
      expect(del).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("asks for a second click before a new workbook replaces the open one", () => {
    const { tree, session } = mount();
    const fresh = vi.spyOn(session, "newWorkbook").mockResolvedValue();
    act(() => byData(tree, "data-sheet-new")[0].props.onClick());
    expect(fresh).not.toHaveBeenCalled();
    expect(text(byData(tree, "data-sheet-new")[0])).toContain("Confirm replace");
    act(() => byData(tree, "data-sheet-new")[0].props.onClick());
    expect(fresh).toHaveBeenCalledTimes(1);
  });
});

describe("datasets panel names", () => {
  it("derives a display name from the provider id", () => {
    expect(datasetLabel("pricing-dataset")).toBe("Pricing");
    expect(datasetLabel("q_all")).toBe("Q all");
    expect(datasetLabel("fct_products")).toBe("Fct products");
    expect(datasetLabel("media.paged.data/orders")).toBe("Orders");
  });

  it("shows the name, not the id, and keeps the id in the tooltip", () => {
    const host = {
      log: { debug() {}, info() {}, warn() {}, error() {} },
      supports: (f: string) => f === "dataProviders@1",
      dataProviders: {
        discover: () => [
          {
            id: "pricing-dataset",
            category: "dataset",
            revision: "1",
            schema: { fields: [{ name: "sku", ty: "text" }, { name: "price", ty: "float" }] },
          },
        ],
        get: async () => null,
        onDidChange: () => ({ dispose() {} }),
      },
    } as unknown as BundleHost;
    const session = createWorkbookSession(host);
    const Panel = makeDatasetsPanel(host, session);
    let tree!: ReactTestRenderer;
    act(() => {
      tree = create(createElement(Panel));
    });
    const name = byData(tree, "data-dataset-name")[0];
    expect(text(name)).toContain("Pricing");
    expect(name.props.title).toBe("pricing-dataset");
    expect(JSON.stringify(byData(tree, "data-dataset-fields")[0].props.children)).toContain(
      "2 fields: sku, price",
    );
  });
});
