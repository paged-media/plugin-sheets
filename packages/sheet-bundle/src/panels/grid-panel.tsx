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

// The grid panel — the INTERIM sheets-mode grid (spec §8.1, S-02). This is
// the panel-hosted grid, NOT sheets-mode-in-frame (still SDK-blocked: S-01
// editContext/objectType throw; S-02 the in-frame Vello surface is a gap).
// The panel paints the engine's already-windowed GridScene as an SVG over
// the active sheet, scrolls by re-requesting a window, click-selects a
// cell, and overlays an <input> editor that commits on Enter.
//
// A React expert-leaf factory closing over BundleHost + the session, built
// from host surfaces + React ONLY (no @paged-media/shell). ALL spreadsheet
// work is the engine's (windowing, formatting, recalc happen in Rust); the
// panel reads the session's scene snapshot and the PURE geometry helpers
// (gridSceneToSvg / hitCell / cellEditorRect) from sheet-host-model and
// triggers session actions (setGridSelection, editCell). Token-layer
// styling (--pg-*, --space-*, --font-mono, --radius-*) mirrors the
// workbook panel so it reads native in both themes.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactElement,
} from "react";

import type { BundleHost } from "@paged-media/plugin-api";
import {
  DEFAULT_GRID_SVG_OPTIONS,
  applyCompletion,
  arityHint,
  cellEditorRect,
  completionTokenAt,
  gridSceneToSvg,
  hitCell,
  matchFunctions,
  type FunctionEntry,
  type GridScene,
} from "@paged-media/sheet-host-model";

import { columnLabel, type WorkbookSession } from "../session";

// ---------------------------------------------------------------- styles

const kicker: CSSProperties = {
  font: "700 10px var(--font-sans, sans-serif)",
  letterSpacing: "var(--tracking-wide, 0.14em)",
  textTransform: "uppercase",
  color: "var(--pg-muted-fg)",
  margin: "0 0 var(--space-2, 8px)",
};

const note: CSSProperties = {
  margin: "var(--space-3, 12px) 0 0",
  font: "10px/1.5 var(--font-sans, sans-serif)",
  color: "var(--pg-muted-fg)",
};

const editorInput: CSSProperties = {
  position: "absolute",
  boxSizing: "border-box",
  font: "12px var(--font-mono, monospace)",
  color: "var(--pg-fg)",
  background: "var(--pg-bg)",
  border: "1px solid var(--pg-primary)",
  borderRadius: 0,
  padding: "0 3px",
  margin: 0,
  outline: "none",
};

const formulaBarRow: CSSProperties = {
  display: "flex",
  alignItems: "stretch",
  gap: "var(--space-1, 4px)",
  position: "relative",
};

const cellRefBadge: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  minWidth: 44,
  padding: "0 6px",
  font: "11px var(--font-mono, monospace)",
  color: "var(--pg-muted-fg)",
  background: "var(--pg-subtle, var(--pg-bg))",
  border: "1px solid var(--pg-border)",
  borderRadius: "var(--radius-sm, 4px)",
};

const formulaInput: CSSProperties = {
  flex: 1,
  boxSizing: "border-box",
  font: "12px var(--font-mono, monospace)",
  color: "var(--pg-fg)",
  background: "var(--pg-bg)",
  border: "1px solid var(--pg-border)",
  borderRadius: "var(--radius-sm, 4px)",
  padding: "4px 6px",
  outline: "none",
};

const completionList: CSSProperties = {
  position: "absolute",
  top: "calc(100% + 2px)",
  left: 48,
  right: 0,
  zIndex: 2,
  listStyle: "none",
  margin: 0,
  padding: 0,
  maxHeight: 180,
  overflowY: "auto",
  background: "var(--pg-bg)",
  border: "1px solid var(--pg-border)",
  borderRadius: "var(--radius-sm, 4px)",
  boxShadow: "var(--shadow-md, 0 4px 12px rgba(0,0,0,0.18))",
};

// The viewport size the grid windows into (content-space pt). The interim
// panel is a fixed scroll box; the in-frame surface (S-02) will size to the
// frame's content box. Scroll steps move the origin one row/col at a time
// (the engine re-windows from there — virtualization keeps it cheap).
const VIEWPORT_W_PT = 480;
const VIEWPORT_H_PT = 320;

/** The pixel size the SVG renders at (1 pt = 1 px in the interim panel; the
 *  in-frame surface honours the document zoom). */
const PX_PER_PT = 1;

interface EditorState {
  row: number;
  col: number;
  value: string;
}

export function makeGridPanel(
  host: BundleHost,
  session: WorkbookSession,
): () => ReactElement {
  return function GridPanel(): ReactElement {
    // Re-render on session changes (selection set, cell committed, import).
    const [, force] = useState(0);
    useEffect(() => {
      const sub = session.onDidChange(() => force((n) => n + 1));
      return () => sub.dispose();
    }, []);

    // Scroll origin (the window's top-left cell). The engine windows from
    // here; scrolling re-requests a new window (virtualization in Rust).
    const [firstRow, setFirstRow] = useState(0);
    const [firstCol, setFirstCol] = useState(0);
    const [editor, setEditor] = useState<EditorState | null>(null);
    const editorRef = useRef<HTMLInputElement | null>(null);

    const st = session.state();

    // ── Formula bar (S-04) — bound to the selected cell. The bar shows the
    // cell's re-enterable INPUT (formula or literal) and commits through the
    // journaled editCell lane. Autocomplete proposes engine-registry function
    // names for the token under the caret (no spreadsheet semantics in TS —
    // the names ARE the engine's, the matching is pure prefix). ──────────────
    const sel = st.gridSelection;
    const fbRow = sel?.anchorRow ?? null;
    const fbCol = sel?.anchorCol ?? null;
    // The bar tracks the selected cell unless the user is actively editing it
    // (`fbDraft` non-null). Switching cells discards an uncommitted draft —
    // the documented "adjust state while rendering" pattern: `draftCell`
    // records which cell the open draft belongs to (set when editing starts);
    // when the selection moves off it, the draft is cleared.
    const [fbDraft, setFbDraft] = useState<string | null>(null);
    const [draftCell, setDraftCell] = useState<string>("");
    const fbCellKey = fbRow !== null && fbCol !== null ? `${fbRow}:${fbCol}` : "";
    if (fbDraft !== null && fbCellKey !== draftCell) {
      // Selection moved off the cell being edited — discard the draft.
      setFbDraft(null);
    }
    /** Begin/continue a draft on the CURRENT selected cell. */
    const startDraft = useCallback(
      (value: string) => {
        setFbDraft(value);
        setDraftCell(fbCellKey);
      },
      [fbCellKey],
    );
    const fbValue =
      fbDraft ??
      (fbRow !== null && fbCol !== null ? session.cellInputAt(fbRow, fbCol) : "");

    const fbInputRef = useRef<HTMLInputElement | null>(null);
    const [fbCaret, setFbCaret] = useState(0);
    const [activeCompletion, setActiveCompletion] = useState(0);

    // Completions for the token under the caret (pure prefix match over the
    // engine's registry table — empty when the token is empty / no engine).
    const functions: readonly FunctionEntry[] = st.engine
      ? session.functionList()
      : [];
    const token = completionTokenAt(fbValue, fbCaret);
    const completions: FunctionEntry[] =
      fbDraft !== null && token.text.length > 0
        ? matchFunctions(functions, token.text)
        : [];
    // Keep the active index in range as the list shrinks/grows.
    const activeIdx =
      completions.length === 0
        ? 0
        : Math.min(activeCompletion, completions.length - 1);

    const commitFormula = useCallback(
      (value: string) => {
        if (fbRow === null || fbCol === null || st.activeSheet === null) return;
        // Journaled lane — one undoable step, dirty cut recomputed in Rust.
        session.editCell(st.activeSheet, fbRow, fbCol, value);
        setFbDraft(null);
      },
      [fbRow, fbCol, st.activeSheet],
    );

    const chooseCompletion = useCallback(
      (entry: FunctionEntry) => {
        const next = applyCompletion(fbValue, token, entry.name);
        startDraft(next.value);
        setFbCaret(next.caret);
        // Restore focus + caret after the controlled re-render.
        requestAnimationFrame(() => {
          const el = fbInputRef.current;
          if (el) {
            el.focus();
            el.setSelectionRange(next.caret, next.caret);
          }
        });
      },
      [fbValue, token, startDraft],
    );

    const onFormulaKey = useCallback(
      (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (completions.length > 0) {
          if (e.key === "ArrowDown") {
            e.preventDefault();
            setActiveCompletion((i) => (i + 1) % completions.length);
            return;
          }
          if (e.key === "ArrowUp") {
            e.preventDefault();
            setActiveCompletion(
              (i) => (i - 1 + completions.length) % completions.length,
            );
            return;
          }
          if (e.key === "Tab") {
            e.preventDefault();
            chooseCompletion(completions[activeIdx]);
            return;
          }
        }
        if (e.key === "Enter") {
          e.preventDefault();
          // Enter accepts an open completion if one is highlighted by Tab/arrow
          // navigation; otherwise it commits the formula.
          commitFormula(fbValue);
        } else if (e.key === "Escape") {
          e.preventDefault();
          setFbDraft(null);
        }
      },
      [completions, activeIdx, chooseCompletion, commitFormula, fbValue],
    );

    const onFormulaChange = useCallback(
      (e: React.ChangeEvent<HTMLInputElement>) => {
        startDraft(e.target.value);
        setFbCaret(e.target.selectionStart ?? e.target.value.length);
        setActiveCompletion(0);
      },
      [startDraft],
    );

    // The engine-windowed scene for the current origin (null before import
    // or when windowing fails — the panel says so).
    const scene: GridScene | null =
      st.activeSheet !== null
        ? session.gridScene(firstRow, firstCol, VIEWPORT_W_PT, VIEWPORT_H_PT)
        : null;

    // Keep the editor focused while open.
    useEffect(() => {
      if (editor) editorRef.current?.focus();
    }, [editor?.row, editor?.col]);

    const onSvgClick = useCallback(
      (e: React.MouseEvent<SVGSVGElement>) => {
        if (!scene) return;
        const rect = e.currentTarget.getBoundingClientRect();
        // Pixel → content-space pt (the SVG viewBox is pt; it renders at
        // PX_PER_PT, so divide back out).
        const xPt = (e.clientX - rect.left) / PX_PER_PT;
        const yPt = (e.clientY - rect.top) / PX_PER_PT;
        const hit = hitCell(scene, xPt, yPt);
        if (!hit) return;
        session.setGridSelection(hit.row, hit.col, 1, 1);
      },
      [scene],
    );

    const onSvgDoubleClick = useCallback(
      (e: React.MouseEvent<SVGSVGElement>) => {
        if (!scene || st.activeSheet === null) return;
        const rect = e.currentTarget.getBoundingClientRect();
        const xPt = (e.clientX - rect.left) / PX_PER_PT;
        const yPt = (e.clientY - rect.top) / PX_PER_PT;
        const hit = hitCell(scene, xPt, yPt);
        if (!hit) return;
        session.setGridSelection(hit.row, hit.col, 1, 1);
        const current =
          scene.cells.find((c) => c.row === hit.row && c.col === hit.col)
            ?.text ?? "";
        setEditor({ row: hit.row, col: hit.col, value: current });
      },
      [scene, st.activeSheet],
    );

    const commitEditor = useCallback(() => {
      if (!editor || st.activeSheet === null) {
        setEditor(null);
        return;
      }
      const ok = session.editCell(
        st.activeSheet,
        editor.row,
        editor.col,
        editor.value,
      );
      if (!ok) host.log.warn("grid: cell edit was not committed");
      setEditor(null);
    }, [editor, st.activeSheet]);

    const onEditorKey = useCallback(
      (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (e.key === "Enter") {
          e.preventDefault();
          commitEditor();
        } else if (e.key === "Escape") {
          e.preventDefault();
          setEditor(null);
        }
      },
      [commitEditor],
    );

    // No engine / sheet yet — the honest empty state (import in Workbook).
    if (st.activeSheet === null || !scene) {
      return (
        <div
          data-sheet-panel="grid"
          style={{ padding: "var(--space-3, 12px)" }}
        >
          <div style={kicker}>Grid</div>
          <p style={{ ...note, marginTop: 0 }}>
            {st.bootError
              ? `Engine unavailable — ${st.bootError}.`
              : "Import a workbook to edit it in the grid."}
          </p>
        </div>
      );
    }

    const o = {
      ...DEFAULT_GRID_SVG_OPTIONS,
      gridColor: "var(--pg-border)",
      textColor: "var(--pg-fg)",
      selectionColor: "var(--pg-primary)",
    };
    const svg = gridSceneToSvg(scene, o);
    const wPx = (scene.viewport.xOffsets.at(-1) ?? 0) * PX_PER_PT;
    const hPx = (scene.viewport.yOffsets.at(-1) ?? 0) * PX_PER_PT;

    // The editor overlay rect (content-space pt → px), or null.
    const edRect = editor
      ? cellEditorRect(scene, editor.row, editor.col)
      : null;

    return (
      <div
        data-sheet-panel="grid"
        style={{
          padding: "var(--space-3, 12px)",
          display: "flex",
          flexDirection: "column",
          gap: "var(--space-2, 8px)",
        }}
      >
        <div style={kicker}>Grid</div>

        {/* Formula bar (S-04) — bound to the selected cell; autocompletes
            engine-registry function names. Disabled until a cell is selected. */}
        <div data-sheet-formula-bar style={formulaBarRow}>
          <span data-formula-cellref style={cellRefBadge}>
            {fbRow !== null && fbCol !== null
              ? `${columnLabel(fbCol)}${fbRow + 1}`
              : "—"}
          </span>
          <input
            ref={fbInputRef}
            data-formula-input
            type="text"
            value={fbValue}
            disabled={fbRow === null || fbCol === null}
            placeholder={
              fbRow === null ? "Select a cell to edit its formula" : "="
            }
            onChange={onFormulaChange}
            onKeyDown={onFormulaKey}
            onSelect={(e) =>
              setFbCaret(e.currentTarget.selectionStart ?? fbValue.length)
            }
            style={{
              ...formulaInput,
              opacity: fbRow === null ? 0.6 : 1,
            }}
          />
          {completions.length > 0 && (
            <ul data-formula-completions style={completionList}>
              {completions.map((entry, i) => (
                <li key={entry.name}>
                  <button
                    type="button"
                    data-formula-completion={entry.name}
                    // mousedown (not click) so the choice lands before the
                    // input's blur tears the list down.
                    onMouseDown={(e) => {
                      e.preventDefault();
                      chooseCompletion(entry);
                    }}
                    style={{
                      display: "flex",
                      justifyContent: "space-between",
                      gap: "var(--space-2, 8px)",
                      width: "100%",
                      textAlign: "left",
                      font: "12px var(--font-mono, monospace)",
                      color: "var(--pg-fg)",
                      background:
                        i === activeIdx
                          ? "var(--pg-primary-subtle, var(--pg-subtle, transparent))"
                          : "none",
                      border: "none",
                      padding: "4px 8px",
                      cursor: "pointer",
                    }}
                  >
                    <span>{entry.name}</span>
                    <span style={{ color: "var(--pg-muted-fg)" }}>
                      {arityHint(entry)}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>

        {/* Scroll controls — one row/col per step; the engine re-windows. */}
        <div style={{ display: "flex", gap: "var(--space-1, 4px)" }}>
          <button
            type="button"
            data-grid-scroll="up"
            onClick={() => setFirstRow((r) => Math.max(0, r - 1))}
            style={scrollBtn}
          >
            ↑
          </button>
          <button
            type="button"
            data-grid-scroll="down"
            onClick={() => setFirstRow((r) => r + 1)}
            style={scrollBtn}
          >
            ↓
          </button>
          <button
            type="button"
            data-grid-scroll="left"
            onClick={() => setFirstCol((c) => Math.max(0, c - 1))}
            style={scrollBtn}
          >
            ←
          </button>
          <button
            type="button"
            data-grid-scroll="right"
            onClick={() => setFirstCol((c) => c + 1)}
            style={scrollBtn}
          >
            →
          </button>
          <span
            data-grid-origin
            style={{
              alignSelf: "center",
              font: "11px var(--font-mono, monospace)",
              color: "var(--pg-muted-fg)",
            }}
          >
            R{firstRow} C{firstCol}
          </span>
          {/* K-6 / S-14 — copy the selected range / paste at the anchor.
              Disabled until a cell is selected; the engine owns the values,
              host.clipboard the OS clipboard. */}
          <button
            type="button"
            data-grid-copy
            disabled={!sel}
            onClick={() => {
              void session.copySelection().then((r) => {
                if (!r.ok) host.log.warn(`copy: ${r.message}`);
              });
            }}
            style={{ ...scrollBtn, marginLeft: "var(--space-2, 8px)" }}
          >
            Copy
          </button>
          <button
            type="button"
            data-grid-paste
            disabled={!sel}
            onClick={() => {
              void session.pasteAtSelection().then((r) => {
                if (!r.ok) host.log.warn(`paste: ${r.message}`);
              });
            }}
            style={scrollBtn}
          >
            Paste
          </button>
        </div>

        {/* The grid surface: the engine-windowed SVG + the cell editor. */}
        <div
          data-grid-surface
          style={{
            position: "relative",
            width: wPx,
            height: hPx,
            border: "1px solid var(--pg-border)",
            borderRadius: "var(--radius-sm, 4px)",
            background: "var(--pg-bg)",
            overflow: "hidden",
          }}
        >
          <svg
            data-grid-svg-root
            width={wPx}
            height={hPx}
            viewBox={`0 0 ${scene.viewport.xOffsets.at(-1) ?? 0} ${
              scene.viewport.yOffsets.at(-1) ?? 0
            }`}
            onClick={onSvgClick}
            onDoubleClick={onSvgDoubleClick}
            style={{ display: "block", cursor: "cell" }}
            // The pure helper already produced the inner SVG; we re-host its
            // body so the React handlers attach. dangerouslySetInnerHTML is
            // the body the engine+helper produced (already XML-escaped).
            dangerouslySetInnerHTML={{ __html: innerOf(svg) }}
          />
          {editor && edRect && (
            <input
              ref={editorRef}
              data-grid-editor
              type="text"
              value={editor.value}
              onChange={(e) =>
                setEditor((prev) =>
                  prev ? { ...prev, value: e.target.value } : prev,
                )
              }
              onKeyDown={onEditorKey}
              onBlur={commitEditor}
              style={{
                ...editorInput,
                left: edRect[0] * PX_PER_PT,
                top: edRect[1] * PX_PER_PT,
                width: edRect[2] * PX_PER_PT,
                height: edRect[3] * PX_PER_PT,
              }}
            />
          )}
        </div>

        {/* S-01/S-02 honesty: this is the interim panel grid, not the
            in-frame sheets-mode surface (still SDK-blocked). */}
        <p data-grid-honesty style={note}>
          Interim panel grid — double-click a cell to edit, Enter to commit.
          In-frame sheets mode awaits the SDK rendering surface (S-02).
        </p>
      </div>
    );
  };
}

const scrollBtn: CSSProperties = {
  font: "12px var(--font-sans, sans-serif)",
  color: "var(--pg-fg)",
  background: "var(--pg-bg)",
  border: "1px solid var(--pg-border)",
  borderRadius: "var(--radius-sm, 4px)",
  padding: "2px 8px",
  cursor: "pointer",
};

/** Strip the outer `<svg …>…</svg>` wrapper produced by `gridSceneToSvg`,
 *  yielding just the body so the panel can re-host it inside a React-owned
 *  `<svg>` that carries the click handlers. Pure string surgery on the
 *  helper's own output (always `<svg …>BODY</svg>`). */
function innerOf(svg: string): string {
  const start = svg.indexOf(">");
  const end = svg.lastIndexOf("</svg>");
  if (start < 0 || end < 0 || end <= start) return "";
  return svg.slice(start + 1, end);
}
