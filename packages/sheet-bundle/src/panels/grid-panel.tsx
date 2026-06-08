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
  cellEditorRect,
  gridSceneToSvg,
  hitCell,
  type GridScene,
} from "@paged-media/sheet-host-model";

import type { WorkbookSession } from "../session";

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
