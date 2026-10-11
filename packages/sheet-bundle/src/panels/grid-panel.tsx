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
  hitFillHandle,
  matchFunctions,
  type FunctionEntry,
  type GridScene,
  type GridSelection,
} from "../../../sheet-host-model/src";

import type { FindMatch } from "../engine";
import { columnLabel, type SessionNotice, type WorkbookSession } from "../session";
import {
  fillTarget,
  fullyVisible,
  gridKeyAction,
  scrollToShow,
  type AdvanceDir,
} from "../grid-nav";

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

// The viewport the grid windows into (content-space pt). Its WIDTH follows
// the panel: a ResizeObserver measures the panel's content box, so the grid
// fills a narrow dock and grows when the dock is widened. The default is the
// width of the standard 320 px dock, used until the first measurement (and
// where there is no ResizeObserver, e.g. headless tests). Scroll steps move
// the origin one row/col at a time (the engine re-windows from there).
const DEFAULT_VIEWPORT_W_PT = 280;
const MIN_VIEWPORT_W_PT = 120;
const VIEWPORT_H_PT = 320;
/** The surface's left + right border, inside the measured content box. */
const SURFACE_BORDER_PX = 2;

/** The pixel size the SVG renders at (1 pt = 1 px in the interim panel; the
 *  in-frame surface honours the document zoom). */
const PX_PER_PT = 1;

interface EditorState {
  row: number;
  col: number;
  value: string;
}

export function makeGridPanel(
  _host: BundleHost,
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

    // The viewport width, measured from the panel (see DEFAULT_VIEWPORT_W_PT).
    // A callback ref, because the empty state and the grid render different
    // root elements: each one is observed while it is mounted.
    const [viewW, setViewW] = useState(DEFAULT_VIEWPORT_W_PT);
    const observer = useRef<ResizeObserver | null>(null);
    const rootRef = useCallback((el: HTMLDivElement | null) => {
      observer.current?.disconnect();
      observer.current = null;
      if (!el || typeof ResizeObserver === "undefined") return;
      const ro = new ResizeObserver((entries) => {
        const width = entries[entries.length - 1]?.contentRect.width ?? 0;
        if (width <= 0) return; // hidden (a collapsed dock): keep the last size
        const next = Math.max(MIN_VIEWPORT_W_PT, Math.floor(width / PX_PER_PT) - SURFACE_BORDER_PX);
        setViewW((prev) => (prev === next ? prev : next));
      });
      ro.observe(el);
      observer.current = ro;
    }, []);
    useEffect(() => () => observer.current?.disconnect(), []);

    // One status line for answers that used to reach only the log: copy and
    // paste (posted by the session, whichever caller asked), a refused fill
    // and an edit that did not commit.
    const [notice, setNotice] = useState<SessionNotice | null>(() => session.notice());
    useEffect(() => {
      const sub = session.onNotice(setNotice);
      return () => sub.dispose();
    }, []);
    const showError = (text: string) =>
      setNotice((prev) => ({ text, tone: "error", seq: (prev?.seq ?? 0) + 1 }));

    // Find and replace in the sheet (the one home for find): Cmd+F, "Find in
    // sheet…" and "Find & replace…" focus the field (they bump
    // `findRequest`); Enter / Next select + reveal the next match after the
    // active cell (in this grid AND in-frame). Replace, the match toggles
    // and the full hit list sit under "Replace and options".
    const [findText, setFindText] = useState("");
    const [findStatus, setFindStatus] = useState<string | null>(null);
    const [replacement, setReplacement] = useState("");
    const [matchCase, setMatchCase] = useState(false);
    const [entireCell, setEntireCell] = useState(false);
    const [inFormulas, setInFormulas] = useState(false);
    const [hits, setHits] = useState<FindMatch[] | null>(null);
    const [findMsg, setFindMsg] = useState<string | null>(null);
    const findInputRef = useRef<HTMLInputElement | null>(null);
    useEffect(() => {
      if (st.findRequest > 0) findInputRef.current?.focus();
    }, [st.findRequest]);
    const findOpts = { matchCase, entireCell, inFormulas };
    const runFind = (backwards: boolean) => {
      const hit = session.findNext(findText, findOpts, backwards);
      setFindStatus(
        hit ? `${columnLabel(hit.col)}${hit.row + 1}` : findText ? "No match" : null,
      );
    };
    const onFindAll = () => {
      if (!findText) return;
      const found = session.findAll(findText, findOpts, "sheet");
      setHits(found);
      setFindMsg(`${found.length} match${found.length === 1 ? "" : "es"}.`);
    };
    const onReplaceAll = () => {
      if (!findText) return;
      const res = session.replaceAll(findText, replacement, findOpts, "sheet");
      setHits(null); // stale after a replace
      setFindMsg(
        "error" in res
          ? res.error
          : `Replaced ${res.occurrences} occurrence${res.occurrences === 1 ? "" : "s"} in ${res.replacedCells} cell${res.replacedCells === 1 ? "" : "s"}` +
              (res.skipped > 0 ? ` (${res.skipped} skipped).` : "."),
      );
    };

    // ── Formula bar (S-04) — bound to the selected cell. The bar shows the
    // cell's re-enterable INPUT (formula or literal) and commits through the
    // journaled editCell lane. Autocomplete proposes engine-registry function
    // names for the token under the caret (no spreadsheet semantics in TS —
    // the names ARE the engine's, the matching is pure prefix). ──────────────
    const sel = st.gridSelection;
    // Wave 5 — the bar follows the ACTIVE cell (Tab/Enter walk inside a
    // multi-cell selection), not the selection's top-left.
    const active = sel ? session.activeCell() : null;
    const fbRow = active?.row ?? null;
    const fbCol = active?.col ?? null;
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
        ? session.gridScene(firstRow, firstCol, viewW, VIEWPORT_H_PT)
        : null;

    // Keep the editor focused while open.
    useEffect(() => {
      if (editor) editorRef.current?.focus();
    }, [editor?.row, editor?.col]);

    // Wave 5 — pointer gestures: press selects (shift extends) or grabs
    // the fill handle; a drag extends the selection / sizes the fill; the
    // release fills. The grid surface takes keyboard focus on press.
    const surfaceRef = useRef<HTMLDivElement | null>(null);
    const [fillDrag, setFillDrag] = useState<{
      source: GridSelection;
      target: GridSelection | null;
    } | null>(null);
    const selectDrag = useRef(false);
    const lastDragCell = useRef("");
    const pressed = useRef(false);
    const toPt = (e: React.MouseEvent<SVGSVGElement>): [number, number] => {
      const rect = e.currentTarget.getBoundingClientRect();
      // Pixel → content-space pt (the SVG viewBox is pt; it renders at
      // PX_PER_PT, so divide back out).
      return [(e.clientX - rect.left) / PX_PER_PT, (e.clientY - rect.top) / PX_PER_PT];
    };

    const onSvgMouseDown = useCallback(
      (e: React.MouseEvent<SVGSVGElement>) => {
        if (!scene || e.button !== 0) return;
        pressed.current = true;
        surfaceRef.current?.focus();
        const [xPt, yPt] = toPt(e);
        if (sel && hitFillHandle(scene, xPt, yPt)) {
          setFillDrag({ source: sel, target: null });
          return;
        }
        const hit = hitCell(scene, xPt, yPt);
        if (!hit) return;
        if (e.shiftKey && sel) session.extendSelection(hit.row, hit.col);
        else session.selectCell(hit.row, hit.col);
        selectDrag.current = true;
        lastDragCell.current = `${hit.row}:${hit.col}`;
      },
      [scene, sel],
    );

    const onSvgMouseMove = useCallback(
      (e: React.MouseEvent<SVGSVGElement>) => {
        if (!scene || (!fillDrag && !selectDrag.current)) return;
        const [xPt, yPt] = toPt(e);
        const hit = hitCell(scene, xPt, yPt);
        if (!hit) return;
        if (fillDrag) {
          setFillDrag({ ...fillDrag, target: fillTarget(fillDrag.source, hit) });
        } else if (lastDragCell.current !== `${hit.row}:${hit.col}`) {
          lastDragCell.current = `${hit.row}:${hit.col}`;
          session.extendSelection(hit.row, hit.col);
        }
      },
      [scene, fillDrag],
    );

    const endGesture = useCallback(() => {
      selectDrag.current = false;
      if (fillDrag) {
        setFillDrag(null);
        if (fillDrag.target) {
          const r = session.fillSelectionTo(fillDrag.target, true);
          if (!r.ok) showError(`Can't fill: ${r.message}.`);
        }
      }
    }, [fillDrag]);

    // A click with no press before it (keyboard-less harnesses) still selects.
    const onSvgClick = useCallback(
      (e: React.MouseEvent<SVGSVGElement>) => {
        if (pressed.current) {
          pressed.current = false;
          return;
        }
        if (!scene) return;
        const [xPt, yPt] = toPt(e);
        const hit = hitCell(scene, xPt, yPt);
        if (!hit) return;
        session.setGridSelection(hit.row, hit.col, 1, 1);
      },
      [scene],
    );

    // Keep the active cell in the panel window.
    useEffect(() => {
      if (!active || !scene) return;
      const next = scrollToShow(
        { firstRow, firstCol },
        fullyVisible(scene, viewW, VIEWPORT_H_PT),
        active,
      );
      if (next.firstRow !== firstRow) setFirstRow(next.firstRow);
      if (next.firstCol !== firstCol) setFirstCol(next.firstCol);
    }, [active?.row, active?.col]);

    // Keys on the focused grid surface: the shared key map. Typing / F2
    // open the panel's own cell editor; everything else is the session's
    // (moves, Tab/Enter, Delete, Cmd+C/V/D/R/A/Z). A key the grid takes
    // stops here, so the host's shortcuts (Cmd+Z = document undo) do not
    // also fire.
    const onSurfaceKey = useCallback(
      (e: React.KeyboardEvent<HTMLDivElement>) => {
        if (editor || e.target !== e.currentTarget) return;
        const action = gridKeyAction(e, null);
        if (action.kind === "type" && active) {
          e.preventDefault();
          e.stopPropagation();
          setEditor({ row: active.row, col: active.col, value: action.ch });
          return;
        }
        if (action.kind === "edit" && active) {
          e.preventDefault();
          e.stopPropagation();
          setEditor({
            row: active.row,
            col: active.col,
            value: session.cellInputAt(active.row, active.col),
          });
          return;
        }
        if (session.handleGridKey(e)) {
          e.preventDefault();
          e.stopPropagation();
        }
      },
      [editor, active],
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
        // The re-enterable input (a formula shows as its formula).
        setEditor({ row: hit.row, col: hit.col, value: session.cellInputAt(hit.row, hit.col) });
      },
      [scene, st.activeSheet],
    );

    const commitEditor = useCallback((then?: AdvanceDir) => {
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
      if (!ok) showError("The cell edit was not saved.");
      setEditor(null);
      if (then) {
        session.handleGridKey({
          key: then === "right" || then === "left" ? "Tab" : "Enter",
          shiftKey: then === "left" || then === "up",
        });
        surfaceRef.current?.focus();
      }
    }, [editor, st.activeSheet]);

    const onEditorKey = useCallback(
      (e: React.KeyboardEvent<HTMLInputElement>) => {
        if (e.key === "Enter") {
          e.preventDefault();
          commitEditor(e.shiftKey ? "up" : "down");
        } else if (e.key === "Tab") {
          e.preventDefault();
          commitEditor(e.shiftKey ? "left" : "right");
        } else if (e.key === "Escape") {
          e.preventDefault();
          setEditor(null);
          surfaceRef.current?.focus();
        }
      },
      [commitEditor],
    );

    // No engine / sheet yet — the honest empty state (import in Workbook).
    if (st.activeSheet === null || !scene) {
      return (
        <div
          ref={rootRef}
          data-sheet-panel="grid"
          style={{ padding: "var(--space-3, 12px)" }}
        >
          <div style={kicker}>Grid</div>
          <p data-grid-empty style={{ ...note, marginTop: 0 }}>
            {st.bootError
              ? `The spreadsheet engine could not start: ${st.bootError}.`
              : "Open or create a workbook to edit it here."}
          </p>
          {!st.bootError && (
            <button
              type="button"
              data-grid-new
              onClick={() => void session.newWorkbook()}
              style={{ ...scrollBtn, marginTop: "var(--space-2, 8px)" }}
            >
              New blank workbook
            </button>
          )}
        </div>
      );
    }

    const svg = svgOf(fillDrag?.target ? { ...scene, fillPreview: fillDrag.target } : scene);
    const wPx = (scene.viewport.xOffsets.at(-1) ?? 0) * PX_PER_PT;
    const hPx = (scene.viewport.yOffsets.at(-1) ?? 0) * PX_PER_PT;

    // The editor overlay rect (content-space pt → px), or null.
    const edRect = editor
      ? cellEditorRect(scene, editor.row, editor.col)
      : null;

    return (
      <div
        ref={rootRef}
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

        {/* Find and replace in the sheet (Wave 5; the one home for find). */}
        <div data-grid-find style={{ display: "flex", flexWrap: "wrap", gap: "var(--space-1, 4px)" }}>
          <input
            ref={findInputRef}
            data-find-input
            data-sheet-find-needle
            type="text"
            value={findText}
            placeholder="Find in sheet"
            aria-label="Find in sheet"
            onChange={(e) => {
              setFindText(e.target.value);
              setFindStatus(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                runFind(e.shiftKey);
              } else if (e.key === "Escape") {
                e.preventDefault();
                surfaceRef.current?.focus();
              }
            }}
            style={{ ...formulaInput, flex: "1 1 120px", minWidth: 0 }}
          />
          <button
            type="button"
            data-find-prev
            disabled={!findText}
            title={findText ? "Previous match (Shift+Enter)" : "Type something to find first"}
            onClick={() => runFind(true)}
            style={scrollBtn}
          >
            Prev
          </button>
          <button
            type="button"
            data-find-next
            disabled={!findText}
            title={findText ? "Next match (Enter)" : "Type something to find first"}
            onClick={() => runFind(false)}
            style={scrollBtn}
          >
            Next
          </button>
          <button
            type="button"
            data-sheet-find
            disabled={!findText}
            title={findText ? "List every match in this sheet" : "Type something to find first"}
            onClick={onFindAll}
            style={scrollBtn}
          >
            Find all
          </button>
          {findStatus && (
            <span
              data-find-status
              style={{
                alignSelf: "center",
                font: "11px var(--font-sans, sans-serif)",
                color: "var(--pg-muted-fg)",
              }}
            >
              {findStatus}
            </span>
          )}
        </div>
        <details data-sheet-replace-section style={{ font: "12px var(--font-sans, sans-serif)" }}>
          <summary style={{ cursor: "pointer", color: "var(--pg-muted-fg)" }}>
            Replace and options
          </summary>
          <div style={{ display: "flex", flexWrap: "wrap", gap: "var(--space-1, 4px)", marginTop: "var(--space-1, 4px)" }}>
            <input
              data-sheet-find-replacement
              type="text"
              value={replacement}
              onChange={(e) => setReplacement(e.target.value)}
              placeholder="Replace with"
              aria-label="Replace with"
              style={{ ...formulaInput, flex: "1 1 120px", minWidth: 0 }}
            />
            <button
              type="button"
              data-sheet-replace-all
              disabled={!findText}
              title={findText ? "Replace every match in this sheet" : "Type something to find first"}
              onClick={onReplaceAll}
              style={scrollBtn}
            >
              Replace all
            </button>
          </div>
          <div style={{ display: "flex", flexWrap: "wrap", gap: "var(--space-2, 8px)", marginTop: "var(--space-1, 4px)" }}>
            {(
              [
                ["data-sheet-find-case", "Match case", matchCase, setMatchCase],
                ["data-sheet-find-entire", "Entire cell", entireCell, setEntireCell],
                ["data-sheet-find-formulas", "In formulas", inFormulas, setInFormulas],
              ] as const
            ).map(([attr, label, checked, set]) => (
              <label key={attr} style={{ display: "flex", alignItems: "center", gap: 4, color: "var(--pg-fg)" }}>
                <input
                  {...{ [attr]: true }}
                  type="checkbox"
                  checked={checked}
                  onChange={(e) => set(e.target.checked)}
                />
                {label}
              </label>
            ))}
          </div>
        </details>
        {findMsg && (
          <p data-sheet-find-msg role="status" style={{ ...note, marginTop: 0, color: "var(--pg-fg)", fontSize: 12 }}>
            {findMsg}
          </p>
        )}
        {hits && hits.length > 0 && (
          <ul
            data-sheet-find-hits
            style={{
              listStyle: "none",
              margin: 0,
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
                    display: "block",
                    width: "100%",
                    textAlign: "left",
                    font: "12px var(--font-sans, sans-serif)",
                    color: "var(--pg-fg)",
                    background: "none",
                    border: "none",
                    padding: "3px 6px",
                    cursor: "pointer",
                  }}
                >
                  <span style={{ fontFamily: "var(--font-mono, monospace)", fontSize: 11 }}>
                    {columnLabel(h.col)}
                    {h.row + 1}
                  </span>{" "}
                  {h.excerpt}
                </button>
              </li>
            ))}
          </ul>
        )}

        {/* Scroll controls — one row/col per step; the engine re-windows. */}
        <div style={{ display: "flex", flexWrap: "wrap", gap: "var(--space-1, 4px)" }}>
          <button
            type="button"
            data-grid-scroll="up"
            aria-label="Scroll up one row"
            title="Scroll up one row"
            onClick={() => setFirstRow((r) => Math.max(0, r - 1))}
            style={scrollBtn}
          >
            ↑
          </button>
          <button
            type="button"
            data-grid-scroll="down"
            aria-label="Scroll down one row"
            title="Scroll down one row"
            onClick={() => setFirstRow((r) => r + 1)}
            style={scrollBtn}
          >
            ↓
          </button>
          <button
            type="button"
            data-grid-scroll="left"
            aria-label="Scroll left one column"
            title="Scroll left one column"
            onClick={() => setFirstCol((c) => Math.max(0, c - 1))}
            style={scrollBtn}
          >
            ←
          </button>
          <button
            type="button"
            data-grid-scroll="right"
            aria-label="Scroll right one column"
            title="Scroll right one column"
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
            title={sel ? "Copy the selected cells" : "Select cells to copy first"}
            onClick={() => void session.copySelection()}
            style={{ ...scrollBtn, marginLeft: "var(--space-2, 8px)" }}
          >
            Copy
          </button>
          <button
            type="button"
            data-grid-paste
            disabled={!sel}
            title={sel ? "Paste at the selected cell" : "Select a cell to paste at first"}
            onClick={() => void session.pasteAtSelection()}
            style={scrollBtn}
          >
            Paste
          </button>
        </div>

        {notice && (
          <p
            key={notice.seq}
            data-grid-notice={notice.tone}
            role={notice.tone === "error" ? "alert" : "status"}
            style={{
              margin: 0,
              font: "12px var(--font-sans, sans-serif)",
              color: notice.tone === "error" ? "var(--status-error)" : "var(--pg-muted-fg)",
            }}
          >
            {notice.text}
          </p>
        )}

        {/* The grid surface: the engine-windowed SVG + the cell editor. */}
        <div
          data-grid-surface
          ref={surfaceRef}
          tabIndex={0}
          onKeyDown={onSurfaceKey}
          style={{
            position: "relative",
            width: wPx,
            maxWidth: "100%",
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
            onMouseDown={onSvgMouseDown}
            onMouseMove={onSvgMouseMove}
            onMouseUp={endGesture}
            onMouseLeave={endGesture}
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
              onBlur={() => commitEditor()}
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
        <p data-grid-honesty style={{ ...note, marginTop: 0 }}>
          Click a cell to select it and type to edit; the same grid edits
          inside a placed sheet frame.
        </p>
        <details style={{ ...note, marginTop: 0 }}>
          <summary style={{ cursor: "pointer" }}>About</summary>
          Drag or Shift+click to select a range. F2 edits the cell, Enter or
          Tab commits and moves, Delete clears. Drag the corner handle to
          fill. Double-click a placed sheet frame on the page to edit it
          there.
        </details>
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

const GRID_SVG_OPTIONS = {
  ...DEFAULT_GRID_SVG_OPTIONS,
  gridColor: "var(--pg-border)",
  textColor: "var(--pg-fg)",
  selectionColor: "var(--pg-primary)",
};

/** The SVG string of a scene, built once per scene. The session memoises
 *  the scene until the next change signal, so a re-render that changed
 *  nothing in the grid (a formula-bar keystroke, a completion move)
 *  reuses the string instead of re-serialising every cell. */
const svgCache = new WeakMap<GridScene, string>();
function svgOf(scene: GridScene): string {
  let svg = svgCache.get(scene);
  if (svg === undefined) {
    svg = gridSceneToSvg(scene, GRID_SVG_OPTIONS);
    svgCache.set(scene, svg);
  }
  return svg;
}

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
