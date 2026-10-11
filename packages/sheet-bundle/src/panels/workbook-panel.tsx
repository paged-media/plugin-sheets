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

// The workbook panel — a React expert-leaf factory closing over the
// BundleHost + the in-memory session. It owns the file input (S-11: no
// host file picker), the sheet list, the range input, the "Place on page"
// action, the S-08 storage notice, and the engine-boot failure state
// (S-10) — rendered honestly, never faked.
//
// Layout: the file and sheet controls stay on top; everything else sits in
// collapsible sections (native <details>), with "Place on page" open by
// default. Settings with another home are a one-line pointer here: find
// and replace live in the Grid panel; number format, font, fill, alignment
// and iterative calculation in Sheet properties (when the host renders it).
//
// Built from host surfaces + React ONLY (no @paged-media/shell): all
// spreadsheet work is the engine's; the panel reads the session snapshot
// and triggers session actions. Token-layer styling (--pg-*, --space-*,
// --font-mono, --radius-*, sentence-case labels) mirrors plugin-web's
// source panel so it reads native in both themes.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactElement,
  type ReactNode,
} from "react";

import type { BundleHost } from "@paged-media/plugin-api";

import type { FunctionEntry } from "../../../sheet-host-model/src";
import { DELIMITED_MIMES, importBytes, XLSX_MIME } from "../import-xlsx";
import { columnLabel, type WorkbookSession } from "../session";
import { useConfirm } from "./confirm";
import { FormatSection, labelOf, NamesSection } from "./format-section";
import { PROPERTIES_PANEL_ID } from "./properties-panel";

const GRID_PANEL_ID = "media.paged.sheet.panel.grid";

// ---------------------------------------------------------------- styles

const kicker: CSSProperties = {
  font: "700 10px var(--font-sans, sans-serif)",
  letterSpacing: "var(--tracking-wide, 0.14em)",
  textTransform: "uppercase",
  color: "var(--pg-muted-fg)",
  margin: "var(--space-3, 12px) 0 var(--space-1, 4px)",
};

const body: CSSProperties = {
  font: "12px var(--font-sans, sans-serif)",
  color: "var(--pg-fg)",
};

const note: CSSProperties = {
  margin: "var(--space-1, 4px) 0 0",
  font: "11px/1.5 var(--font-sans, sans-serif)",
  color: "var(--pg-muted-fg)",
};

const input: CSSProperties = {
  font: "12px var(--font-sans, sans-serif)",
  fontFamily: "var(--font-mono, monospace)",
  color: "var(--pg-fg)",
  background: "var(--pg-bg)",
  border: "1px solid var(--pg-border)",
  borderRadius: "var(--radius-sm, 4px)",
  padding: "4px 6px",
  minWidth: 0,
};

const primaryButton: CSSProperties = {
  font: "500 12px var(--font-sans, sans-serif)",
  color: "var(--primary-fg, #fff)",
  background: "var(--pg-primary)",
  border: "none",
  borderRadius: "var(--radius-md, 6px)",
  padding: "6px 12px",
  cursor: "pointer",
};

/** A quieter button for the structure verbs. */
const secondaryButton: CSSProperties = {
  font: "12px var(--font-sans, system-ui)",
  padding: "3px 8px",
  color: "var(--pg-fg)",
  background: "var(--pg-subtle, var(--pg-bg))",
  border: "1px solid var(--pg-border)",
  borderRadius: "var(--radius-sm, 4px)",
  cursor: "pointer",
};

const row: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  alignItems: "center",
  gap: "var(--space-1, 4px)",
  marginTop: "var(--space-1, 4px)",
};

const linkButton: CSSProperties = {
  font: "inherit",
  color: "var(--pg-primary)",
  background: "none",
  border: "none",
  padding: 0,
  cursor: "pointer",
  textDecoration: "underline",
};

const sectionStyle: CSSProperties = {
  borderTop: "1px solid var(--chrome-divider, var(--pg-border))",
  padding: "var(--space-2, 8px) 0",
};

const summaryStyle: CSSProperties = {
  font: "600 12px var(--font-sans, sans-serif)",
  color: "var(--pg-fg)",
  cursor: "pointer",
};

/** One collapsible group of the panel. */
function Section(props: {
  id: string;
  title: string;
  open?: boolean;
  children: ReactNode;
}): ReactElement {
  return (
    <details data-sheet-section={props.id} open={props.open} style={sectionStyle}>
      <summary style={summaryStyle}>{props.title}</summary>
      <div style={{ display: "flex", flexDirection: "column", marginTop: "var(--space-1, 4px)" }}>
        {props.children}
      </div>
    </details>
  );
}

// ----------------------------------------------------------------- panel

export function makeWorkbookPanel(
  host: BundleHost,
  session: WorkbookSession,
): () => ReactElement {
  return function WorkbookPanel(): ReactElement {
    // Re-render on every session change (import, sheet/range edits).
    const [version, force] = useState(0);
    const fileRef = useRef<HTMLInputElement | null>(null);
    useEffect(() => {
      const sub = session.onDidChange(() => force((n) => n + 1));
      return () => sub.dispose();
    }, []);

    const st = session.state();
    const { armed, confirm } = useConfirm();

    const onFile = useCallback(
      async (e: React.ChangeEvent<HTMLInputElement>) => {
        const file = e.target.files?.[0];
        if (!file) return;
        try {
          const bytes = new Uint8Array(await file.arrayBuffer());
          await importBytes(session, bytes, file.name);
        } catch (err) {
          host.log.error("workbook file read failed", err);
        }
      },
      [],
    );

    // S-11: prefer the HOST file picker (a real modal dialog); the panel's
    // own `<input>` stays as the honest no-picker fallback.
    const pickerSupported = host.supports("shell.pickFile@1");
    const onPick = useCallback(async () => {
      try {
        const files = await host.shell.pickFile({
          accept: [".xlsx", XLSX_MIME, ".csv", ".tsv", ...DELIMITED_MIMES],
          multiple: false,
        });
        const file = files[0];
        if (!file) return; // cancelled
        await importBytes(session, file.bytes, file.name);
      } catch (err) {
        host.log.error("workbook pick failed", err);
      }
    }, []);

    // The Sheet properties panel carries number format, font, fill,
    // alignment and iterative calculation; where the host renders it, the
    // format section points there instead of repeating them.
    const openProperties = host.supports("contribute.schemaPanel@1")
      ? () => host.shell.openPanel(PROPERTIES_PANEL_ID)
      : undefined;

    const [placeMsg, setPlaceMsg] = useState<string | null>(null);
    const onPlace = useCallback(() => {
      setPlaceMsg(null);
      void session.lowerSelection().then((id) => {
        if (id === null) setPlaceMsg("The range could not be placed. Check that it is a valid range like A1:D20.");
      });
    }, []);

    // Wave 4 — workbook structure + pagination. Every verb answers
    // {ok} / {ok:false, message}; the message shows verbatim (the engine's
    // own reason, e.g. what preserved content blocks a row insert).
    const [structMsg, setStructMsg] = useState<string | null>(null);
    const [sheetMsg, setSheetMsg] = useState<string | null>(null);
    const [renameTo, setRenameTo] = useState("");
    const report = (r: { ok: true } | { ok: false; message: string }, done: string) =>
      setStructMsg(r.ok ? done : r.message);
    const reportSheet = (r: { ok: true } | { ok: false; message: string }, done: string) =>
      setSheetMsg(r.ok ? done : r.message);
    // Wave 6 — header rows repeated at the top of every threaded frame.
    const [headerRows, setHeaderRows] = useState(1);
    const onPaginate = useCallback(async () => {
      const r = await session.paginateSelection({
        repeatHeaderRows: Math.max(0, Math.floor(headerRows)),
      });
      setPlaceMsg(r.ok ? "Paginated into the frame chain (live)." : r.message);
    }, [headerRows]);

    // Sort-range controls (thin glue — the engine owns the sort semantics;
    // sheet.plugin.sort.command). Key column is 1-based in the UI.
    const [sortKey, setSortKey] = useState(1);
    const [sortAsc, setSortAsc] = useState(true);
    const [sortHeader, setSortHeader] = useState(false);
    const [sortMsg, setSortMsg] = useState<string | null>(null);
    const onSort = useCallback(() => {
      const res = session.sortRange(
        Math.max(0, Math.floor(sortKey) - 1),
        sortAsc,
        sortHeader,
      );
      setSortMsg(res.ok ? "Sorted." : res.message);
    }, [sortKey, sortAsc, sortHeader]);

    // New cell style from the selected cell (S-04). Thin glue over
    // session.newCellStyleFromSelection — the engine owns the lowering, the
    // platform owns the style mint + the cell read; this only routes the name.
    const [styleName, setStyleName] = useState("Cell style");
    const [styleMsg, setStyleMsg] = useState<string | null>(null);

    // Chart authoring (editor-ui-coverage M — the model was import-only).
    const [chartKind, setChartKind] = useState("column");
    const [chartValues, setChartValues] = useState("");
    const [chartCats, setChartCats] = useState("");
    const [chartTitle, setChartTitle] = useState("");
    // The §16.4 transpose control: which axis of the values block is a series.
    const [chartSeriesIn, setChartSeriesIn] = useState("columns");
    const [chartMsg, setChartMsg] = useState<string | null>(null);
    const onAuthorChart = useCallback(() => {
      if (!chartValues.trim()) {
        setChartMsg("Enter a values range (for example B2:C8) first.");
        return;
      }
      const res = session.authorChart(
        chartValues.trim(),
        chartCats.trim(),
        chartKind,
        chartTitle.trim(),
        chartSeriesIn,
      );
      setChartMsg(
        res.ok
          ? "Chart created. Place it on the page from the list below; it is not saved into the .xlsx."
          : res.message,
      );
    }, [chartValues, chartCats, chartKind, chartTitle, chartSeriesIn]);

    // Function browser (editor-ui-coverage M): filter + copy-to-clipboard.
    const [fnFilter, setFnFilter] = useState("");
    const [fnMsg, setFnMsg] = useState<string | null>(null);
    const onCopyFunction = useCallback(async (name: string) => {
      if (!host.supports("clipboard@1")) {
        setFnMsg(`Copying isn't available here; type ${name}( in a cell.`);
        return;
      }
      try {
        await host.clipboard.write({ text: `${name}()` });
        setFnMsg(`${name}() copied; paste it into a cell.`);
      } catch (e) {
        setFnMsg(`Couldn't copy ${name}: ${String(e)}.`);
      }
    }, []);
    const onNewCellStyle = useCallback(async () => {
      const res = await session.newCellStyleFromSelection(
        styleName.trim() || "Cell style",
      );
      if (!res.ok) {
        setStyleMsg(res.message);
        return;
      }
      const captured = `Captured ${res.capturedCount} propert${res.capturedCount === 1 ? "y" : "ies"}`;
      setStyleMsg(
        res.applied
          ? `${captured}; style applied to the cell.`
          : `${captured}; ${res.applyMessage ?? "style created (not applied)"}.`,
      );
    }, [styleName]);

    // Memoised in the session until the next change (not per render).
    const sheets = session.sheets();
    // The engine's read-only inventories, read once per session change —
    // not on every render (a keystroke in any of this panel's inputs
    // re-renders it).
    const inventories = useMemo(() => {
      const eng = st.engine;
      if (!eng) return null;
      return {
        freezes: eng.listFreezePanes(),
        validations: eng.listDataValidations(),
        comments: eng.listComments(),
        charts: eng.listCharts(),
      };
    }, [st.engine, version]);
    // The chart-kind vocabulary comes from the ENGINE (Rust owns which kinds
    // exist), never a literal list here.
    const chartKinds = session.chartKinds();
    const hasRange = !!st.selectedRange;
    const deleteLabel = (key: string, label: string) =>
      armed === key ? "Confirm delete" : label;

    return (
      <div
        data-sheet-panel="workbook"
        style={{
          padding: "var(--space-3, 12px)",
          display: "flex",
          flexDirection: "column",
        }}
      >
        <div style={{ ...kicker, marginTop: 0 }}>Workbook</div>
        {/* S-11 RESOLVED: the host file picker is the primary path; the
         *  panel's own `<input>` is the honest fallback when no picker is
         *  wired (headless / a host that predates the door). */}
        <div style={{ ...row, marginTop: 0, marginBottom: "var(--space-2, 8px)" }}>
          {pickerSupported ? (
            <button
              data-sheet-pick
              type="button"
              onClick={() => void onPick()}
              style={primaryButton}
            >
              Open .xlsx or .csv…
            </button>
          ) : (
            <input
              ref={fileRef}
              data-sheet-file
              type="file"
              accept=".xlsx,.csv,.tsv"
              aria-label="Open a workbook"
              onChange={onFile}
              style={{ ...body, maxWidth: "100%" }}
            />
          )}
          <button
            data-sheet-new
            type="button"
            data-armed={armed === "new" ? "" : undefined}
            title={sheets.length > 0 ? "Replaces the open workbook; this can't be undone" : undefined}
            onClick={() => {
              const run = () => void session.newWorkbook();
              if (sheets.length > 0) confirm("new", run);
              else run();
            }}
            style={secondaryButton}
          >
            {armed === "new" ? "Confirm replace" : "New blank workbook"}
          </button>
        </div>

        {/* S-10: engine-boot failure (the wasm isn't built) — say so. */}
        {st.bootError && (
          <div
            data-sheet-boot-error
            role="alert"
            style={{
              ...body,
              padding: "6px 8px",
              color: "var(--status-error, #b00)",
              background: "var(--pg-subtle, var(--pg-bg))",
              border: "1px solid var(--pg-border)",
              borderRadius: "var(--radius-sm, 4px)",
            }}
          >
            The spreadsheet engine could not start: {st.bootError}.
          </div>
        )}

        {!st.bootError && sheets.length === 0 && (
          <p data-sheet-empty style={{ ...note, marginTop: 0 }}>
            Open a workbook or start a blank one to place a table on the page.
          </p>
        )}

        {st.fileName && (
          <p style={{ ...body, margin: "0 0 var(--space-1, 4px)" }}>
            Imported <span style={{ fontFamily: "var(--font-mono, monospace)" }}>{st.fileName}</span>.
          </p>
        )}

        {sheets.length > 0 && (
          <>
            <div style={kicker}>Sheet</div>
            <select
              data-sheet-select
              value={st.activeSheet ?? sheets[0].id}
              aria-label="Active sheet"
              onChange={(e) => session.setActiveSheet(Number(e.target.value))}
              style={input}
            >
              {sheets.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name} ({s.rows} × {s.cols})
                </option>
              ))}
            </select>

            <div style={row}>
              <button
                type="button"
                data-sheet-add
                style={secondaryButton}
                onClick={() => reportSheet(session.addSheet(), "Sheet added.")}
              >
                Add sheet
              </button>
              <input
                data-sheet-rename-to
                type="text"
                value={renameTo}
                onChange={(e) => setRenameTo(e.target.value)}
                placeholder="New name"
                aria-label="New sheet name"
                style={{ ...input, width: 110, marginTop: 0 }}
              />
              <button
                type="button"
                data-sheet-rename
                style={secondaryButton}
                disabled={renameTo.trim().length === 0}
                title={renameTo.trim() ? undefined : "Type a new name first"}
                onClick={() =>
                  reportSheet(
                    session.renameSheet(st.activeSheet ?? 0, renameTo.trim()),
                    "Sheet renamed.",
                  )
                }
              >
                Rename
              </button>
              <button
                type="button"
                data-sheet-delete
                data-armed={armed === "sheet" ? "" : undefined}
                style={secondaryButton}
                disabled={sheets.length <= 1}
                title={sheets.length <= 1 ? "A workbook needs at least one sheet" : "Delete this sheet; this can't be undone"}
                onClick={() =>
                  confirm("sheet", () =>
                    reportSheet(session.deleteSheet(st.activeSheet ?? 0), "Sheet deleted."),
                  )
                }
              >
                {deleteLabel("sheet", "Delete sheet")}
              </button>
            </div>
            {sheetMsg && (
              <div data-sheet-sheet-msg role="status" style={{ ...body, marginTop: 4 }}>
                {sheetMsg}
              </div>
            )}

            <div style={{ height: "var(--space-2, 8px)" }} />

            <Section id="place" title="Place on page" open>
              <label style={{ ...body, display: "flex", flexDirection: "column", gap: 2 }}>
                Range
                <input
                  data-sheet-range
                  type="text"
                  value={st.selectedRange ?? ""}
                  onChange={(e) => session.setRange(e.target.value)}
                  placeholder="A1:D20"
                  style={input}
                />
              </label>
              <div style={{ ...row, marginTop: "var(--space-2, 8px)" }}>
                <button
                  type="button"
                  data-sheet-lower
                  onClick={onPlace}
                  disabled={!hasRange}
                  title={hasRange ? "Place the range on the page as a table" : "Enter a range first"}
                  style={{
                    ...primaryButton,
                    opacity: hasRange ? 1 : 0.5,
                    cursor: hasRange ? "pointer" : "not-allowed",
                  }}
                >
                  Place on page
                </button>
              </div>
              <div style={row}>
                <button
                  type="button"
                  data-sheet-paginate
                  onClick={() => void onPaginate()}
                  disabled={!hasRange}
                  style={secondaryButton}
                  title={hasRange ? "Select a text frame of a threaded chain first" : "Enter a range first"}
                >
                  Paginate into threaded frames
                </button>
                <label
                  style={{ ...body, display: "inline-flex", alignItems: "center", gap: 4 }}
                  title="Rows repeated at the top of every frame of the chain"
                >
                  Header rows
                  <input
                    data-sheet-paginate-header-rows
                    type="number"
                    min={0}
                    value={headerRows}
                    onChange={(e) => setHeaderRows(Number(e.target.value))}
                    style={{ ...input, width: 52 }}
                  />
                </label>
              </div>
              {placeMsg && (
                <div data-sheet-place-msg role="status" style={{ ...body, marginTop: 4 }}>
                  {placeMsg}
                </div>
              )}
            </Section>

            <Section id="structure" title="Rows and columns">
              <p style={{ ...note, marginTop: 0 }}>Acts on the rows or columns selected in the grid.</p>
              <div style={row}>
                <button type="button" data-sheet-insert-rows style={secondaryButton}
                  onClick={() => report(session.structuralEdit("insertRows"), "Rows inserted.")}>
                  Insert rows
                </button>
                <button type="button" data-sheet-delete-rows style={secondaryButton}
                  data-armed={armed === "rows" ? "" : undefined}
                  title="Delete the selected rows; this can't be undone"
                  onClick={() => confirm("rows", () => report(session.structuralEdit("deleteRows"), "Rows deleted."))}>
                  {deleteLabel("rows", "Delete rows")}
                </button>
                <button type="button" data-sheet-insert-cols style={secondaryButton}
                  onClick={() => report(session.structuralEdit("insertCols"), "Columns inserted.")}>
                  Insert columns
                </button>
                <button type="button" data-sheet-delete-cols style={secondaryButton}
                  data-armed={armed === "cols" ? "" : undefined}
                  title="Delete the selected columns; this can't be undone"
                  onClick={() => confirm("cols", () => report(session.structuralEdit("deleteCols"), "Columns deleted."))}>
                  {deleteLabel("cols", "Delete columns")}
                </button>
              </div>
              {structMsg && (
                <div data-sheet-struct-msg role="status" style={{ ...body, marginTop: 4 }}>
                  {structMsg}
                </div>
              )}
            </Section>

            <Section id="format" title="Format and layout">
              <FormatSection
                session={session}
                styles={{ kicker, body, input, button: secondaryButton, row }}
                openProperties={openProperties}
              />
            </Section>

            <Section id="names" title="Names">
              <NamesSection
                session={session}
                styles={{ kicker, body, input, button: secondaryButton, row }}
              />
            </Section>

            {/* Sort range — thin controls over engine.sortRange (the
             *  values-only honest subset; a formula range refuses and the
             *  engine's message shows verbatim). */}
            <Section id="sort" title="Sort range">
              <div style={{ ...row, marginTop: 0 }}>
                <label style={{ ...body, display: "flex", alignItems: "center", gap: 4 }}>
                  Key column
                  <input
                    data-sheet-sort-key
                    type="number"
                    min={1}
                    value={sortKey}
                    onChange={(e) => setSortKey(Number(e.target.value))}
                    title="1 = the first column of the range"
                    style={{ ...input, width: 52 }}
                  />
                </label>
                <select
                  data-sheet-sort-dir
                  value={sortAsc ? "asc" : "desc"}
                  aria-label="Sort direction"
                  onChange={(e) => setSortAsc(e.target.value === "asc")}
                  style={input}
                >
                  <option value="asc">Ascending</option>
                  <option value="desc">Descending</option>
                </select>
                <label style={{ ...body, display: "flex", alignItems: "center", gap: 4 }}>
                  <input
                    data-sheet-sort-header
                    type="checkbox"
                    checked={sortHeader}
                    onChange={(e) => setSortHeader(e.target.checked)}
                  />
                  Has header row
                </label>
              </div>
              <div style={row}>
                <button
                  type="button"
                  data-sheet-sort
                  onClick={onSort}
                  disabled={!hasRange}
                  title={hasRange ? undefined : "Enter a range first"}
                  style={{
                    ...primaryButton,
                    opacity: hasRange ? 1 : 0.5,
                    cursor: hasRange ? "pointer" : "not-allowed",
                  }}
                >
                  Sort
                </button>
              </div>
              {sortMsg && (
                <p data-sheet-sort-msg style={{ ...body, margin: "var(--space-1, 4px) 0 0" }}>
                  {sortMsg}
                </p>
              )}
            </Section>

            {/* New cell style from the selected cell (S-04): the style is
             *  minted + populated from the cell's appearance and applied
             *  back to the cell (the message reports whether the apply took). */}
            <Section id="cell-style" title="Cell styles">
              <input
                data-sheet-cellstyle-name
                type="text"
                value={styleName}
                onChange={(e) => setStyleName(e.target.value)}
                placeholder="Style name"
                aria-label="Style name"
                style={input}
              />
              <div style={row}>
                <button
                  type="button"
                  data-sheet-cellstyle-new
                  onClick={() => void onNewCellStyle()}
                  style={secondaryButton}
                >
                  New style from selected cell
                </button>
              </div>
              {styleMsg && (
                <p data-sheet-cellstyle-msg style={{ ...body, margin: "var(--space-1, 4px) 0 0" }}>
                  {styleMsg}
                </p>
              )}
              <p data-sheet-cellstyle-note style={note}>
                Takes the fill and borders of the selected cell in a placed table.
              </p>
            </Section>

            {/* CHART AUTHORING — the chart engine was import-only. Authored
             *  charts render + place through the SAME generator as parsed
             *  ones, and stay PAGE-side (never written back to the xlsx —
             *  the publishing-first ruling, asserted in conformance).
             *  The kind list is READ FROM THE ENGINE (session.chartKinds),
             *  never hard-coded here: which kinds exist is a Rust decision,
             *  so the panel cannot offer one the engine refuses. */}
            <Section id="charts" title="Charts">
              <div style={{ ...row, marginTop: 0 }}>
                <select
                  data-sheet-chart-kind
                  value={chartKind}
                  aria-label="Chart type"
                  onChange={(e) => setChartKind(e.target.value)}
                  style={input}
                >
                  {chartKinds.map((k) => (
                    <option key={k} value={k}>
                      {labelOf(k)}
                    </option>
                  ))}
                </select>
                {/* Transpose rows/columns: which axis of the values block is a
                 *  series. Re-reads the same cells — never rewrites data. */}
                <select
                  data-sheet-chart-series-in
                  value={chartSeriesIn}
                  onChange={(e) => setChartSeriesIn(e.target.value)}
                  title="Which axis of the values block is a series"
                  aria-label="Series in"
                  style={input}
                >
                  <option value="columns">Series in columns</option>
                  <option value="rows">Series in rows</option>
                </select>
              </div>
              <div style={row}>
                <input
                  data-sheet-chart-values
                  type="text"
                  value={chartValues}
                  onChange={(e) => setChartValues(e.target.value)}
                  placeholder="Values, e.g. B2:C8"
                  aria-label="Values range"
                  style={{ ...input, flex: "1 1 110px" }}
                />
                <input
                  data-sheet-chart-cats
                  type="text"
                  value={chartCats}
                  onChange={(e) => setChartCats(e.target.value)}
                  placeholder="Labels (optional)"
                  aria-label="Labels range"
                  style={{ ...input, flex: "1 1 110px" }}
                />
              </div>
              <div style={row}>
                <input
                  data-sheet-chart-title
                  type="text"
                  value={chartTitle}
                  onChange={(e) => setChartTitle(e.target.value)}
                  placeholder="Title (optional)"
                  aria-label="Chart title"
                  style={{ ...input, flex: "1 1 110px" }}
                />
                <button
                  type="button"
                  data-sheet-chart-author
                  onClick={onAuthorChart}
                  style={secondaryButton}
                >
                  Create chart
                </button>
              </div>
              {chartMsg && (
                <p data-sheet-chart-msg style={{ ...body, margin: "var(--space-1, 4px) 0 0" }}>
                  {chartMsg}
                </p>
              )}
              {(() => {
                if (!inventories) return null;
                const { charts } = inventories;
                if (charts.length === 0) return null;
                return (
                  <div data-sheet-chart-list style={{ marginTop: "var(--space-2, 8px)" }}>
                    {charts.map((c) => (
                      <div
                        key={c.index}
                        style={{ display: "flex", alignItems: "center", gap: 6, padding: "2px 0", ...body }}
                      >
                        <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                          {c.title || `${labelOf(c.kind)} chart`}
                          <span style={{ color: "var(--pg-muted-fg)" }}>
                            {" "}· {c.title ? `${labelOf(c.kind)}, ` : ""}
                            {c.seriesCount} series
                          </span>
                        </span>
                        <button
                          type="button"
                          data-sheet-chart-lower={c.index}
                          onClick={() => void session.lowerChart(c.index)}
                          style={secondaryButton}
                        >
                          Place on page
                        </button>
                      </div>
                    ))}
                  </div>
                );
              })()}
            </Section>

            {/* FUNCTION BROWSER — the 224-function library was reachable only
             *  by typing into the grid formula bar. Searchable, grouped by
             *  family, arg counts from the engine's own registry (never a
             *  hand-kept list). Click copies `NAME(` for pasting into a cell
             *  when the clipboard door is wired. */}
            <Section id="functions" title="Functions">
              <input
                data-sheet-fn-filter
                type="text"
                value={fnFilter}
                onChange={(e) => setFnFilter(e.target.value)}
                placeholder="Filter functions"
                aria-label="Filter functions"
                style={input}
              />
              {fnMsg && (
                <p role="status" style={{ ...body, margin: "var(--space-1, 4px) 0 0" }}>{fnMsg}</p>
              )}
              <div
                data-sheet-fn-list
                style={{ maxHeight: 180, overflowY: "auto", marginTop: "var(--space-1, 4px)" }}
              >
                {(() => {
                  if (!st.engine) return null;
                  // The registry, read ONCE per session (it is build-time
                  // fixed) — a filter keystroke re-filters, never re-reads.
                  const all = session.functionList();
                  const q = fnFilter.trim().toUpperCase();
                  const hitsFn = q
                    ? all.filter(
                        (f) =>
                          f.name.includes(q) || f.family.toUpperCase().includes(q),
                      )
                    : all;
                  const byFamily = new Map<string, FunctionEntry[]>();
                  for (const f of hitsFn) {
                    const list = byFamily.get(f.family) ?? [];
                    list.push(f);
                    byFamily.set(f.family, list);
                  }
                  return [...byFamily.entries()].map(([family, fns]) => (
                    <div key={family}>
                      <div
                        style={{
                          font: "600 11px var(--font-sans, sans-serif)",
                          color: "var(--pg-muted-fg)",
                          margin: "var(--space-1, 4px) 0 2px",
                        }}
                      >
                        {labelOf(family)} ({fns.length})
                      </div>
                      {fns.map((f) => (
                        <button
                          key={f.name}
                          type="button"
                          data-sheet-fn={f.name}
                          title={`${f.name}: ${f.minArgs}${
                            f.maxArgs === null
                              ? " or more"
                              : f.maxArgs === f.minArgs
                                ? ""
                                : ` to ${f.maxArgs}`
                          } argument${f.maxArgs === 1 && f.minArgs === 1 ? "" : "s"}. Click to copy.`}
                          onClick={() => void onCopyFunction(f.name)}
                          style={{
                            display: "block",
                            width: "100%",
                            textAlign: "left",
                            border: "none",
                            background: "none",
                            cursor: "pointer",
                            padding: "1px 0",
                            fontSize: 11,
                            fontFamily: "var(--font-mono, monospace)",
                            color: "var(--pg-fg)",
                          }}
                        >
                          {f.name}
                        </button>
                      ))}
                    </div>
                  ));
                })()}
              </div>
            </Section>

            {/* INSPECT — the three read-only inventories the engine has
             *  carried all along (freeze panes, data validations, comments).
             *  Preserve-first honesty: validations are preserved, never
             *  enforced; comments are display-only. */}
            {(() => {
              if (!inventories) return null;
              const { freezes, validations, comments } = inventories;
              if (
                freezes.length === 0 &&
                validations.length === 0 &&
                comments.length === 0
              ) {
                return null;
              }
              return (
                <Section id="inventory" title="Workbook contents">
                  <div data-sheet-inspect>
                    {freezes.map((f, i) => (
                      <p key={`f${i}`} data-sheet-freeze style={{ ...body, margin: 0 }}>
                        {sheets[f.sheet]?.name ?? `Sheet ${f.sheet + 1}`}: frozen{" "}
                        {f.rows} row{f.rows === 1 ? "" : "s"}, {f.cols} column
                        {f.cols === 1 ? "" : "s"}
                      </p>
                    ))}
                    {validations.map((v, i) => (
                      <p key={`v${i}`} data-sheet-validation style={{ ...body, margin: 0 }}>
                        {sheets[v.sheet]?.name ?? `Sheet ${v.sheet + 1}`}: {v.count}{" "}
                        data validation{v.count === 1 ? "" : "s"} (
                        {v.kinds.join(", ")}), kept but not enforced
                      </p>
                    ))}
                    {comments.length > 0 && (
                      <p data-sheet-comments style={{ ...body, margin: 0 }}>
                        {comments.length} comment{comments.length === 1 ? "" : "s"} (read-only):{" "}
                        {comments
                          .slice(0, 3)
                          .map((c) => `${c.author || "Unknown"} at ${columnLabel(c.col)}${c.row + 1}`)
                          .join(" · ")}
                        {comments.length > 3 ? " · …" : ""}
                      </p>
                    )}
                  </div>
                </Section>
              );
            })()}

            <p data-sheet-find-pointer style={{ ...note, marginTop: "var(--space-2, 8px)" }}>
              Find and replace are in the{" "}
              <button
                type="button"
                data-sheet-open-grid
                onClick={() => {
                  host.shell.openPanel(GRID_PANEL_ID);
                  session.requestFind();
                }}
                style={linkButton}
              >
                Grid panel
              </button>
              .
            </p>
          </>
        )}

        {/* S-08 / .paged container: be honest about where the workbook lives —
         *  inside the .paged document (travels with the file) when host.parts
         *  is wired, else this browser's local store, else in-memory. */}
        <p data-sheet-honesty style={{ ...note, marginTop: "var(--space-2, 8px)" }}>
          {host.supports("storage.parts@1")
            ? "The workbook is saved inside the document and travels with the .paged file."
            : host.supports("storage.blob@1")
              ? "The workbook is saved in this browser and restored on reload."
              : "The workbook lives in memory only; open it again after a reload."}
        </p>
      </div>
    );
  };
}
