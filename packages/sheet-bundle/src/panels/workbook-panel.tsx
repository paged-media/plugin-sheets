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

import type { WorkbookSession } from "../session";

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

    const onLower = useCallback(() => {
      void session.lowerSelection();
    }, []);

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
        {/* S-11: the panel owns the file input (no host file picker). */}
        <input
          ref={fileRef}
          data-sheet-file
          type="file"
          accept=".xlsx"
          onChange={onFile}
          style={{ ...body, marginBottom: "var(--space-2, 8px)" }}
        />

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
          </>
        )}

        {/* S-08: workbook bytes live in memory only — be honest. */}
        <p
          data-sheet-honesty
          style={{
            margin: "var(--space-3, 12px) 0 0",
            font: "10px/1.5 var(--font-sans, sans-serif)",
            color: "var(--pg-muted-fg)",
          }}
        >
          Workbook lives in memory — re-import after reload. The frame
          binding persists with the document.
        </p>
      </div>
    );
  };
}
