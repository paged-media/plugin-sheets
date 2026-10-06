// The paged.sheet OBJECT MODEL (ADR 323) over a REAL headless host: core
// via canvas-wasm (real document undo, real plugin metadata, real parts)
// and the REAL sheet engine wasm. Every read and write goes through the
// shared registry (`h.objects`) — the door the Node CLI, a Boa bridge, a
// data binding and a schema-driven panel field all use.
//
// DUAL-GATED like the other real-engine suites: skipped without the wasm
// artifact locally, FAILED under REQUIRE_REAL_ENGINE=1 (CI).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { HeadlessHost } from "@paged-media/plugin-sdk";
import type { BundleHost, ObjectValue, ProviderRecordSet } from "@paged-media/plugin-api";

import { sheetBundle } from "../src";
import { contributeObjectModel, SHEET_KINDS } from "../src/object-model";
import { createWorkbookSession, type WorkbookSession } from "../src/session";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  authorWorkbook,
  blankPageIdml,
  ENGINE_BUILT,
  openHost,
  settle,
  sheetHost,
} from "./perf/workload";

const P = "plugin:media.paged.sheet";
const HERE = dirname(fileURLToPath(import.meta.url));
const TABLES_XLSX = join(HERE, "..", "..", "..", "corpus/xlsx-corpus/07-tables.xlsx");

if (process.env.REQUIRE_REAL_ENGINE === "1" && !ENGINE_BUILT) {
  describe("object model (wasm artifact) — REQUIRED", () => {
    it("FAILS: REQUIRE_REAL_ENGINE=1 but the wasm artifact is missing", () => {
      throw new Error("build it with `bash scripts/build-wasm.sh`");
    });
  });
}

const val = (v: ObjectValue): unknown => (v.kind === "value" ? v.value : v);

describe.skipIf(!ENGINE_BUILT)("paged.sheet object model [sheet.om.model]", () => {
  vi.setConfig({ testTimeout: 60_000 });
  let h: HeadlessHost;
  let host: BundleHost;
  let s: WorkbookSession;
  let om: { dispose(): void };

  /** 1..9 in A1:C3, row-major. */
  async function bootWith(bytes?: Uint8Array): Promise<void> {
    h = await openHost();
    await h.load(blankPageIdml());
    host = sheetHost(h);
    s = createWorkbookSession(host);
    om = contributeObjectModel(host, s);
    await s.import(bytes ?? (await authorWorkbook(3, 3, (r, c) => String(r * 3 + c + 1))), "book.xlsx");
  }

  async function place(range = "Sheet1!A1:C3"): Promise<string> {
    const r = (await h.objects.invoke("media.paged.sheet.command.lowerToFrame", { range })) as {
      frame: string;
    };
    await settle();
    return r.frame;
  }

  beforeEach(async () => {
    await bootWith();
  });
  afterEach(() => {
    om?.dispose();
    s?.dispose();
    h?.dispose();
  });

  it("contributes the seven kinds, each with schema rows", async () => {
    const kinds = (await h.objects.kinds()).filter((k) => k.owner === "media.paged.sheet");
    expect(kinds.map((k) => k.kind).sort()).toEqual(
      [...SHEET_KINDS].map((k) => `${P}/${k}`).sort(),
    );
    const cell = await h.objects.schema(`${P}/cell`);
    expect(cell.map((r) => r.path)).toEqual(
      expect.arrayContaining(["value", "formula", "input", "display", "numberFormat", "bold", "fill"]),
    );
    expect(cell.find((r) => r.path === "display")?.access).toBe("derived");
    const chart = await h.objects.schema(`${P}/chart/0`);
    expect(chart.find((r) => r.path === "kind")?.type).toMatchObject({ kind: "enum" });
  });

  it("reads cells, ranges, sheets and the workbook", async () => {
    expect(val(await h.objects.get(`${P}/cell/Sheet1!B2`, "value"))).toBe("5");
    expect(val(await h.objects.get(`${P}/cell/Sheet1!B2`, "display"))).toBe("5");
    expect(val(await h.objects.get(`${P}/cell/Sheet1!B2`, "formula"))).toBeNull();
    expect(val(await h.objects.get(`${P}/range/Sheet1!A1:B2`, "values"))).toEqual([
      ["1", "2"],
      ["4", "5"],
    ]);
    expect(val(await h.objects.get(`${P}/sheet/Sheet1`, "usedRange"))).toBe("A1:C3");
    expect(val(await h.objects.get(`${P}/workbook/main`, "sheets"))).toEqual(["Sheet1"]);
    expect(await h.objects.query(`${P}/sheet`)).toEqual([`${P}/sheet/Sheet1`]);
    expect(await h.objects.query(`${P}/sheet[name="Sheet1"]`)).toEqual([`${P}/sheet/Sheet1`]);
    const bad = await h.objects.get(`${P}/cell/Nope!A1`, "value");
    expect(bad.kind).toBe("refused");
  });

  it("refuses a write before the workbook is placed (no host for the label)", async () => {
    const r = await h.objects.set(`${P}/cell/Sheet1!A1`, "value", "7");
    expect(r.applied).toBe(false);
    expect(r.reason).toMatch(/place/);
    expect(val(await h.objects.get(`${P}/cell/Sheet1!A1`, "value"))).toBe("1");
  });

  it("a set is ONE document undo step; undo and redo move the workbook [sheet.om.undo]", async () => {
    const frame = await place();
    expect(h.objects.schema).toBeDefined();
    const r = await h.objects.set(`${P}/cell/Sheet1!B2`, "value", "42");
    expect(r).toMatchObject({ applied: true, undoSteps: 1 });
    expect(val(await h.objects.get(`${P}/cell/Sheet1!B2`, "value"))).toBe("42");
    // The label on the hosting frame names the live version.
    expect(s.objectBridge().hostFrames()).toContain(frame.replace(/^textFrame:/, ""));

    await host.document.undo();
    await settle();
    expect(val(await h.objects.get(`${P}/cell/Sheet1!B2`, "value"))).toBe("5");
    await host.document.redo();
    await settle();
    expect(val(await h.objects.get(`${P}/cell/Sheet1!B2`, "value"))).toBe("42");
  });

  it("formulas, number formats and styles", async () => {
    await place();
    expect((await h.objects.set(`${P}/cell/Sheet1!D1`, "formula", "=A1+C3")).applied).toBe(true);
    expect(val(await h.objects.get(`${P}/cell/Sheet1!D1`, "value"))).toBe("10");
    expect(val(await h.objects.get(`${P}/cell/Sheet1!D1`, "formula"))).toBe("=A1+C3");
    const fmt = await h.objects.batch([
      { op: "set", address: `${P}/cell/Sheet1!B2`, path: "numberFormat", value: "0.00" },
      { op: "set", address: `${P}/cell/Sheet1!B2`, path: "bold", value: true },
      { op: "set", address: `${P}/cell/Sheet1!B2`, path: "hAlign", value: "center" },
    ]);
    expect(fmt).toMatchObject({ applied: true, undoSteps: 1 });
    expect(val(await h.objects.get(`${P}/cell/Sheet1!B2`, "display"))).toBe("5.00");
    expect(val(await h.objects.get(`${P}/cell/Sheet1!B2`, "bold"))).toBe(true);
    expect(val(await h.objects.get(`${P}/cell/Sheet1!B2`, "hAlign"))).toBe("center");
    // Formula fill over a range: relative references follow each cell.
    expect((await h.objects.set(`${P}/range/Sheet1!E1:E3`, "formula", "=A1*2")).applied).toBe(true);
    expect(val(await h.objects.get(`${P}/range/Sheet1!E1:E3`, "values"))).toEqual([["2"], ["8"], ["14"]]);
  });

  it("validates before anything applies", async () => {
    await place();
    const wrongType = await h.objects.set(`${P}/cell/Sheet1!A1`, "value", 5);
    expect(wrongType).toMatchObject({ applied: false, code: "invalidValue" });
    const formulaAsValue = await h.objects.set(`${P}/cell/Sheet1!A1`, "value", "=1+1");
    expect(formulaAsValue.applied).toBe(false);
    const readOnly = await h.objects.set(`${P}/cell/Sheet1!A1`, "display", "x");
    expect(readOnly.applied).toBe(false);
    // All-or-nothing: the good op in a batch with a bad one does not land.
    const mixed = await h.objects.batch([
      { op: "set", address: `${P}/cell/Sheet1!A1`, path: "value", value: "100" },
      { op: "set", address: `${P}/cell/Sheet1!A2`, path: "numberFormat", value: 3 },
    ]);
    expect(mixed.applied).toBe(false);
    expect(val(await h.objects.get(`${P}/cell/Sheet1!A1`, "value"))).toBe("1");
  });

  it("a 100-cell range set is ONE batch: one engine write, one save, one commit [sheet.om.perf]", async () => {
    await place();
    await h.objects.set(`${P}/cell/Sheet1!A1`, "value", "1"); // the baseline version is stored once
    const engine = s.state().engine!;
    const count = { setCells: 0, setCell: 0, save: 0, commit: 0, parts: 0 };
    const wrap = <T extends object, K extends keyof T>(o: T, k: K, key: keyof typeof count) => {
      const f = o[k] as unknown as (...a: unknown[]) => unknown;
      (o as Record<K, unknown>)[k] = (...a: unknown[]) => {
        count[key] += 1;
        return f.apply(o, a);
      };
    };
    wrap(engine, "setCells", "setCells");
    wrap(engine, "setCell", "setCell");
    wrap(engine, "saveXlsx", "save");
    wrap(h.objects.core!, "commit", "commit");
    wrap(host.parts, "write", "parts");
    const values = Array.from({ length: 10 }, (_, r) => Array.from({ length: 10 }, (_, c) => String(r * 10 + c)));
    const r = await h.objects.set(`${P}/range/Sheet1!A10:J19`, "values", values);
    expect(r).toMatchObject({ applied: true, undoSteps: 1 });
    // The budget (never raise it): one engine write, one save, one commit,
    // two content-addressed parts (the bytes and the meta).
    expect(count).toEqual({ setCells: 1, setCell: 0, save: 1, commit: 1, parts: 2 });
    expect(val(await h.objects.get(`${P}/cell/Sheet1!J19`, "value"))).toBe("99");
  });

  it("sheets: create, rename, freeze, delete", async () => {
    await place();
    expect((await h.objects.batch([{ op: "create", kind: `${P}/sheet`, props: { name: "Data" } }])).applied).toBe(true);
    expect(await h.objects.query(`${P}/sheet`)).toEqual([`${P}/sheet/Sheet1`, `${P}/sheet/Data`]);
    expect((await h.objects.set(`${P}/sheet/Data`, "name", "Facts")).applied).toBe(true);
    expect((await h.objects.set(`${P}/sheet/Facts`, "freezeRows", 1)).applied).toBe(true);
    expect(val(await h.objects.get(`${P}/sheet/Facts`, "freezeRows"))).toBe(1);
    expect((await h.objects.batch([{ op: "delete", address: `${P}/sheet/Facts` }])).applied).toBe(true);
    expect(await h.objects.query(`${P}/sheet`)).toEqual([`${P}/sheet/Sheet1`]);
  });

  it("named ranges: create, read, retarget, delete", async () => {
    await place();
    const c = await h.objects.batch([
      { op: "create", kind: `${P}/namedRange`, props: { name: "Totals", refersTo: "Sheet1!C1:C3" } },
    ]);
    expect(c.applied).toBe(true);
    expect(await h.objects.query(`${P}/namedRange`)).toEqual([`${P}/namedRange/Totals`]);
    expect(val(await h.objects.get(`${P}/namedRange/Totals`, "refersTo"))).toBe("Sheet1!$C$1:$C$3");
    await h.objects.set(`${P}/cell/Sheet1!D1`, "formula", "=SUM(Totals)");
    expect(val(await h.objects.get(`${P}/cell/Sheet1!D1`, "value"))).toBe("18");
    expect((await h.objects.set(`${P}/namedRange/Totals`, "refersTo", "Sheet1!A1:A3")).applied).toBe(true);
    expect(val(await h.objects.get(`${P}/cell/Sheet1!D1`, "value"))).toBe("12");
    expect((await h.objects.batch([{ op: "delete", address: `${P}/namedRange/Totals` }])).applied).toBe(true);
    expect(await h.objects.query(`${P}/namedRange`)).toEqual([]);
  });

  it("charts: create, read series and options, patch [sheet.om.chart]", async () => {
    await place();
    const created = await h.objects.batch([
      {
        op: "create",
        kind: `${P}/chart`,
        props: {
          kind: "column",
          title: "Sales",
          series: [{ values: "Sheet1!B1:B3", categories: "Sheet1!A1:A3", name: "B", color: "" }],
        },
      },
    ]);
    expect(created.applied).toBe(true);
    const charts = await h.objects.query(`${P}/chart`);
    expect(charts).toEqual([`${P}/chart/0`]);
    expect(val(await h.objects.get(`${P}/chart/0`, "title"))).toBe("Sales");
    expect(val(await h.objects.get(`${P}/chart/0`, "series"))).toEqual([
      { values: "Sheet1!B1:B3", categories: "Sheet1!A1:A3", name: "B", color: "" },
    ]);
    const patched = await h.objects.batch([
      { op: "set", address: `${P}/chart/0`, path: "kind", value: "line" },
      { op: "set", address: `${P}/chart/0`, path: "legend", value: false },
      { op: "set", address: `${P}/chart/0`, path: "valueAxisMax", value: 100 },
    ]);
    expect(patched).toMatchObject({ applied: true, undoSteps: 1 });
    expect(val(await h.objects.get(`${P}/chart/0`, "kind"))).toBe("line");
    expect(val(await h.objects.get(`${P}/chart/0`, "valueAxisMax"))).toBe(100);
    const bad = await h.objects.set(`${P}/chart/0`, "kind", "sparkle");
    expect(bad.applied).toBe(false);
    // The chart ops live in the version meta: undo takes the patch back.
    await host.document.undo();
    await settle();
    expect(val(await h.objects.get(`${P}/chart/0`, "kind"))).toBe("column");
  });

  it("typed commands are listed with value-typed args", async () => {
    const cmds = (await h.objects.commands()).filter((c) => c.owner === "media.paged.sheet");
    expect(cmds.length).toBeGreaterThanOrEqual(15);
    const lower = cmds.find((c) => c.id === "media.paged.sheet.command.lowerToFrame");
    expect(lower?.args).toMatchObject({ kind: "struct" });
    await expect(h.objects.invoke("media.paged.sheet.command.lowerToFrame", { range: 3 })).rejects.toThrow(
      /invalidValue/,
    );
  });

  it("a structural edit through its typed command", async () => {
    await place();
    await h.objects.invoke("media.paged.sheet.command.insertRows", { sheet: "Sheet1", at: 0, count: 1 });
    expect(val(await h.objects.get(`${P}/cell/Sheet1!A2`, "value"))).toBe("1");
  });
});

describe.skipIf(!ENGINE_BUILT)("paged.sheet tables [sheet.om.table]", () => {
  vi.setConfig({ testTimeout: 60_000 });
  it("reads a structured table; writes are refused (the part re-emits verbatim)", async () => {
    const h = await openHost();
    try {
      await h.load(blankPageIdml());
      const host = sheetHost(h);
      const s = createWorkbookSession(host);
      const om = contributeObjectModel(host, s);
      await s.import(new Uint8Array(readFileSync(TABLES_XLSX)), "tables.xlsx");
      const tables = await h.objects.query(`${P}/table`);
      expect(tables.length).toBeGreaterThan(0);
      const t = tables[0]!;
      const cols = val(await h.objects.get(t, "columns")) as string[];
      expect(cols.length).toBeGreaterThan(0);
      expect(typeof val(await h.objects.get(t, "range"))).toBe("string");
      expect(typeof val(await h.objects.get(t, "totalsRow"))).toBe("boolean");
      const w = await h.objects.set(t, "name", "Renamed");
      expect(w.applied).toBe(false);
      om.dispose();
      s.dispose();
    } finally {
      h.dispose();
    }
  });
});

describe.skipIf(!ENGINE_BUILT)("paged.sheet datasets, both directions [sheet.om.dataset]", () => {
  vi.setConfig({ testTimeout: 60_000 });

  const records = (rows: [string, number][]): ProviderRecordSet => ({
    schema: { fields: [{ name: "region", ty: "text" }, { name: "units", ty: "float" }] },
    columns: [rows.map((r) => r[0]), rows.map((r) => r[1])],
    rowCount: rows.length,
  });

  it("a live dataset re-pulls on every provider revision", async () => {
    const h = await openHost();
    try {
      await h.load(blankPageIdml());
      const host = sheetHost(h);
      const s = createWorkbookSession(host);
      const om = contributeObjectModel(host, s);
      let current = records([["North", 1], ["South", 2], ["East", 3]]);
      const provider = h.dataProviders.register({
        id: "media.paged.data.dataset.sales",
        category: "dataset",
        schema: current.schema,
        revision: "r1",
        getSnapshot: () => current,
      });
      await h.objects.invoke("media.paged.sheet.command.sheetFromDataset", {
        providerId: "media.paged.data.dataset.sales",
        live: true,
      });
      expect(val(await h.objects.get(`${P}/cell/Sheet1!A4`, "value"))).toBe("East");
      current = records([["West", 10]]);
      provider.update("r2");
      await settle();
      expect(val(await h.objects.get(`${P}/range/Sheet1!A2:B4`, "values"))).toEqual([
        ["West", "10"],
        ["", ""],
        ["", ""],
      ]);
      expect(s.state().dataSource).toMatchObject({ revision: "r2", stale: false, live: true });
      om.dispose();
      s.dispose();
    } finally {
      h.dispose();
    }
  });

  it("a sheet range publishes as a dataset; edits bump its revision", async () => {
    const h = await openHost();
    try {
      await h.load(blankPageIdml());
      const host = sheetHost(h);
      const s = createWorkbookSession(host);
      const om = contributeObjectModel(host, s);
      await s.import(
        await authorWorkbook(3, 2, (r, c) => (r === 0 ? ["name", "qty"][c]! : c === 0 ? `item${r}` : String(r * 5))),
        "pub.xlsx",
      );
      const id = (await h.objects.invoke("media.paged.sheet.command.publishDataset", {
        name: "stock",
        range: "Sheet1!A1:B3",
        header: true,
      })) as string;
      expect(id).toBe("media.paged.sheet.dataset.stock");
      const info = h.dataProviders.discover("dataset").find((d) => d.id === id)!;
      expect(info.schema.fields).toEqual([
        { name: "name", ty: "text", nullable: true },
        { name: "qty", ty: "float", nullable: true },
      ]);
      const snap = await h.dataProviders.get(id);
      expect(snap?.records.columns).toEqual([
        ["item1", "item2"],
        [5, 10],
      ]);
      const seen: string[] = [];
      h.dataProviders.onDidChange(id, (r) => seen.push(r));
      s.editCell(0, 1, 1, "7");
      await settle();
      expect(seen.length).toBeGreaterThan(0);
      expect((await h.dataProviders.get(id))?.records.columns[1]).toEqual([7, 10]);
      om.dispose();
      s.dispose();
    } finally {
      h.dispose();
    }
  });
});

describe.skipIf(!ENGINE_BUILT)("the real bundle boots headless with its object model [sheet.om.headless]", () => {
  vi.setConfig({ testTimeout: 60_000 });
  it("activate registers the kinds and typed commands; get/set/batch/undo work", async () => {
    const h = await openHost();
    try {
      await h.load(blankPageIdml());
      const d = h.loadBundle(sheetBundle);
      await h.objects.invoke("media.paged.sheet.command.newWorkbook", {});
      await h.objects.invoke("media.paged.sheet.command.lowerToFrame", { range: "Sheet1!A1:B2" });
      await settle();
      const r = await h.objects.batch([
        { op: "set", address: `${P}/cell/Sheet1!A1`, path: "value", value: "hello" },
        { op: "set", address: `${P}/cell/Sheet1!B1`, path: "formula", value: '=A1&"!"' },
      ]);
      expect(r).toMatchObject({ applied: true, undoSteps: 1 });
      expect(val(await h.objects.get(`${P}/cell/Sheet1!B1`, "value"))).toBe("hello!");
      await h.host.document.undo();
      await settle();
      expect(val(await h.objects.get(`${P}/cell/Sheet1!B1`, "value"))).toBe("");
      d.dispose();
    } finally {
      h.dispose();
    }
  });
});
