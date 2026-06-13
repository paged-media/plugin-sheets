// sheet.data-provider.consumer (S-15) — the consumer half of the
// data-provider contract: discoverDatasets() lists the governed datasets
// the platform offers, and sourceFromDataset() pulls a provider's resolved
// snapshot + seeds a fresh sheet (row 0 = header, rows 1.. = the records).
// No DOM, no wasm: bootEmptyEngine is mocked to a fake engine that captures
// setCell calls, and a fake host.dataProviders returns a small RecordSet.
//
// ISOLATION (§2.1): the consumer touches ONLY host.dataProviders — these
// tests never import paged.data / any sibling plugin; they prove the
// session maps the neutral contract shape to the engine. The cell-value
// encoding is exercised in BOTH the plain-JS and the tagged `{ t, v }`
// forms (the contract permits either — the defensive cellToString handles
// both; a follow-up should standardize one).

import { createElement } from "react";
import {
  act,
  create,
  type ReactTestInstance,
  type ReactTestRenderer,
} from "react-test-renderer";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  BundleHost,
  DataProviderInfo,
  DataProviderSnapshot,
  Disposable,
} from "@paged-media/plugin-api";

import type { SheetEngine } from "../src";
import {
  cellToString,
  createWorkbookSession,
  makeDatasetsPanel,
} from "../src";

// ---- the mocked engine module ------------------------------------------
//
// bootEmptyEngine() is what sourceFromDataset boots a fresh workbook with;
// we replace it with a fake that records every setCell so we can assert the
// seeded header + rows without any real wasm. setCellCalls is a hoist-safe
// module-level sink the mock factory closes over (vi.mock is hoisted).

const setCellCalls: Array<[number, number, number, string]> = [];
let bootShouldThrow = false;

function makeFakeEngine(): SheetEngine {
  return {
    loadXlsx() {},
    saveXlsx: () => new Uint8Array(),
    setCell: (sheet, row, col, input) => {
      setCellCalls.push([sheet, row, col, input]);
      return { changed: [{ sheet, row, col, display: input }] };
    },
    getCellDisplay: () => "",
    getCellInput: () => "",
    sortRange: () => ({ changed: [], edits: [] }),
    findAll: () => [],
    replaceAll: () => ({ occurrences: 0, changed: [], edits: [], skipped: [] }),
    getRangeLowered: () => ({ cols: [], rows: [], rules: { h: [], v: [] }, merges: [] }),
    getRangeValues: () => [],
    paginate: () => [],
    getGridScene: () => ({
      viewport: { firstRow: 0, firstCol: 0, rows: 0, cols: 0, xOffsets: [0], yOffsets: [0] },
      cells: [],
      styles: [],
      gridlines: { h: [], v: [] },
      selection: null,
    }),
    setGridSelection() {},
    // The seeded extent the panel/range default reads back. 3 cols × 3 rows
    // (header + 2 data rows) matches the fixture below.
    listSheets: () => [{ id: 0, name: "Sheet1", rows: 3, cols: 3 }],
    listCharts: () => [],
    listFreezePanes: () => [],
    listFunctions: () => [],
    getChartGeometry: () => ({ widthPt: 0, heightPt: 0, prims: [] }),
    dispose() {},
  };
}

vi.mock("../src/engine", async () => {
  const actual = await vi.importActual<typeof import("../src/engine")>(
    "../src/engine",
  );
  return {
    ...actual,
    bootEmptyEngine: vi.fn(async () => {
      if (bootShouldThrow) throw new Error(actual.ENGINE_NOT_BUILT);
      return makeFakeEngine();
    }),
  };
});

// ---- the fixtures ------------------------------------------------------

/** A 3-field schema (the `ty` key per the contract — NOT `type`). */
function info(): DataProviderInfo {
  return {
    id: "fct_products",
    category: "dataset",
    revision: "rev-1",
    schema: {
      fields: [
        { name: "sku", ty: "text" },
        { name: "qty", ty: "int" },
        { name: "active", ty: "bool" },
      ],
    },
  };
}

/** A snapshot whose columns mix PLAIN JS values and the tagged `{ t, v }`
 *  form — exercising the defensive cellToString in one go. Column-major:
 *  columns[c][r] is the cell at data-row r, schema col c. 2 data rows. */
function snapshot(): DataProviderSnapshot {
  return {
    id: "fct_products",
    revision: "rev-1",
    records: {
      schema: info().schema,
      columns: [
        // col 0 (sku) — plain strings
        ["A-1", "B-2"],
        // col 1 (qty) — tagged numbers `{ t: "number", v }`
        [
          { t: "number", v: 10 },
          { t: "number", v: 20 },
        ],
        // col 2 (active) — plain boolean + tagged null
        [true, { t: "null", v: null }],
      ],
      rowCount: 2,
    },
  };
}

/** A recording fake of the slice of host the consumer touches: log,
 *  supports("dataProviders@1"), shell.openPanel, and host.dataProviders
 *  (discover/get/onDidChange). `wired` toggles the surface presence to
 *  exercise the honest-defer path. */
function fakeHost(opts?: { wired?: boolean }) {
  const wired = opts?.wired ?? true;
  const changeListeners = new Map<string, (revision: string) => void>();
  const getCalls: string[] = [];
  const dataProviders = {
    register: vi.fn(() => ({ update() {}, dispose() {} })),
    discover: vi.fn((category?: string) =>
      category === "dataset" ? [info()] : [],
    ),
    get: vi.fn(async (id: string): Promise<DataProviderSnapshot | null> => {
      getCalls.push(id);
      return id === "fct_products" ? snapshot() : null;
    }),
    onDidChange: vi.fn(
      (id: string, listener: (revision: string) => void): Disposable => {
        changeListeners.set(id, listener);
        return { dispose: () => changeListeners.delete(id) };
      },
    ),
  };
  const openedPanels: string[] = [];
  const host = {
    log: { debug() {}, info() {}, warn() {}, error() {} },
    supports: (f: string) => (wired ? f === "dataProviders@1" : false),
    // When NOT wired the surface itself is absent (the consumer guards on
    // both supports() AND the surface existing).
    dataProviders: wired ? dataProviders : undefined,
    shell: { openPanel: (id: string) => openedPanels.push(id), closePanel() {} },
  } as unknown as BundleHost;
  return { host, dataProviders, changeListeners, getCalls, openedPanels };
}

beforeEach(() => {
  setCellCalls.length = 0;
  bootShouldThrow = false;
});

describe("sheet_data_provider_consumer: cellToString (contract encoding)", () => {
  it("unwraps the tagged `{ t, v }` form to its value", () => {
    expect(cellToString({ t: "text", v: "hi" })).toBe("hi");
    expect(cellToString({ t: "number", v: 42 })).toBe("42");
    expect(cellToString({ t: "null", v: null })).toBe("");
  });

  it("uses a plain JS value directly", () => {
    expect(cellToString("hi")).toBe("hi");
    expect(cellToString(42)).toBe("42");
    expect(cellToString(true)).toBe("TRUE");
    expect(cellToString(false)).toBe("FALSE");
    expect(cellToString(null)).toBe("");
    expect(cellToString(undefined)).toBe("");
  });
});

describe("sheet_data_provider_consumer: discoverDatasets", () => {
  it("returns the registry's discovered datasets in the dataset category", () => {
    const { host, dataProviders } = fakeHost();
    const session = createWorkbookSession(host);
    const found = session.discoverDatasets();
    expect(found.map((d) => d.id)).toEqual(["fct_products"]);
    expect(dataProviders.discover).toHaveBeenCalledWith("dataset");
  });

  it("returns [] when no registry is wired (honest defer, §2.1)", () => {
    const { host, dataProviders } = fakeHost({ wired: false });
    const session = createWorkbookSession(host);
    expect(session.discoverDatasets()).toEqual([]);
    // The absent surface was never touched.
    expect(dataProviders.discover).not.toHaveBeenCalled();
  });
});

describe("sheet_data_provider_consumer: sourceFromDataset", () => {
  it("pulls the snapshot + seeds sheet 0 (header row + records)", async () => {
    const { host, getCalls } = fakeHost();
    const session = createWorkbookSession(host);

    await session.sourceFromDataset("fct_products");

    expect(getCalls).toEqual(["fct_products"]);

    // Row 0 — the header from the schema field NAMES.
    expect(setCellCalls).toContainEqual([0, 0, 0, "sku"]);
    expect(setCellCalls).toContainEqual([0, 0, 1, "qty"]);
    expect(setCellCalls).toContainEqual([0, 0, 2, "active"]);

    // Rows 1.. — the column-major records, each via cellToString (the
    // tagged `{ t: "number" }` qty unwrapped; the plain boolean cased;
    // the tagged null blanked).
    expect(setCellCalls).toContainEqual([0, 1, 0, "A-1"]);
    expect(setCellCalls).toContainEqual([0, 2, 0, "B-2"]);
    expect(setCellCalls).toContainEqual([0, 1, 1, "10"]);
    expect(setCellCalls).toContainEqual([0, 2, 1, "20"]);
    expect(setCellCalls).toContainEqual([0, 1, 2, "TRUE"]);
    expect(setCellCalls).toContainEqual([0, 2, 2, ""]);
  });

  it("sets state.engine / activeSheet / dataSource + emits a change", async () => {
    const { host } = fakeHost();
    const session = createWorkbookSession(host);
    let changes = 0;
    session.onDidChange(() => changes++);

    await session.sourceFromDataset("fct_products");

    const st = session.state();
    expect(st.engine).not.toBeNull();
    expect(st.activeSheet).toBe(0);
    expect(st.fileName).toBe("fct_products");
    expect(st.dataSource).toEqual({
      providerId: "fct_products",
      revision: "rev-1",
      stale: false,
    });
    expect(changes).toBeGreaterThan(0);
  });

  it("subscribes to onDidChange and marks the sheet stale on a new revision (no auto-refetch)", async () => {
    const { host, changeListeners, getCalls } = fakeHost();
    const session = createWorkbookSession(host);

    await session.sourceFromDataset("fct_products");
    expect(changeListeners.has("fct_products")).toBe(true);
    expect(getCalls).toEqual(["fct_products"]); // one pull so far

    // The provider announces a newer revision.
    changeListeners.get("fct_products")!("rev-2");

    // The sheet is flagged stale — but NO second get() (§1.1: no
    // auto-refetch; re-sourcing is an explicit author action).
    expect(session.state().dataSource).toEqual({
      providerId: "fct_products",
      revision: "rev-1",
      stale: true,
    });
    expect(getCalls).toEqual(["fct_products"]);
  });

  it("honest defer when no registry is wired: no get, no seed (§2.1)", async () => {
    const { host, dataProviders } = fakeHost({ wired: false });
    const session = createWorkbookSession(host);

    await session.sourceFromDataset("fct_products");

    expect(dataProviders.get).not.toHaveBeenCalled();
    expect(setCellCalls).toEqual([]);
    expect(session.state().engine).toBeNull();
    expect(session.state().dataSource).toBeNull();
  });

  it("no-ops (no seed) when the provider no longer exists", async () => {
    const { host } = fakeHost();
    const session = createWorkbookSession(host);

    await session.sourceFromDataset("does_not_exist");

    expect(setCellCalls).toEqual([]);
    expect(session.state().engine).toBeNull();
    expect(session.state().dataSource).toBeNull();
  });

  it("surfaces a boot failure honestly (bootError set, no dataSource)", async () => {
    bootShouldThrow = true;
    const { host } = fakeHost();
    const session = createWorkbookSession(host);

    await session.sourceFromDataset("fct_products");

    expect(session.state().bootError).not.toBeNull();
    expect(session.state().dataSource).toBeNull();
    expect(setCellCalls).toEqual([]);
  });
});

/** Walk the test tree collecting nodes carrying the given data-* prop. */
function byData(
  tree: ReactTestRenderer,
  key: string,
  value?: string,
): ReactTestInstance[] {
  return tree.root.findAll(
    (n) =>
      n.props != null &&
      n.props[key] !== undefined &&
      (value === undefined || n.props[key] === value),
  );
}

describe("sheet_data_provider_consumer: datasets panel", () => {
  it("lists each discovered dataset (id + field count) with a Source button", () => {
    const { host } = fakeHost();
    const session = createWorkbookSession(host);
    const Panel = makeDatasetsPanel(host, session);
    let tree!: ReactTestRenderer;
    act(() => {
      tree = create(createElement(Panel));
    });

    const rows = byData(tree, "data-dataset-row");
    expect(rows).toHaveLength(1);
    expect(rows[0].props["data-dataset-id"]).toBe("fct_products");

    // The field-count meta is shown (3 fields).
    const meta = byData(tree, "data-dataset-fields");
    expect(meta).toHaveLength(1);
    expect(meta[0].props["data-dataset-fields"]).toBe(3);

    // The Source button is present and sources on click.
    const sourceBtns = byData(tree, "data-dataset-source");
    expect(sourceBtns).toHaveLength(1);
    const spy = vi.spyOn(session, "sourceFromDataset").mockResolvedValue();
    act(() => {
      sourceBtns[0].props.onClick();
    });
    expect(spy).toHaveBeenCalledWith("fct_products");
  });

  it("shows the honest empty state when no datasets are discovered", () => {
    const { host } = fakeHost({ wired: false });
    const session = createWorkbookSession(host);
    const Panel = makeDatasetsPanel(host, session);
    let tree!: ReactTestRenderer;
    act(() => {
      tree = create(createElement(Panel));
    });

    expect(byData(tree, "data-dataset-row")).toHaveLength(0);
    const empty = byData(tree, "data-datasets-empty");
    expect(empty).toHaveLength(1);
    const text = JSON.stringify(empty[0].props.children);
    expect(text).toContain("paged.data");
  });
});
