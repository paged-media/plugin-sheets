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
// host file picker), the sheet list, the range input, the "Lower to
// frame" action, the S-08 in-memory honesty notice, and the engine-boot
// failure state (S-10) — rendered honestly, never faked.
//
// Built from host surfaces + React ONLY (no @paged-media/shell): all
// spreadsheet work is the engine's; the panel reads the session snapshot
// and triggers session actions. Token-layer styling (--pg-*, --space-*,
// --font-mono, --radius-*, sentence-case labels) mirrors plugin-web's
// source panel so it reads native in both themes.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactElement,
} from "react";

import type { BundleHost } from "@paged-media/plugin-api";

import type { FindMatch } from "../engine";
import { XLSX_MIME } from "../import-xlsx";
import { columnLabel, type WorkbookSession } from "../session";

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

const input: CSSProperties = {
  font: "12px var(--font-mono, monospace)",
  color: "var(--pg-fg)",
  background: "var(--pg-bg)",
  border: "1px solid var(--pg-border)",
  borderRadius: "var(--radius-sm, 4px)",
  padding: "4px 6px",
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

// ----------------------------------------------------------------- panel

export function makeWorkbookPanel(
  host: BundleHost,
  session: WorkbookSession,
): () => ReactElement {
  return function WorkbookPanel(): ReactElement {
    // Re-render on every session change (import, sheet/range edits).
    const [, force] = useState(0);
    const fileRef = useRef<HTMLInputElement | null>(null);
    useEffect(() => {
      const sub = session.onDidChange(() => force((n) => n + 1));
      return () => sub.dispose();
    }, []);

    const st = session.state();

    const onFile = useCallback(
      async (e: React.ChangeEvent<HTMLInputElement>) => {
        const file = e.target.files?.[0];
        if (!file) return;
        try {
          const bytes = new Uint8Array(await file.arrayBuffer());
          await session.import(bytes, file.name);
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
          accept: [".xlsx", XLSX_MIME],
          multiple: false,
        });
        const file = files[0];
        if (!file) return; // cancelled
        await session.import(file.bytes, file.name);
      } catch (err) {
        host.log.error("workbook pick failed", err);
      }
    }, []);

    const onLower = useCallback(() => {
      void session.lowerSelection();
    }, []);

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

    // Find & replace controls (sheet.plugin.find-replace.panel). Hits are a
    // snapshot — a later edit/replace invalidates them, so actions clear it.
    const [needle, setNeedle] = useState("");
    const [replacement, setReplacement] = useState("");
    const [matchCase, setMatchCase] = useState(false);
    const [entireCell, setEntireCell] = useState(false);
    const [inFormulas, setInFormulas] = useState(false);
    const [hits, setHits] = useState<FindMatch[] | null>(null);
    const [findMsg, setFindMsg] = useState<string | null>(null);
    const onFind = useCallback(() => {
      if (!needle) return;
      const found = session.findAll(
        needle,
        { matchCase, entireCell, inFormulas },
        "sheet",
      );
      setHits(found);
      setFindMsg(`${found.length} hit${found.length === 1 ? "" : "s"}.`);
    }, [needle, matchCase, entireCell, inFormulas]);
    const onReplaceAll = useCallback(() => {
      if (!needle) return;
      const res = session.replaceAll(
        needle,
        replacement,
        { matchCase, entireCell, inFormulas },
        "sheet",
      );
      setHits(null); // stale after a replace
      setFindMsg(
        "error" in res
          ? res.error
          : `Replaced ${res.occurrences} occurrence${res.occurrences === 1 ? "" : "s"} in ${res.replacedCells} cell${res.replacedCells === 1 ? "" : "s"}` +
              (res.skipped > 0 ? ` (${res.skipped} skipped).` : "."),
      );
    }, [needle, replacement, matchCase, entireCell, inFormulas]);

    // New cell style from the selected cell (S-04). Thin glue over
    // session.newCellStyleFromSelection — the engine owns the lowering, the
    // platform owns the style mint + the cell read; this only routes the name.
    const [styleName, setStyleName] = useState("Cell style");
    const [styleMsg, setStyleMsg] = useState<string | null>(null);
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

    const sheets = st.engine ? st.engine.listSheets() : [];

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
        {pickerSupported ? (
          <button
            data-sheet-pick
            type="button"
            onClick={() => void onPick()}
            style={{
              ...primaryButton,
              alignSelf: "flex-start",
              marginBottom: "var(--space-2, 8px)",
            }}
          >
            Choose .xlsx…
          </button>
        ) : (
          <input
            ref={fileRef}
            data-sheet-file
            type="file"
            accept=".xlsx"
            onChange={onFile}
            style={{ ...body, marginBottom: "var(--space-2, 8px)" }}
          />
        )}

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
            Engine unavailable — {st.bootError}.
          </div>
        )}

        {st.fileName && (
          <p style={{ ...body, margin: "0 0 var(--space-1, 4px)" }}>
            Imported <span style={{ font: "12px var(--font-mono, monospace)" }}>{st.fileName}</span>.
          </p>
        )}

        {sheets.length > 0 && (
          <>
            <div style={kicker}>Sheet</div>
            <select
              data-sheet-select
              value={st.activeSheet ?? sheets[0].id}
              onChange={(e) => session.setActiveSheet(Number(e.target.value))}
              style={input}
            >
              {sheets.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name} ({s.rows}×{s.cols})
                </option>
              ))}
            </select>

            <div style={kicker}>Range</div>
            <input
              data-sheet-range
              type="text"
              value={st.selectedRange ?? ""}
              onChange={(e) => session.setRange(e.target.value)}
              placeholder="A1:D20"
              style={input}
            />

            <div style={{ marginTop: "var(--space-3, 12px)" }}>
              <button
                type="button"
                data-sheet-lower
                onClick={onLower}
                disabled={!st.selectedRange}
                style={{
                  ...primaryButton,
                  opacity: st.selectedRange ? 1 : 0.5,
                  cursor: st.selectedRange ? "pointer" : "not-allowed",
                }}
              >
                Lower to frame
              </button>
            </div>

            {/* Sort range — thin controls over engine.sortRange (the
             *  values-only honest subset; a formula range refuses and the
             *  engine's message shows verbatim). */}
            <div style={kicker}>Sort range</div>
            <div
              style={{
                display: "flex",
                gap: "var(--space-1, 4px)",
                alignItems: "center",
              }}
            >
              <input
                data-sheet-sort-key
                type="number"
                min={1}
                value={sortKey}
                onChange={(e) => setSortKey(Number(e.target.value))}
                title="Key column (1 = first column of the range)"
                style={{ ...input, width: 48 }}
              />
              <select
                data-sheet-sort-dir
                value={sortAsc ? "asc" : "desc"}
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
                Header
              </label>
            </div>
            <div style={{ marginTop: "var(--space-1, 4px)" }}>
              <button
                type="button"
                data-sheet-sort
                onClick={onSort}
                disabled={!st.selectedRange}
                style={{
                  ...primaryButton,
                  opacity: st.selectedRange ? 1 : 0.5,
                  cursor: st.selectedRange ? "pointer" : "not-allowed",
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

            {/* Find & replace — thin glue over engine.findAll/replaceAll
             *  (active-sheet scope; click a hit to select its cell). */}
            <div style={kicker}>Find &amp; replace</div>
            <input
              data-sheet-find-needle
              type="text"
              value={needle}
              onChange={(e) => setNeedle(e.target.value)}
              placeholder="Find…"
              style={input}
            />
            <input
              data-sheet-find-replacement
              type="text"
              value={replacement}
              onChange={(e) => setReplacement(e.target.value)}
              placeholder="Replace with…"
              style={{ ...input, marginTop: "var(--space-1, 4px)" }}
            />
            <div
              style={{
                display: "flex",
                gap: "var(--space-2, 8px)",
                marginTop: "var(--space-1, 4px)",
                flexWrap: "wrap",
              }}
            >
              <label style={{ ...body, display: "flex", alignItems: "center", gap: 4 }}>
                <input
                  data-sheet-find-case
                  type="checkbox"
                  checked={matchCase}
                  onChange={(e) => setMatchCase(e.target.checked)}
                />
                Match case
              </label>
              <label style={{ ...body, display: "flex", alignItems: "center", gap: 4 }}>
                <input
                  data-sheet-find-entire
                  type="checkbox"
                  checked={entireCell}
                  onChange={(e) => setEntireCell(e.target.checked)}
                />
                Entire cell
              </label>
              <label style={{ ...body, display: "flex", alignItems: "center", gap: 4 }}>
                <input
                  data-sheet-find-formulas
                  type="checkbox"
                  checked={inFormulas}
                  onChange={(e) => setInFormulas(e.target.checked)}
                />
                In formulas
              </label>
            </div>
            <div
              style={{
                display: "flex",
                gap: "var(--space-1, 4px)",
                marginTop: "var(--space-1, 4px)",
              }}
            >
              <button
                type="button"
                data-sheet-find
                onClick={onFind}
                disabled={!needle}
                style={{
                  ...primaryButton,
                  opacity: needle ? 1 : 0.5,
                  cursor: needle ? "pointer" : "not-allowed",
                }}
              >
                Find
              </button>
              <button
                type="button"
                data-sheet-replace-all
                onClick={onReplaceAll}
                disabled={!needle}
                style={{
                  ...primaryButton,
                  opacity: needle ? 1 : 0.5,
                  cursor: needle ? "pointer" : "not-allowed",
                }}
              >
                Replace all
              </button>
            </div>
            {findMsg && (
              <p data-sheet-find-msg style={{ ...body, margin: "var(--space-1, 4px) 0 0" }}>
                {findMsg}
              </p>
            )}
            {hits && hits.length > 0 && (
              <ul
                data-sheet-find-hits
                style={{
                  listStyle: "none",
                  margin: "var(--space-1, 4px) 0 0",
                  padding: 0,
                  maxHeight: 160,
                  overflowY: "auto",
                  border: "1px solid var(--pg-border)",
                  borderRadius: "var(--radius-sm, 4px)",
                }}
              >
                {hits.map((h, i) => (
                  <li key={`${h.sheet}:${h.row}:${h.col}:${i}`}>
                    <button
                      type="button"
                      data-sheet-find-hit={`${h.row}:${h.col}`}
                      onClick={() => session.goToCell(h.sheet, h.row, h.col)}
                      style={{
                        ...body,
                        display: "block",
                        width: "100%",
                        textAlign: "left",
                        background: "none",
                        border: "none",
                        padding: "3px 6px",
                        cursor: "pointer",
                      }}
                    >
                      <span style={{ font: "11px var(--font-mono, monospace)" }}>
                        {columnLabel(h.col)}
                        {h.row + 1}
                      </span>{" "}
                      {h.excerpt}
                    </button>
                  </li>
                ))}
              </ul>
            )}

            {/* New cell style from the selected cell (S-04). Honest about the
             *  residual: the style is minted + populated from the cell's
             *  appearance; applying it BACK is wire-shape-only today (the
             *  message reports whether the apply took). */}
            <div style={kicker}>New cell style</div>
            <input
              data-sheet-cellstyle-name
              type="text"
              value={styleName}
              onChange={(e) => setStyleName(e.target.value)}
              placeholder="Style name"
              style={input}
            />
            <div style={{ marginTop: "var(--space-1, 4px)" }}>
              <button
                type="button"
                data-sheet-cellstyle-new
                onClick={() => void onNewCellStyle()}
                style={primaryButton}
              >
                New style from selected cell
              </button>
            </div>
            {styleMsg && (
              <p data-sheet-cellstyle-msg style={{ ...body, margin: "var(--space-1, 4px) 0 0" }}>
                {styleMsg}
              </p>
            )}
            <p
              data-sheet-cellstyle-note
              style={{
                margin: "var(--space-1, 4px) 0 0",
                font: "10px/1.5 var(--font-sans, sans-serif)",
                color: "var(--pg-muted-fg)",
              }}
            >
              Captures fill + borders from the selected cell over a lowered
              table. Applying the style to cells is pending the platform Table
              style surface.
            </p>
          </>
        )}

        {/* S-08 / .paged container: be honest about where the workbook lives —
         *  inside the .paged document (travels with the file) when host.parts
         *  is wired, else this browser's local store, else in-memory. */}
        <p
          data-sheet-honesty
          style={{
            margin: "var(--space-3, 12px) 0 0",
            font: "10px/1.5 var(--font-sans, sans-serif)",
            color: "var(--pg-muted-fg)",
          }}
        >
          {host.supports("storage.parts@1")
            ? "Workbook is saved inside the document and travels with the .paged file. The frame binding persists with it."
            : host.supports("storage.blob@1")
              ? "Workbook is saved to this browser's local store and restored on reload. The frame binding persists with the document."
              : "Workbook lives in memory — re-import after reload. The frame binding persists with the document."}
        </p>
      </div>
    );
  };
}
