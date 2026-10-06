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

// The paged.sheet OBJECT MODEL (ADR 323; plugin-sdk DESIGN.md §21).
//
// Seven kinds, addressed `plugin:media.paged.sheet/<kind>/<id>`:
//
//   workbook    `workbook/main`            the one workbook of the document
//   sheet       `sheet/<name>`
//   cell        `cell/<sheet>!<A1>`        value, formula, input, display,
//                                          number format, font / fill / alignment
//   range       `range/<sheet>!<A1:B2>`    values (2-D), inputs, formula fill,
//                                          display, the same style rows
//   namedRange  `namedRange/<name>` or `namedRange/<sheet>!<name>`
//   table       `table/<name>`             read-only (the xlsx table part
//                                          re-emits verbatim, spec §10.2)
//   chart       `chart/<index>`            kind, title, legend, series ranges,
//                                          axis titles and bounds
//
// A sheet name with a space or `%` is percent-escaped in an id, so an
// address never breaks a selector (`sheet/My%20Sheet`).
//
// WRITES (rule 2, the label-hash pattern). The workbook lives in the
// engine; a write applies there, then saves the workbook as a
// content-addressed VERSION — `versions/<hash>.xlsx` plus
// `versions/<hash>.json` (name + the chart-op journal, since authored and
// patched charts are page-side only and never reach the xlsx) — and the
// LABEL `x-paged:media.paged.sheet = {v:1,data:{wb:<hash>}}` names it. The
// label is the DOCUMENT's (`doc`, the designmap Label InDesign keeps), not a
// frame's: the workbook is document-scoped (one per document), and core
// keeps ONE label per plugin per host — a frame's already holds the
// placement binding. So a write answers a `state` write (SDK 0.2.43,
// DESIGN.md §21.8) hosted on `doc` under the SUB-KEY
// `x-paged:media.paged.sheet.wb`: the registry writes the content-addressed
// parts first (harmless if the commit then fails), merges `wb` into the
// document envelope (other sub-keys survive) and commits ONE batch = one
// undo step. The layout `{v:1,data:{wb}}` is the one the earlier direct
// `setDocumentMetadata` wrote, so labels from before read unchanged.
// Every op of a batch, across all seven kinds, reaches ONE plugin-level
// planner (`batch`): one engine pass, one saved version, one label.
// Document undo reverts the label; the follower here reloads the version
// the label now names (the baseline version stands for "no label"). The
// `workbook.xlsx` part stays what it was: a cache, refreshed (debounced)
// after every change. A document open prefers the version its label names.
//
// `hostOf` still names the frame a cell is placed in — where ADR 559 puts a
// binding's labels — and never a table cell (InDesign renumbers ids and
// drops cell labels).
//
// ALL spreadsheet semantics stay in the engine: addresses resolve through
// `resolveRange`, values are the engine's typed raw values, formats and
// styles are its `setStyle` patch, formula fill is its fill door.

import type {
  Address,
  BundleHost,
  DataProviderHandle,
  ObjectKindContribution,
  ObjectModelHandle,
  ObjectOp,
  ObjectValue,
  ObjectWrite,
  PartChange,
  PropertySchema,
  ProviderField,
  StructField,
  ProviderRecordSet,
  TypedCommandContribution,
  ValueType,
} from "@paged-media/plugin-api";

import manifest from "../manifest.json";

import type {
  CellStylePatch,
  ChartPatch,
  ChartSpec,
  RawValue,
  ResolvedRange,
  SheetEngine,
  StructuralEditKind,
} from "./engine";
import type { CellRegion, ObjectBridge, WorkbookSession } from "./session";

export const PLUGIN_ID = manifest.id;
export const SHEET_KINDS = ["workbook", "sheet", "cell", "range", "namedRange", "table", "chart"] as const;
export type SheetKind = (typeof SHEET_KINDS)[number];

/** The plugin's ONE label per host (core's rule: exactly `x-paged:<id>`,
 *  envelope `{v, data}`); on `doc` it names the live workbook version. */
export const VERSION_LABEL_KEY = `x-paged:${PLUGIN_ID}`;
/** The sub-key a `state` write sets: the registry merges it into
 *  `data.wb` of the document envelope. */
export const VERSION_SUB_KEY = `${VERSION_LABEL_KEY}.wb`;
const VERSIONS = "versions/";
const WORKBOOK_ID = "main";

/** The chart kinds the engine accepts (its `chartKinds()`; pinned by the
 *  engine spec so this list cannot drift from Rust). */
export const CHART_KINDS = [
  "column",
  "stackedColumn",
  "bar",
  "stackedBar",
  "line",
  "area",
  "scatter",
  "pie",
  "donut",
  "radar",
] as const;
const H_ALIGNS = ["general", "left", "center", "right", "fill", "justify", "centerContinuous", "distributed"];
const V_ALIGNS = ["top", "center", "bottom", "justify", "distributed"];

// ------------------------------------------------------------- schemas

const text: ValueType = { kind: "text" };
const bool: ValueType = { kind: "bool" };
const int: ValueType = { kind: "number", integer: true };
const grid: ValueType = { kind: "list", of: { kind: "list", of: text } };
const series: ValueType = {
  kind: "list",
  of: { kind: "struct", fields: { values: text, categories: text, name: text, color: text } },
};

const rw = (path: string, type: ValueType, extra: Partial<PropertySchema> = {}): PropertySchema => ({
  path,
  type,
  ...extra,
});
const ro = (path: string, type: ValueType, extra: Partial<PropertySchema> = {}): PropertySchema => ({
  path,
  type,
  access: "readOnly",
  ...extra,
});
const derived = (path: string, type: ValueType, extra: Partial<PropertySchema> = {}): PropertySchema => ({
  path,
  type,
  access: "derived",
  ...extra,
});

/** The format rows a cell and a range share (the engine's style patch). */
const STYLE_ROWS: PropertySchema[] = [
  rw("numberFormat", text, { title: "Number format", summary: "An Excel number-format code (`0.00`, `#,##0`, `yyyy-mm-dd`)." }),
  rw("fontName", text, { title: "Font" }),
  rw("fontSize", { kind: "number", unit: "pt" }, { title: "Font size", range: { min: 1, max: 409 } }),
  rw("bold", bool, { title: "Bold" }),
  rw("italic", bool, { title: "Italic" }),
  rw("underline", bool, { title: "Underline" }),
  rw("fontColor", text, { title: "Font colour", summary: "`#RRGGBB`; empty = automatic." }),
  rw("fill", text, { title: "Fill", summary: "`#RRGGBB`; empty = none." }),
  rw("hAlign", { kind: "enum", members: H_ALIGNS }, { title: "Horizontal alignment", default: "general" }),
  rw("vAlign", { kind: "enum", members: V_ALIGNS }, { title: "Vertical alignment", default: "bottom" }),
  rw("wrap", bool, { title: "Wrap text" }),
];
const STYLE_PATHS = new Set(STYLE_ROWS.map((r) => r.path));

/** The schema rows per kind (also the manifest's inline schema — the
 *  manifest spec pins that they agree). */
export const SHEET_SCHEMAS: Record<SheetKind, PropertySchema[]> = {
  workbook: [
    rw("name", text, { title: "Name" }),
    derived("sheets", { kind: "list", of: text }, { title: "Sheets" }),
    rw("iterative", bool, { title: "Iterative calculation" }),
    derived("version", text, { title: "Version", nullable: true, summary: "The content hash the document's label names." }),
    derived("dataSource", text, { title: "Dataset", nullable: true }),
  ],
  sheet: [
    rw("name", text, { title: "Name" }),
    derived("index", int, { title: "Index" }),
    derived("usedRange", text, { title: "Used range", nullable: true }),
    rw("freezeRows", int, { title: "Frozen rows", range: { min: 0 } }),
    rw("freezeCols", int, { title: "Frozen columns", range: { min: 0 } }),
  ],
  cell: [
    rw("value", text, {
      title: "Value",
      summary: "The computed value, unformatted (numbers as `5`, booleans `TRUE`). A write enters a constant; a text starting with `=` is refused (write `formula`).",
    }),
    rw("formula", text, { title: "Formula", nullable: true, summary: "`=…`, or null when the cell holds a constant." }),
    rw("input", text, { title: "Input", summary: "What the formula bar shows: a constant or `=…`." }),
    derived("display", text, { title: "Display", summary: "The formatted text." }),
    ...STYLE_ROWS,
  ],
  range: [
    rw("values", grid, {
      title: "Values",
      summary: "Rows of computed values; a write enters constants (its shape must match the range).",
      heavy: true,
    }),
    rw("inputs", grid, { title: "Inputs", heavy: true }),
    rw("formula", text, { title: "Formula fill", summary: "Write: the formula entered in the top-left cell and filled across (relative references follow). Read: the top-left cell's formula.", nullable: true }),
    derived("display", grid, { title: "Display", heavy: true }),
    derived("rows", int, { title: "Rows" }),
    derived("cols", int, { title: "Columns" }),
    ...STYLE_ROWS,
  ],
  namedRange: [
    rw("name", text, { title: "Name" }),
    rw("refersTo", text, { title: "Refers to" }),
    rw("scope", text, { title: "Scope", summary: "Empty = the workbook; else the sheet it is visible on." }),
  ],
  table: [
    ro("name", text, { title: "Name" }),
    ro("sheet", text, { title: "Sheet" }),
    ro("range", text, { title: "Range" }),
    ro("columns", { kind: "list", of: text }, { title: "Columns" }),
    ro("headerRow", bool, { title: "Header row" }),
    ro("totalsRow", bool, { title: "Totals row" }),
    ro("style", text, { title: "Table style", nullable: true }),
    derived("totals", { kind: "list", of: text }, { title: "Totals", summary: "The totals row's displayed values; empty without one." }),
    derived("rowCount", int, { title: "Body rows" }),
  ],
  chart: [
    rw("kind", { kind: "enum", members: CHART_KINDS }, { title: "Chart type" }),
    rw("title", text, { title: "Title", nullable: true }),
    rw("legend", bool, { title: "Legend" }),
    rw("series", series, { title: "Series", summary: "Ranges as `Sheet!A1:B2`; empty strings mean none." }),
    rw("categoryAxisTitle", text, { title: "Category axis title", nullable: true }),
    rw("valueAxisTitle", text, { title: "Value axis title", nullable: true }),
    rw("valueAxisMin", { kind: "number" }, { title: "Value axis minimum", nullable: true }),
    rw("valueAxisMax", { kind: "number" }, { title: "Value axis maximum", nullable: true }),
    derived("sheet", text, { title: "Sheet" }),
  ],
};

const KIND_TITLES: Record<SheetKind, string> = {
  workbook: "Workbook",
  sheet: "Sheet",
  cell: "Cell",
  range: "Range",
  namedRange: "Named range",
  table: "Table",
  chart: "Chart",
};

// ----------------------------------------------------------- addresses

export const addressOf = (kind: SheetKind, id: string): Address => `plugin:${PLUGIN_ID}/${kind}/${id}`;

const escapeId = (t: string): string => t.replace(/%/g, "%25").replace(/ /g, "%20");
const unescapeId = (t: string): string => t.replace(/%20/g, " ").replace(/%25/g, "%");

function idOf(address: Address, kind: SheetKind): string | null {
  const p = `plugin:${PLUGIN_ID}/${kind}/`;
  return address.startsWith(p) ? unescapeId(address.slice(p.length)) : null;
}

/** `<sheet>!<rest>` (the sheet optionally quoted) → its parts. */
function splitSheet(id: string): { sheet: string; rest: string } | null {
  const bang = id.lastIndexOf("!");
  if (bang <= 0) return null;
  let sheet = id.slice(0, bang);
  if (sheet.startsWith("'") && sheet.endsWith("'")) sheet = sheet.slice(1, -1).replace(/''/g, "'");
  return { sheet, rest: id.slice(bang + 1) };
}

function columnLabel(col: number): string {
  let n = col;
  let label = "";
  do {
    label = String.fromCharCode(65 + (n % 26)) + label;
    n = Math.floor(n / 26) - 1;
  } while (n >= 0);
  return label;
}
const a1 = (row: number, col: number): string => `${columnLabel(col)}${row + 1}`;

class Refuse extends Error {}

// --------------------------------------------------------------- values

function rawText(v: RawValue): string {
  if (v === null) return "";
  if (typeof v === "boolean") return v ? "TRUE" : "FALSE";
  if (typeof v === "number") return String(v);
  if (typeof v === "string") return v;
  return v.error;
}

const value = (v: unknown): ObjectValue => ({ kind: "value", value: v });
const refusedValue = (code: "unknownAddress" | "unknownPath" | "failed", reason: string): ObjectValue => ({
  kind: "refused",
  code,
  reason,
});

// --------------------------------------------------------- the version

interface ChartOp {
  op: "add" | "update";
  index?: number;
  sheet?: number;
  values?: string;
  categories?: string;
  kind?: string;
  title?: string;
  patch?: ChartPatch;
}

interface VersionMeta {
  v: 1;
  name: string;
  chartOps: ChartOp[];
}

async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const d = await globalThis.crypto.subtle.digest("SHA-256", bytes as Uint8Array<ArrayBuffer>);
  return Array.from(new Uint8Array(d), (b) => b.toString(16).padStart(2, "0")).join("");
}

// ------------------------------------------------------------ the model

export interface SheetObjectModel {
  /** Re-read the version label and load the version it names (document
   *  undo / redo / open). Serialised; never rejects. */
  reconcile(): Promise<void>;
  dispose(): void;
}

export function contributeObjectModel(host: BundleHost, session: WorkbookSession): SheetObjectModel {
  const bridge: ObjectBridge = session.objectBridge();
  const enc = new TextEncoder();
  const dec = new TextDecoder();

  // The version bookkeeping of THE workbook (reset when it is replaced).
  let epoch = bridge.epoch();
  /** The version the engine holds now (null = not yet tracked). */
  let loadedHash: string | null = null;
  /** The version "no label" stands for (the state before the first write). */
  let baselineHash: string | null = null;
  let chartOps: ChartOp[] = [];

  function syncEpoch(): void {
    const now = bridge.epoch();
    if (now === epoch) return;
    epoch = now;
    loadedHash = null;
    baselineHash = null;
    chartOps = [];
  }

  const engineOrThrow = (): SheetEngine => {
    const e = bridge.engine();
    if (!e) throw new Refuse("no workbook is open");
    return e;
  };

  // ── resolution ───────────────────────────────────────────────────────

  function sheetIdByName(engine: SheetEngine, name: string): number {
    const lower = name.toLowerCase();
    const s = engine.listSheets().find((x) => x.name.toLowerCase() === lower);
    if (!s) throw new Refuse(`no sheet named "${name}"`);
    return s.id;
  }

  function sheetName(engine: SheetEngine, id: number): string {
    const s = engine.listSheets().find((x) => x.id === id);
    if (!s) throw new Refuse(`no sheet ${id}`);
    return s.name;
  }

  /** `Sheet!A1[:B2]` resolved by the ENGINE. */
  function resolveRef(engine: SheetEngine, id: string, single: boolean): ResolvedRange {
    const parts = splitSheet(id);
    if (!parts) throw new Refuse(`"${id}" must name its sheet (Sheet1!B4)`);
    const sheet = sheetIdByName(engine, parts.sheet);
    if (!engine.resolveRange) throw new Refuse("engine wasm predates resolve_range");
    let r: ResolvedRange;
    try {
      r = engine.resolveRange(sheet, parts.rest);
    } catch (err) {
      throw new Refuse(`"${id}": ${err instanceof Error ? err.message : String(err)}`);
    }
    if (single && (r.top !== r.bottom || r.left !== r.right)) throw new Refuse(`"${id}" is not one cell`);
    return r;
  }

  function cellAddress(engine: SheetEngine, sheet: number, row: number, col: number): Address {
    return addressOf("cell", escapeId(`${sheetName(engine, sheet)}!${a1(row, col)}`));
  }

  // ── reads ────────────────────────────────────────────────────────────

  function styleOf(engine: SheetEngine, r: ResolvedRange, path: string): ObjectValue {
    if (!engine.getStyle) return refusedValue("failed", "engine wasm predates get_style");
    const key: keyof CellStylePatch = path === "numberFormat" ? "numFmt" : (path as keyof CellStylePatch);
    const area = (r.bottom - r.top + 1) * (r.right - r.left + 1);
    const first = engine.getStyle(r.sheet, r.top, r.left)[key] ?? null;
    if (area <= 256) {
      for (let row = r.top; row <= r.bottom; row++) {
        for (let col = r.left; col <= r.right; col++) {
          const v = engine.getStyle(r.sheet, row, col)[key] ?? null;
          if (JSON.stringify(v) !== JSON.stringify(first)) return { kind: "mixed" };
        }
      }
    }
    return value(first);
  }

  function readCellOrRange(engine: SheetEngine, kind: "cell" | "range", id: string, path: string): ObjectValue {
    const r = resolveRef(engine, id, kind === "cell");
    if (STYLE_PATHS.has(path)) return styleOf(engine, r, path);
    if (!engine.getRangeRaw || !engine.getRangeInputs) return refusedValue("failed", "engine wasm predates the value doors");
    if (kind === "cell") {
      switch (path) {
        case "value":
          return value(rawText(engine.getRangeRaw(r.sheet, r.range)[0]?.[0] ?? null));
        case "input":
          return value(engine.getRangeInputs(r.sheet, r.range)[0]?.[0] ?? "");
        case "formula": {
          const input = engine.getRangeInputs(r.sheet, r.range)[0]?.[0] ?? "";
          return value(input.startsWith("=") ? input : null);
        }
        case "display":
          return value(engine.getRangeValues(r.sheet, r.range)[0]?.[0] ?? "");
      }
      return refusedValue("unknownPath", `cell has no path "${path}"`);
    }
    switch (path) {
      case "values":
        return value(engine.getRangeRaw(r.sheet, r.range).map((row) => row.map(rawText)));
      case "inputs":
        return value(engine.getRangeInputs(r.sheet, r.range));
      case "display":
        return value(engine.getRangeValues(r.sheet, r.range));
      case "formula": {
        const input = engine.getRangeInputs(r.sheet, a1(r.top, r.left))[0]?.[0] ?? "";
        return value(input.startsWith("=") ? input : null);
      }
      case "rows":
        return value(r.bottom - r.top + 1);
      case "cols":
        return value(r.right - r.left + 1);
    }
    return refusedValue("unknownPath", `range has no path "${path}"`);
  }

  function namedRangeOf(engine: SheetEngine, id: string) {
    const parts = splitSheet(id);
    const scope = parts ? sheetIdByName(engine, parts.sheet) : null;
    const name = parts ? parts.rest : id;
    const lower = name.toLowerCase();
    const n = (engine.listNames?.() ?? []).find(
      (x) => x.name.toLowerCase() === lower && (x.scope ?? null) === scope,
    );
    if (!n) throw new Refuse(`no defined name "${id}"`);
    return n;
  }

  function chartOf(engine: SheetEngine, id: string): ChartSpec {
    const index = Number(id);
    const c = Number.isInteger(index) ? engine.chartSpecs?.().find((x) => x.index === index) : undefined;
    if (!c) throw new Refuse(`no chart ${id}`);
    return c;
  }

  function seriesOut(c: ChartSpec) {
    return c.series.map((x) => ({
      values: x.values,
      categories: x.categories ?? "",
      name: x.name ?? "",
      color: x.color ?? "",
    }));
  }

  function read(kind: SheetKind, address: Address, path: string): ObjectValue {
    const id = idOf(address, kind);
    if (id === null) return refusedValue("unknownAddress", `${address} is not a ${kind}`);
    const engine = bridge.engine();
    if (!engine) return refusedValue("unknownAddress", "no workbook is open");
    try {
      switch (kind) {
        case "workbook": {
          if (id !== WORKBOOK_ID) throw new Refuse(`no workbook "${id}"`);
          const st = session.state();
          switch (path) {
            case "name":
              return value(st.fileName ?? "");
            case "sheets":
              return value(engine.listSheets().map((x) => x.name));
            case "iterative":
              return value(engine.calcSettings?.().iterative ?? false);
            case "version":
              return value(loadedHash);
            case "dataSource":
              return value(st.dataSource?.providerId ?? null);
          }
          break;
        }
        case "sheet": {
          const sid = sheetIdByName(engine, id);
          const info = engine.listSheets().find((x) => x.id === sid)!;
          switch (path) {
            case "name":
              return value(info.name);
            case "index":
              return value(engine.listSheets().findIndex((x) => x.id === sid));
            case "usedRange":
              return value(info.rows > 0 && info.cols > 0 ? `A1:${a1(info.rows - 1, info.cols - 1)}` : null);
            case "freezeRows":
              return value(engine.getLayout?.(sid).freezeRows ?? 0);
            case "freezeCols":
              return value(engine.getLayout?.(sid).freezeCols ?? 0);
          }
          break;
        }
        case "cell":
        case "range":
          return readCellOrRange(engine, kind, id, path);
        case "namedRange": {
          const n = namedRangeOf(engine, id);
          switch (path) {
            case "name":
              return value(n.name);
            case "refersTo":
              return value(n.refersTo);
            case "scope":
              return value(n.scope === null || n.scope === undefined ? "" : sheetName(engine, n.scope));
          }
          break;
        }
        case "table": {
          const lower = id.toLowerCase();
          const t = (engine.listTables?.() ?? []).find((x) => x.name.toLowerCase() === lower);
          if (!t) throw new Refuse(`no table "${id}"`);
          switch (path) {
            case "name":
              return value(t.name);
            case "sheet":
              return value(sheetName(engine, t.sheet));
            case "range":
              return value(t.range);
            case "columns":
              return value(t.columns);
            case "headerRow":
              return value(t.headerRow);
            case "totalsRow":
              return value(t.totalsRow);
            case "style":
              return value(t.styleName ?? null);
            case "totals":
            case "rowCount": {
              const r = engine.resolveRange!(t.sheet, t.range);
              const rows = r.bottom - r.top + 1 - (t.headerRow ? 1 : 0) - (t.totalsRow ? 1 : 0);
              if (path === "rowCount") return value(Math.max(rows, 0));
              if (!t.totalsRow) return value([]);
              const last = `${a1(r.bottom, r.left)}:${a1(r.bottom, r.right)}`;
              return value(engine.getRangeValues(t.sheet, last)[0] ?? []);
            }
          }
          break;
        }
        case "chart": {
          const c = chartOf(engine, id);
          switch (path) {
            case "kind":
              return value(c.kind);
            case "title":
              return value(c.title ?? null);
            case "legend":
              return value(c.legend);
            case "series":
              return value(seriesOut(c));
            case "categoryAxisTitle":
              return value(c.categoryAxisTitle ?? null);
            case "valueAxisTitle":
              return value(c.valueAxisTitle ?? null);
            case "valueAxisMin":
              return value(c.valueAxisMin ?? null);
            case "valueAxisMax":
              return value(c.valueAxisMax ?? null);
            case "sheet":
              return value(sheetName(engine, c.hostSheet));
          }
          break;
        }
      }
      return refusedValue("unknownPath", `${kind} has no path "${path}"`);
    } catch (err) {
      if (err instanceof Refuse) return refusedValue("unknownAddress", err.message);
      return refusedValue("failed", err instanceof Error ? err.message : String(err));
    }
  }

  function list(kind: SheetKind, within?: Address, limit?: number): Address[] {
    const engine = bridge.engine();
    if (!engine) return [];
    const out: Address[] = [];
    const scopeSheet = within ? idOf(within, "sheet") : null;
    const sheets = engine.listSheets().filter((x) => scopeSheet === null || x.name === scopeSheet);
    const cap = limit ?? Number.POSITIVE_INFINITY;
    switch (kind) {
      case "workbook":
        return [addressOf("workbook", WORKBOOK_ID)];
      case "sheet":
        return sheets.map((x) => addressOf("sheet", escapeId(x.name)));
      case "range":
        return sheets
          .filter((x) => x.rows > 0 && x.cols > 0)
          .map((x) => addressOf("range", escapeId(`${x.name}!A1:${a1(x.rows - 1, x.cols - 1)}`)));
      case "cell": {
        // The cells that hold something, sheet by sheet.
        for (const x of sheets) {
          if (x.rows === 0 || x.cols === 0 || !engine.getRangeInputs) continue;
          const inputs = engine.getRangeInputs(x.id, `A1:${a1(x.rows - 1, x.cols - 1)}`);
          for (let r = 0; r < inputs.length; r++) {
            for (let c = 0; c < inputs[r]!.length; c++) {
              if (inputs[r]![c] === "") continue;
              out.push(addressOf("cell", escapeId(`${x.name}!${a1(r, c)}`)));
              if (out.length >= cap) return out;
            }
          }
        }
        return out;
      }
      case "namedRange":
        return (engine.listNames?.() ?? []).map((n) =>
          addressOf(
            "namedRange",
            escapeId(n.scope === null || n.scope === undefined ? n.name : `${sheetName(engine, n.scope)}!${n.name}`),
          ),
        );
      case "table":
        return (engine.listTables?.() ?? [])
          .filter((t) => scopeSheet === null || sheetName(engine, t.sheet) === scopeSheet)
          .map((t) => addressOf("table", escapeId(t.name)));
      case "chart":
        return (engine.chartSpecs?.() ?? [])
          .filter((c) => scopeSheet === null || sheetName(engine, c.hostSheet) === scopeSheet)
          .map((c) => addressOf("chart", String(c.index)));
    }
  }

  // ── writes ───────────────────────────────────────────────────────────

  /** What one batch accumulates before it touches the engine. */
  interface Pending {
    cells: { sheet: number; row: number; col: number; input: string }[];
    regions: CellRegion[];
  }

  const regionOf = (r: ResolvedRange): CellRegion => ({
    sheet: r.sheet,
    firstRow: r.top,
    firstCol: r.left,
    lastRow: r.bottom,
    lastCol: r.right,
  });

  function flushCells(engine: SheetEngine, p: Pending): void {
    if (p.cells.length === 0) return;
    const batch = p.cells;
    p.cells = [];
    if (engine.setCells) engine.setCells(batch);
    else for (const c of batch) engine.setCell(c.sheet, c.row, c.col, c.input);
  }

  const notFormula = (v: unknown, what: string): string => {
    const t = String(v);
    if (t.startsWith("=")) throw new Refuse(`${what}: "${t}" starts with "=" — write the formula path`);
    return t;
  };

  /** A planned op: validated against the workbook as it is, applied later. */
  type Step = (engine: SheetEngine, p: Pending) => void;

  function planStyle(r: ResolvedRange, path: string, v: unknown): Step {
    const patch: CellStylePatch = {};
    if (path === "numberFormat") patch.numFmt = String(v);
    else (patch as Record<string, unknown>)[path] = v;
    return (engine, p) => {
      if (!engine.setStyle) throw new Refuse("engine wasm predates set_style");
      flushCells(engine, p);
      engine.setStyle(r.sheet, r.range, patch);
      p.regions.push(regionOf(r));
    };
  }

  function planSet(engine: SheetEngine, kind: SheetKind, address: Address, path: string, v: unknown): Step {
    const id = idOf(address, kind);
    if (id === null) throw new Refuse(`${address} is not a ${kind}`);
    switch (kind) {
      case "workbook":
        if (path === "name") return () => bridge.setName(String(v));
        if (path === "iterative") {
          return (e) => {
            const cs = e.calcSettings?.();
            if (!e.setIterative) throw new Refuse("engine wasm predates set_iterative");
            e.setIterative(Boolean(v), cs?.maxIter ?? 100, cs?.maxChange ?? 0.001);
          };
        }
        break;
      case "sheet": {
        const sid = sheetIdByName(engine, id);
        if (path === "name") {
          return (e) => {
            if (!e.renameSheet) throw new Refuse("engine wasm predates rename_sheet");
            e.renameSheet(sid, String(v));
          };
        }
        if (path === "freezeRows" || path === "freezeCols") {
          return (e) => {
            if (!e.setFreeze) throw new Refuse("engine wasm predates set_freeze");
            const l = e.getLayout?.(sid);
            const rows = path === "freezeRows" ? Number(v) : (l?.freezeRows ?? 0);
            const cols = path === "freezeCols" ? Number(v) : (l?.freezeCols ?? 0);
            e.setFreeze(sid, rows, cols);
          };
        }
        break;
      }
      case "cell": {
        const r = resolveRef(engine, id, true);
        if (STYLE_PATHS.has(path)) return planStyle(r, path, v);
        let input: string;
        if (path === "value") input = notFormula(v, "value");
        else if (path === "input") input = String(v);
        else if (path === "formula") {
          if (v === null) input = "";
          else if (!String(v).startsWith("=")) throw new Refuse(`formula "${String(v)}" must start with "="`);
          else input = String(v);
        } else break;
        return (_e, p) => {
          p.cells.push({ sheet: r.sheet, row: r.top, col: r.left, input });
          p.regions.push(regionOf(r));
        };
      }
      case "range": {
        const r = resolveRef(engine, id, false);
        if (STYLE_PATHS.has(path)) return planStyle(r, path, v);
        const rows = r.bottom - r.top + 1;
        const cols = r.right - r.left + 1;
        if (path === "values" || path === "inputs") {
          const g = v as unknown[][];
          if (g.length !== rows || g.some((row) => row.length !== cols)) {
            throw new Refuse(`${path} must be ${rows}×${cols} for ${id}`);
          }
          const inputs = g.map((row) => row.map((x) => (path === "values" ? notFormula(x, "values") : String(x))));
          return (_e, p) => {
            for (let i = 0; i < rows; i++) {
              for (let j = 0; j < cols; j++) {
                p.cells.push({ sheet: r.sheet, row: r.top + i, col: r.left + j, input: inputs[i]![j]! });
              }
            }
            p.regions.push(regionOf(r));
          };
        }
        if (path === "formula") {
          const f = v === null ? "" : String(v);
          if (f !== "" && !f.startsWith("=")) throw new Refuse(`formula "${f}" must start with "="`);
          return (e, p) => {
            if (f === "") {
              for (let i = 0; i < rows; i++) {
                for (let j = 0; j < cols; j++) p.cells.push({ sheet: r.sheet, row: r.top + i, col: r.left + j, input: "" });
              }
            } else {
              p.cells.push({ sheet: r.sheet, row: r.top, col: r.left, input: f });
              flushCells(e, p);
              if (!e.fillRange) throw new Refuse("engine wasm predates fill_range");
              const topRow = `${a1(r.top, r.left)}:${a1(r.top, r.right)}`;
              if (cols > 1) e.fillRange(r.sheet, a1(r.top, r.left), topRow, false);
              if (rows > 1) e.fillRange(r.sheet, topRow, r.range, false);
            }
            p.regions.push(regionOf(r));
          };
        }
        break;
      }
      case "namedRange": {
        const n = namedRangeOf(engine, id);
        const scope = n.scope ?? null;
        const ctx = scope ?? 0;
        if (path === "refersTo") {
          return (e) => e.defineName!(ctx, n.name, String(v), scope);
        }
        if (path === "name") {
          return (e) => {
            e.defineName!(ctx, String(v), n.refersTo, scope);
            e.deleteName!(ctx, n.name, scope);
          };
        }
        if (path === "scope") {
          const next = String(v) === "" ? null : sheetIdByName(engine, String(v));
          return (e) => {
            e.defineName!(next ?? 0, n.name, n.refersTo, next);
            e.deleteName!(ctx, n.name, scope);
          };
        }
        break;
      }
      case "table":
        throw new Refuse("tables are read-only: the xlsx table part re-emits verbatim (spec §10.2)");
      case "chart": {
        const c = chartOf(engine, id);
        const patch: ChartPatch = {};
        switch (path) {
          case "kind":
          case "title":
          case "legend":
          case "categoryAxisTitle":
          case "valueAxisTitle":
          case "valueAxisMin":
          case "valueAxisMax":
            (patch as Record<string, unknown>)[path] = v;
            break;
          case "series":
            patch.series = v as ChartPatch["series"];
            break;
          default:
            throw new Refuse(`chart has no writable path "${path}"`);
        }
        return (e) => {
          if (!e.updateChart) throw new Refuse("engine wasm predates update_chart");
          e.updateChart(c.index, patch);
          chartOps.push({ op: "update", index: c.index, patch });
        };
      }
    }
    throw new Refuse(`${kind} has no writable path "${path}"`);
  }

  function planCreate(engine: SheetEngine, kind: SheetKind, props: Record<string, unknown>): Step {
    switch (kind) {
      case "sheet":
        return (e) => {
          if (!e.addSheet) throw new Refuse("engine wasm predates add_sheet");
          e.addSheet(String(props.name ?? ""));
        };
      case "namedRange": {
        const name = String(props.name ?? "");
        const refersTo = String(props.refersTo ?? "");
        if (!name || !refersTo) throw new Refuse("a named range needs name and refersTo");
        const scope = props.scope ? sheetIdByName(engine, String(props.scope)) : null;
        return (e) => e.defineName!(scope ?? 0, name, refersTo, scope);
      }
      case "chart": {
        const list = (props.series ?? []) as { values: string; categories: string }[];
        if (list.length === 0) throw new Refuse("a chart needs at least one series");
        const kindTag = String(props.kind ?? "column");
        const first = list[0]!;
        const parts = splitSheet(first.values);
        const sheet = parts ? sheetIdByName(engine, parts.sheet) : 0;
        const title = props.title === null || props.title === undefined ? "" : String(props.title);
        // `addChart` takes plain A1 on its host sheet; the full series
        // (any sheet) lands through `updateChart` below.
        const plain = (t: string) => (t ? (splitSheet(t)?.rest ?? t) : "");
        return (e) => {
          const values = plain(first.values);
          const categories = plain(first.categories ?? "");
          const index = e.addChart(sheet, values, categories, kindTag, title);
          chartOps.push({ op: "add", sheet, values, categories, kind: kindTag, title });
          const patch: ChartPatch = {};
          for (const [k, x] of Object.entries(props)) {
            if (k !== "kind" && k !== "title") (patch as Record<string, unknown>)[k] = x;
          }
          if (Object.keys(patch).length > 0 && e.updateChart) {
            e.updateChart(index, patch);
            chartOps.push({ op: "update", index, patch });
          }
        };
      }
    }
    throw new Refuse(`${kind} objects are not created through the object model`);
  }

  function planDelete(engine: SheetEngine, kind: SheetKind, address: Address): Step {
    const id = idOf(address, kind);
    if (id === null) throw new Refuse(`${address} is not a ${kind}`);
    switch (kind) {
      case "sheet": {
        const sid = sheetIdByName(engine, id);
        return (e) => {
          if (!e.deleteSheet) throw new Refuse("engine wasm predates delete_sheet");
          e.deleteSheet(sid);
        };
      }
      case "namedRange": {
        const n = namedRangeOf(engine, id);
        return (e) => e.deleteName!(n.scope ?? 0, n.name, n.scope ?? null);
      }
      case "cell":
      case "range": {
        const r = resolveRef(engine, id, kind === "cell");
        return (_e, p) => {
          for (let i = r.top; i <= r.bottom; i++) {
            for (let j = r.left; j <= r.right; j++) p.cells.push({ sheet: r.sheet, row: i, col: j, input: "" });
          }
          p.regions.push(regionOf(r));
        };
      }
    }
    throw new Refuse(`${kind} objects are not deleted through the object model`);
  }

  /** Save the workbook as a content-addressed version. */
  async function snapshot(engine: SheetEngine): Promise<{ hash: string; parts: PartChange[] }> {
    const bytes = engine.saveXlsx();
    const meta: VersionMeta = { v: 1, name: session.state().fileName ?? "workbook.xlsx", chartOps };
    const metaBytes = enc.encode(JSON.stringify(meta));
    const all = new Uint8Array(bytes.length + 1 + metaBytes.length);
    all.set(bytes, 0);
    all.set(metaBytes, bytes.length + 1);
    const hash = (await sha256Hex(all)).slice(0, 32);
    return {
      hash,
      parts: [
        { path: `${VERSIONS}${hash}.xlsx`, bytes },
        { path: `${VERSIONS}${hash}.json`, bytes: metaBytes },
      ],
    };
  }

  /**
   * The one write path: plan every step (validation against the workbook
   * as it is), apply them to the engine, save a version. Any failure
   * restores the version the engine held and refuses the whole write.
   */
  async function write(
    plan: (engine: SheetEngine) => Step[],
  ): Promise<{ ok: true; hash: string; parts: PartChange[] } | { ok: false; reason: string }> {
    await session.ensureRestored();
    syncEpoch();
    const engine = bridge.engine();
    if (!engine) return { ok: false, reason: "no workbook is open" };
    let steps: Step[];
    try {
      steps = plan(engine);
    } catch (err) {
      return { ok: false, reason: err instanceof Error ? err.message : String(err) };
    }
    const parts: PartChange[] = [];
    if (loadedHash === null) {
      // The first write of this workbook: store where it started, so an undo
      // past every write (the label gone) has a version to go back to.
      const base = await snapshot(engine);
      baselineHash = loadedHash = base.hash;
      parts.push(...base.parts);
    }
    const before = { hash: loadedHash, ops: chartOps.length };
    const pending: Pending = { cells: [], regions: [] };
    try {
      for (const step of steps) step(engine, pending);
      flushCells(engine, pending);
    } catch (err) {
      chartOps = chartOps.slice(0, before.ops);
      await loadVersion(before.hash);
      return { ok: false, reason: err instanceof Error ? err.message : String(err) };
    }
    const next = await snapshot(engine);
    parts.push(...next.parts);
    loadedHash = next.hash;
    bridge.afterWrite(pending.regions);
    return { ok: true, hash: next.hash, parts };
  }

  /** The document envelope with `data.wb = hash`, other sub-keys kept
   *  (a typed command commits through its own door, so it merges here the
   *  way the registry merges a sub-key `state` write). */
  async function labelMutation(hash: string) {
    let data: Record<string, unknown> = {};
    try {
      const env = await host.document?.getDocumentMetadata?.();
      if (env && typeof env.data === "object" && env.data !== null) data = { ...(env.data as Record<string, unknown>) };
    } catch {
      /* no readable label: start a fresh envelope */
    }
    data.wb = hash;
    return {
      op: "setDocumentMetadata",
      args: { key: VERSION_LABEL_KEY, value: JSON.stringify({ v: 1, data }), caller: PLUGIN_ID },
    } as const;
  }

  async function writeParts(parts: readonly PartChange[]): Promise<void> {
    for (const part of parts) {
      if (part.bytes) await host.parts.write(part.path, part.bytes);
    }
  }

  /** The kind an op targets: a `create` names it (qualified or bare),
   *  every other op by its address. */
  function kindOf(op: ObjectOp): SheetKind {
    const prefix = `plugin:${PLUGIN_ID}/`;
    const raw =
      op.op === "create"
        ? op.kind.startsWith(prefix)
          ? op.kind.slice(prefix.length)
          : op.kind
        : op.op === "invoke"
          ? ""
          : op.address.startsWith(prefix)
            ? op.address.slice(prefix.length).split("/")[0]!
            : "";
    if (!(SHEET_KINDS as readonly string[]).includes(raw)) throw new Refuse(`${JSON.stringify(op)} is not a paged.sheet op`);
    return raw as SheetKind;
  }

  /** The PLUGIN-level planner (SDK 0.2.43): every op of a batch, across all
   *  seven kinds, in batch order — one engine pass, one saved version, one
   *  doc-hosted `state` write. */
  async function batchWrite(ops: readonly ObjectOp[]): Promise<ObjectWrite> {
    const r = await write((engine) =>
      ops.map((op) => {
        const kind = kindOf(op);
        if (op.op === "set") return planSet(engine, kind, op.address, op.path, op.value);
        if (op.op === "create") return planCreate(engine, kind, op.props ?? {});
        if (op.op === "delete") return planDelete(engine, kind, op.address);
        throw new Refuse(`"${op.op}" is not a write`);
      }),
    );
    if (!r.ok) return { kind: "rejected", reason: r.reason };
    return {
      kind: "state",
      parts: r.parts,
      host: "doc",
      labelKey: VERSION_SUB_KEY,
      // JSON text, so the registry stores data.wb as a string even for an
      // all-digit hash.
      labelValue: JSON.stringify(r.hash),
    };
  }

  /** A typed command's write: the same versioning, committed through the
   *  bundle's own doors (the parts, then the label as ONE mutation). */
  async function commandWrite(apply: (engine: SheetEngine, p: Pending) => void): Promise<void> {
    const r = await write(() => [apply]);
    if (!r.ok) throw new Error(r.reason);
    await writeParts(r.parts);
    const outcome = await host.document.mutate((await labelMutation(r.hash)) as never);
    if (!(outcome as { applied?: boolean }).applied) {
      host.log.warn("object model: the version label was refused — the change is not an undo step", outcome);
    }
  }

  // ── versions: undo / redo / open ─────────────────────────────────────

  async function readVersion(hash: string): Promise<{ bytes: Uint8Array; meta: VersionMeta } | null> {
    const bytes = await host.parts.read(`${VERSIONS}${hash}.xlsx`);
    const metaBytes = await host.parts.read(`${VERSIONS}${hash}.json`);
    if (!bytes || !metaBytes) return null;
    return { bytes, meta: JSON.parse(dec.decode(metaBytes)) as VersionMeta };
  }

  async function loadVersion(hash: string | null): Promise<boolean> {
    if (!hash) return false;
    const v = await readVersion(hash).catch(() => null);
    if (!v) {
      host.log.warn(`object model: workbook version ${hash} is not stored`);
      return false;
    }
    if (!bridge.loadVersion(v.bytes, v.meta.name)) return false;
    const engine = bridge.engine()!;
    for (const op of v.meta.chartOps) {
      try {
        if (op.op === "add") engine.addChart(op.sheet!, op.values!, op.categories ?? "", op.kind!, op.title ?? "");
        else if (op.op === "update") engine.updateChart?.(op.index!, op.patch!);
      } catch (err) {
        host.log.warn("object model: a chart op did not replay", err);
      }
    }
    chartOps = [...v.meta.chartOps];
    loadedHash = hash;
    return true;
  }

  /** The hash the document's version label names (null = no label;
   *  undefined = the host cannot say). */
  async function labelHash(): Promise<string | null | undefined> {
    if (typeof host.document?.getDocumentMetadata !== "function") return undefined;
    try {
      const env = await host.document.getDocumentMetadata();
      const wb = (env?.data as { wb?: unknown } | undefined)?.wb;
      return typeof wb === "string" ? wb : null;
    } catch {
      return undefined;
    }
  }

  let reconciling: Promise<void> = Promise.resolve();
  function reconcile(): Promise<void> {
    reconciling = reconciling
      .then(async () => {
        syncEpoch();
        if (!bridge.engine()) return;
        const h = await labelHash();
        if (h === undefined) return;
        const target = h ?? baselineHash;
        if (!target || target === loadedHash) return;
        if (await loadVersion(target)) {
          if (h === null) loadedHash = baselineHash;
        }
      })
      .catch((err) => host.log.warn("object model: version follow failed", err));
    return reconciling;
  }

  const historySub =
    typeof host.document?.onDidChange === "function"
      ? host.document.onDidChange((e) => {
          if (e.kind === "undoApplied" || e.kind === "redoApplied") void reconcile();
        })
      : null;

  // ── datasets: publish a range ────────────────────────────────────────

  const published = new Map<string, { range: string; header: boolean; handle: DataProviderHandle; revision: string }>();
  const revisionNow = (): string => `${bridge.epoch()}.${session.contentVersion()}`;

  function snapshotRange(range: string, header: boolean): ProviderRecordSet {
    const engine = engineOrThrow();
    const r = resolveRef(engine, range, false);
    if (!engine.getRangeRaw) throw new Error("engine wasm predates get_range_raw");
    const raw = engine.getRangeRaw(r.sheet, r.range);
    const cols = r.right - r.left + 1;
    const names = header
      ? (raw[0] ?? []).map((v, i) => rawText(v) || `column${i + 1}`)
      : Array.from({ length: cols }, (_, i) => columnLabel(r.left + i));
    const body = header ? raw.slice(1) : raw;
    const fields: ProviderField[] = [];
    const columns: unknown[][] = [];
    for (let c = 0; c < cols; c++) {
      const col = body.map((row) => row[c] ?? null);
      const present = col.filter((v) => v !== null);
      const ty =
        present.length > 0 && present.every((v) => typeof v === "number")
          ? "float"
          : present.length > 0 && present.every((v) => typeof v === "boolean")
            ? "bool"
            : "text";
      fields.push({ name: names[c]!, ty, nullable: true });
      columns.push(ty === "text" ? col.map((v) => (v === null ? null : rawText(v))) : col);
    }
    return { schema: { fields }, columns, rowCount: body.length };
  }

  function publish(name: string, range: string, header: boolean): string {
    if (!host.dataProviders) throw new Error("no data-provider registry is wired");
    const id = `${PLUGIN_ID}.dataset.${name}`;
    published.get(id)?.handle.dispose();
    const first = snapshotRange(range, header);
    const revision = revisionNow();
    const handle = host.dataProviders.register({
      id,
      category: "dataset",
      schema: first.schema,
      revision,
      getSnapshot: () => snapshotRange(range, header),
    });
    published.set(id, { range, header, handle, revision });
    return id;
  }

  const changeSub = session.onDidChange((c) => {
    if (c.kind === "selection" || published.size === 0) return;
    const revision = revisionNow();
    for (const p of published.values()) {
      if (p.revision === revision) continue;
      p.revision = revision;
      p.handle.update(revision);
    }
  });

  // ── kinds ────────────────────────────────────────────────────────────

  const hostOf = (): Address | null => {
    const f = bridge.hostFrames()[0];
    return f ? `textFrame:${f}` : null;
  };

  const kinds: ObjectKindContribution[] = SHEET_KINDS.map((kind) => ({
    kind,
    title: KIND_TITLES[kind],
    schema: SHEET_SCHEMAS[kind],
    ...(kind === "range" ? { content: { kind: "cells" as const } } : {}),
    hostOf,
    async list(query) {
      await session.ensureRestored();
      syncEpoch();
      return list(kind, query.within, query.limit);
    },
    async get(address, path) {
      await session.ensureRestored();
      return read(kind, address, path);
    },
  }));

  // ── typed commands ───────────────────────────────────────────────────

  const C = (s: string) => `${PLUGIN_ID}.command.${s}`;
  const struct = (fields: Record<string, StructField>): ValueType => ({ kind: "struct", fields });
  /** A field the caller may omit; `invoke` fills the default in. */
  const dflt = (type: ValueType, d: unknown): StructField => ({ ...type, default: d });
  const findOpts = { matchCase: dflt(bool, false), entireCell: dflt(bool, false), inFormulas: dflt(bool, false) };

  const cellRange = (range: string) => resolveRef(engineOrThrow(), range, false);

  const structural = (id: string, title: string, kind: StructuralEditKind): TypedCommandContribution => ({
    id: C(id),
    title,
    args: struct({ sheet: text, at: int, count: dflt(int, 1) }),
    async handler(_ctx, a) {
      const { sheet, at, count } = a as { sheet: string; at: number; count: number };
      await commandWrite((engine) => {
        if (!engine.structuralEdit) throw new Error("engine wasm predates structural_edit");
        engine.structuralEdit(sheetIdByName(engine, sheet), kind, at, count);
      });
    },
  });

  const fill = (id: string, title: string, down: boolean): TypedCommandContribution => ({
    id: C(id),
    title,
    args: struct({ range: text }),
    async handler(_ctx, a) {
      const r = cellRange((a as { range: string }).range);
      await commandWrite((engine, p) => {
        if (!engine.fillRange) throw new Error("engine wasm predates fill_range");
        const src = down ? `${a1(r.top, r.left)}:${a1(r.top, r.right)}` : `${a1(r.top, r.left)}:${a1(r.bottom, r.left)}`;
        engine.fillRange(r.sheet, src, r.range, false);
        p.regions.push(regionOf(r));
      });
    },
  });

  const commands: TypedCommandContribution[] = [
    {
      id: C("newWorkbook"),
      title: "New blank workbook",
      args: struct({}),
      result: text,
      async handler() {
        await session.newWorkbook();
        return addressOf("workbook", WORKBOOK_ID);
      },
    },
    {
      id: C("importXlsx"),
      title: "Import workbook (.xlsx)",
      args: struct({ bytes: { kind: "bytes", mime: ["application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"] }, name: dflt(text, "workbook.xlsx") }),
      result: text,
      async handler(_ctx, a) {
        const { bytes, name } = a as { bytes: Uint8Array; name: string };
        await session.import(bytes, name);
        if (!bridge.engine()) throw new Error("the workbook did not load");
        return addressOf("workbook", WORKBOOK_ID);
      },
    },
    {
      id: C("lowerToFrame"),
      title: "Place a range on the page",
      args: struct({ range: text }),
      result: struct({ frame: text }),
      async handler(_ctx, a) {
        await session.ensureRestored();
        const r = cellRange((a as { range: string }).range);
        session.setActiveSheet(r.sheet);
        session.setRange(r.range);
        const frame = await session.lowerSelection();
        if (!frame) throw new Error(`could not place ${(a as { range: string }).range}`);
        return { frame: `textFrame:${frame}` };
      },
    },
    {
      id: C("lowerChartToFrame"),
      title: "Place a chart on the page",
      args: struct({ chart: int }),
      result: bool,
      async handler(_ctx, a) {
        return session.lowerChart((a as { chart: number }).chart);
      },
    },
    {
      id: C("sheetFromDataset"),
      title: "Sheet from dataset",
      args: struct({ providerId: text, live: dflt(bool, true) }),
      result: text,
      async handler(_ctx, a) {
        const { providerId, live } = a as { providerId: string; live: boolean };
        await session.sourceFromDataset(providerId, { live });
        if (session.state().dataSource?.providerId !== providerId) throw new Error(`could not source ${providerId}`);
        return addressOf("workbook", WORKBOOK_ID);
      },
    },
    {
      id: C("publishDataset"),
      title: "Publish a range as a dataset",
      args: struct({ name: text, range: text, header: dflt(bool, true) }),
      result: text,
      async handler(_ctx, a) {
        await session.ensureRestored();
        const { name, range, header } = a as { name: string; range: string; header: boolean };
        if (!/^[A-Za-z0-9_-]+$/.test(name)) throw new Error(`invalidValue: dataset name "${name}" (letters, digits, _ and - only)`);
        return publish(name, range, header);
      },
    },
    {
      id: C("unpublishDataset"),
      title: "Stop publishing a dataset",
      args: struct({ name: text }),
      result: bool,
      async handler(_ctx, a) {
        const id = `${PLUGIN_ID}.dataset.${(a as { name: string }).name}`;
        const p = published.get(id);
        if (!p) return false;
        p.handle.dispose();
        published.delete(id);
        return true;
      },
    },
    {
      id: C("addSheet"),
      title: "Add sheet",
      args: struct({ name: { ...text, optional: true } }),
      result: text,
      async handler(_ctx, a) {
        let made = "";
        await commandWrite((engine) => {
          if (!engine.addSheet) throw new Error("engine wasm predates add_sheet");
          made = sheetName(engine, engine.addSheet((a as { name?: string }).name ?? ""));
        });
        return addressOf("sheet", escapeId(made));
      },
    },
    structural("insertRows", "Insert rows", "insertRows"),
    structural("deleteRows", "Delete rows", "deleteRows"),
    structural("insertColumns", "Insert columns", "insertCols"),
    structural("deleteColumns", "Delete columns", "deleteCols"),
    fill("fillDown", "Fill down", true),
    fill("fillRight", "Fill right", false),
    {
      id: C("clearCells"),
      title: "Clear cells",
      args: struct({ range: text }),
      async handler(_ctx, a) {
        const r = cellRange((a as { range: string }).range);
        await commandWrite((_e, p) => {
          for (let i = r.top; i <= r.bottom; i++) {
            for (let j = r.left; j <= r.right; j++) p.cells.push({ sheet: r.sheet, row: i, col: j, input: "" });
          }
          p.regions.push(regionOf(r));
        });
      },
    },
    {
      id: C("sortRange"),
      title: "Sort range",
      args: struct({ range: text, keyColumn: dflt(int, 0), ascending: dflt(bool, true), hasHeader: dflt(bool, false) }),
      async handler(_ctx, a) {
        const { range, keyColumn, ascending, hasHeader } = a as {
          range: string;
          keyColumn: number;
          ascending: boolean;
          hasHeader: boolean;
        };
        const r = cellRange(range);
        await commandWrite((engine, p) => {
          engine.sortRange(r.sheet, r.range, keyColumn, ascending, hasHeader);
          p.regions.push(regionOf(r));
        });
      },
    },
    {
      id: C("findInSheet"),
      title: "Find in workbook",
      args: struct({ needle: text, ...findOpts }),
      result: { kind: "list", of: text },
      async handler(_ctx, a) {
        await session.ensureRestored();
        const { needle, ...opts } = a as { needle: string; matchCase: boolean; entireCell: boolean; inFormulas: boolean };
        const engine = engineOrThrow();
        return engine.findAll(undefined, needle, opts).map((m) => cellAddress(engine, m.sheet, m.row, m.col));
      },
    },
    {
      id: C("findReplace"),
      title: "Replace in workbook",
      args: struct({ needle: text, replacement: text, ...findOpts }),
      result: int,
      async handler(_ctx, a) {
        const { needle, replacement, ...opts } = a as {
          needle: string;
          replacement: string;
          matchCase: boolean;
          entireCell: boolean;
          inFormulas: boolean;
        };
        let occurrences = 0;
        await commandWrite((engine) => {
          occurrences = engine.replaceAll(undefined, needle, replacement, opts).occurrences;
        });
        return occurrences;
      },
    },
  ];

  // A host that predates the object model (plugin-api < 0.2.42) has no
  // door: the bundle still works, its objects are just not addressable. A
  // 0.2.42 registry ignores the plugin-level `batch` and finds no per-kind
  // one: writes then refuse (reads still work) — peers require 0.2.43.
  const handle: ObjectModelHandle | null =
    typeof host.contribute?.objectModel === "function"
      ? host.contribute.objectModel({ kinds, commands, batch: batchWrite })
      : null;

  return {
    reconcile,
    dispose() {
      historySub?.dispose();
      changeSub.dispose();
      for (const p of published.values()) p.handle.dispose();
      published.clear();
      handle?.dispose();
    },
  };
}
