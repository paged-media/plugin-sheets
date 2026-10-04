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

// Wave 6 — the workbook panel's FORMAT & LAYOUT section: number formats,
// font, fill, borders, alignment / wrap, merge, column width / row height,
// the freeze toggle and the name manager. Thin glue: every control forwards
// one session verb, every rule (validation, dedup, the xlsx records, what a
// name resolves to) is the engine's, and each verb's message shows verbatim.

import { useState, type CSSProperties, type ReactElement } from "react";

import type { CellStylePatch, EdgeStyle } from "../engine";
import type { BorderKind, SessionResult, WorkbookSession } from "../session";

/** Number-format presets — format CODES offered as a starting point (the
 *  engine renders them; "Custom" takes any code). */
export const NUMBER_FORMAT_PRESETS: { label: string; code: string }[] = [
  { label: "General", code: "General" },
  { label: "Integer (0)", code: "0" },
  { label: "Decimal (0.00)", code: "0.00" },
  { label: "Thousands (#,##0)", code: "#,##0" },
  { label: "Thousands, 2 dp", code: "#,##0.00" },
  { label: "Percent (0%)", code: "0%" },
  { label: "Percent, 2 dp", code: "0.00%" },
  { label: "Scientific", code: "0.00E+00" },
  { label: "Date (yyyy-mm-dd)", code: "yyyy-mm-dd" },
  { label: "Date (d-mmm-yy)", code: "d-mmm-yy" },
  { label: "Time (h:mm)", code: "h:mm" },
  { label: "Euro", code: "#,##0.00 [$€-407]" },
  { label: "Text (@)", code: "@" },
];

const BORDER_STYLES = ["thin", "medium", "thick", "dashed", "dotted", "double", "hair"];
const H_ALIGNS = ["general", "left", "center", "right", "justify"];
const V_ALIGNS = ["bottom", "center", "top"];

export interface FormatSectionStyles {
  kicker: CSSProperties;
  body: CSSProperties;
  input: CSSProperties;
  button: CSSProperties;
  row: CSSProperties;
}

/** The format & layout controls (rendered inside the workbook panel). */
export function FormatSection(props: {
  session: WorkbookSession;
  styles: FormatSectionStyles;
}): ReactElement {
  const { session, styles: st } = props;
  const [msg, setMsg] = useState<string | null>(null);
  const report = (r: SessionResult, done: string) => setMsg(r.ok ? done : r.message);

  const target = session.formatTarget();
  const cur = session.styleAtTarget();
  const layout = session.layout();

  const [preset, setPreset] = useState("0.00");
  const [custom, setCustom] = useState("");
  const [fontName, setFontName] = useState("");
  const [fontSize, setFontSize] = useState("");
  const [fontColor, setFontColor] = useState("#000000");
  const [fill, setFill] = useState("#FFFF00");
  const [borderStyle, setBorderStyle] = useState("thin");
  const [borderColor, setBorderColor] = useState("#000000");
  const [width, setWidth] = useState("");
  const [height, setHeight] = useState("");
  const [nameNew, setNameNew] = useState("");
  const [nameRefers, setNameRefers] = useState("");
  const [nameLocal, setNameLocal] = useState(false);

  const apply = (patch: CellStylePatch, done: string) => report(session.setStyle(patch), done);
  const edge: EdgeStyle = { style: borderStyle, color: borderColor };
  const borders = (kind: BorderKind, done: string) =>
    report(session.setBorders(kind, edge), done);
  const frozen = !!layout && (layout.freezeRows > 0 || layout.freezeCols > 0);
  const names = session.names();

  const summary = cur
    ? [
        cur.numFmt,
        [cur.fontName, cur.fontSize ? `${cur.fontSize} pt` : null].filter(Boolean).join(" "),
        cur.bold ? "bold" : null,
        cur.italic ? "italic" : null,
        cur.underline ? "underline" : null,
        cur.fill ? `fill ${cur.fill}` : null,
        cur.hAlign !== "general" ? cur.hAlign : null,
        cur.wrap ? "wrap" : null,
      ]
        .filter((p) => p)
        .join(" · ")
    : null;

  return (
    <div data-sheet-format-section>
      <div style={st.kicker}>Format{target ? ` — ${target.range}` : ""}</div>
      {summary && (
        <div data-sheet-fmt-current style={{ ...st.body, opacity: 0.8 }}>
          {summary}
        </div>
      )}

      {/* Number format: a preset or a custom code. */}
      <div style={st.row}>
        <select
          data-sheet-fmt-numfmt
          value={preset}
          onChange={(e) => setPreset(e.target.value)}
          style={st.input}
        >
          {NUMBER_FORMAT_PRESETS.map((p) => (
            <option key={p.code} value={p.code}>
              {p.label}
            </option>
          ))}
        </select>
        <input
          data-sheet-fmt-numfmt-custom
          type="text"
          value={custom}
          placeholder="custom code"
          onChange={(e) => setCustom(e.target.value)}
          style={{ ...st.input, width: 110 }}
        />
        <button
          type="button"
          data-sheet-fmt-numfmt-apply
          style={st.button}
          onClick={() => apply({ numFmt: custom.trim() || preset }, "Number format applied.")}
        >
          Number format
        </button>
      </div>

      {/* Font. */}
      <div style={st.row}>
        <button type="button" data-sheet-fmt-bold style={{ ...st.button, fontWeight: 700 }}
          aria-pressed={!!cur?.bold}
          onClick={() => apply({ bold: !cur?.bold }, "Bold toggled.")}>
          B
        </button>
        <button type="button" data-sheet-fmt-italic style={{ ...st.button, fontStyle: "italic" }}
          aria-pressed={!!cur?.italic}
          onClick={() => apply({ italic: !cur?.italic }, "Italic toggled.")}>
          I
        </button>
        <button type="button" data-sheet-fmt-underline style={{ ...st.button, textDecoration: "underline" }}
          aria-pressed={!!cur?.underline}
          onClick={() => apply({ underline: !cur?.underline }, "Underline toggled.")}>
          U
        </button>
        <input data-sheet-fmt-font-name type="text" value={fontName} placeholder={cur?.fontName ?? "Font"}
          onChange={(e) => setFontName(e.target.value)} style={{ ...st.input, width: 90 }} />
        <input data-sheet-fmt-font-size type="number" min={1} max={409} value={fontSize}
          placeholder={cur?.fontSize ? String(cur.fontSize) : "pt"}
          onChange={(e) => setFontSize(e.target.value)} style={{ ...st.input, width: 48 }} />
        <input data-sheet-fmt-font-color type="color" value={fontColor}
          onChange={(e) => setFontColor(e.target.value)} title="Text colour" />
        <button
          type="button"
          data-sheet-fmt-font-apply
          style={st.button}
          onClick={() =>
            apply(
              {
                ...(fontName.trim() ? { fontName: fontName.trim() } : {}),
                ...(fontSize.trim() ? { fontSize: Number(fontSize) } : {}),
                fontColor,
              },
              "Font applied.",
            )
          }
        >
          Font
        </button>
      </div>

      {/* Fill. */}
      <div style={st.row}>
        <input data-sheet-fmt-fill type="color" value={fill} onChange={(e) => setFill(e.target.value)}
          title="Fill colour" />
        <button type="button" data-sheet-fmt-fill-apply style={st.button}
          onClick={() => apply({ fill }, "Fill applied.")}>
          Fill
        </button>
        <button type="button" data-sheet-fmt-fill-clear style={st.button}
          onClick={() => apply({ fill: "" }, "Fill cleared.")}>
          No fill
        </button>
      </div>

      {/* Borders. */}
      <div style={st.row}>
        <select data-sheet-fmt-border-style value={borderStyle}
          onChange={(e) => setBorderStyle(e.target.value)} style={st.input}>
          {BORDER_STYLES.map((b) => (
            <option key={b} value={b}>
              {b}
            </option>
          ))}
        </select>
        <input data-sheet-fmt-border-color type="color" value={borderColor}
          onChange={(e) => setBorderColor(e.target.value)} title="Border colour" />
        {(["all", "outline", "top", "bottom", "left", "right", "none"] as const).map((k) => (
          <button key={k} type="button" data-sheet-fmt-border={k} style={st.button}
            onClick={() => borders(k, k === "none" ? "Borders removed." : `Borders (${k}) applied.`)}>
            {k === "all" ? "All" : k === "none" ? "None" : k[0].toUpperCase() + k.slice(1)}
          </button>
        ))}
      </div>

      {/* Alignment + wrap. */}
      <div style={st.row}>
        <select data-sheet-fmt-halign value={cur?.hAlign ?? "general"}
          onChange={(e) => apply({ hAlign: e.target.value }, "Alignment applied.")} style={st.input}>
          {H_ALIGNS.map((a) => (
            <option key={a} value={a}>
              {a}
            </option>
          ))}
        </select>
        <select data-sheet-fmt-valign value={cur?.vAlign ?? "bottom"}
          onChange={(e) => apply({ vAlign: e.target.value }, "Vertical alignment applied.")}
          style={st.input}>
          {V_ALIGNS.map((a) => (
            <option key={a} value={a}>
              {a}
            </option>
          ))}
        </select>
        <label style={{ ...st.body, display: "flex", alignItems: "center", gap: 4 }}>
          <input data-sheet-fmt-wrap type="checkbox" checked={!!cur?.wrap}
            onChange={(e) => apply({ wrap: e.target.checked }, e.target.checked ? "Wrap on." : "Wrap off.")} />
          Wrap
        </label>
      </div>

      <div style={st.kicker}>Layout</div>
      <div style={st.row}>
        <button type="button" data-sheet-merge style={st.button}
          onClick={() => report(session.mergeTarget(), "Merged.")}>
          Merge
        </button>
        <button type="button" data-sheet-unmerge style={st.button}
          onClick={() => report(session.unmergeTarget(), "Unmerged.")}>
          Unmerge
        </button>
        <button type="button" data-sheet-freeze style={st.button} aria-pressed={frozen}
          onClick={() => report(session.toggleFreeze(), frozen ? "Panes unfrozen." : "Panes frozen.")}>
          {frozen ? `Unfreeze (${layout!.freezeRows}×${layout!.freezeCols})` : "Freeze panes"}
        </button>
      </div>
      <div style={st.row}>
        <input data-sheet-col-width type="number" min={0} max={255} value={width} placeholder="width (ch)"
          onChange={(e) => setWidth(e.target.value)} style={{ ...st.input, width: 80 }} />
        <button type="button" data-sheet-col-width-set style={st.button} disabled={!width.trim()}
          onClick={() => report(session.setColumnWidth(Number(width)), "Column width set.")}>
          Set width
        </button>
        <button type="button" data-sheet-col-width-reset style={st.button}
          onClick={() => report(session.setColumnWidth(null), "Column width reset.")}>
          Default
        </button>
      </div>
      <div style={st.row}>
        <input data-sheet-row-height type="number" min={0} max={409} value={height} placeholder="height (pt)"
          onChange={(e) => setHeight(e.target.value)} style={{ ...st.input, width: 80 }} />
        <button type="button" data-sheet-row-height-set style={st.button} disabled={!height.trim()}
          onClick={() => report(session.setRowHeight(Number(height)), "Row height set.")}>
          Set height
        </button>
        <button type="button" data-sheet-row-height-reset style={st.button}
          onClick={() => report(session.setRowHeight(null), "Row height reset.")}>
          Default
        </button>
      </div>

      <div style={st.kicker}>Names</div>
      <div style={st.row}>
        <input data-sheet-name-new type="text" value={nameNew} placeholder="Name"
          onChange={(e) => setNameNew(e.target.value)} style={{ ...st.input, width: 90 }} />
        <input data-sheet-name-refers type="text" value={nameRefers}
          placeholder={target?.range ?? "A1:B5"}
          onChange={(e) => setNameRefers(e.target.value)} style={{ ...st.input, width: 90 }} />
        <label style={{ ...st.body, display: "flex", alignItems: "center", gap: 4 }}>
          <input data-sheet-name-local type="checkbox" checked={nameLocal}
            onChange={(e) => setNameLocal(e.target.checked)} />
          This sheet only
        </label>
        <button type="button" data-sheet-name-define style={st.button} disabled={!nameNew.trim()}
          onClick={() =>
            report(
              session.defineName(nameNew, nameRefers.trim() || undefined, nameLocal),
              `Name ${nameNew.trim()} defined.`,
            )
          }>
          Define
        </button>
      </div>
      {names.length > 0 && (
        <ul data-sheet-names style={{ ...st.body, margin: "4px 0 0", paddingLeft: 16 }}>
          {names.map((n) => (
            <li key={`${n.name}:${n.scope ?? "wb"}`}>
              <span style={{ fontFamily: "var(--font-mono, monospace)" }}>{n.name}</span>
              {n.scope != null ? " (sheet)" : ""} = {n.refersTo}{" "}
              <button type="button" data-sheet-name-place={n.name} style={st.button}
                onClick={() =>
                  void session.placeName(n.name).then((id) =>
                    setMsg(id ? `Placed ${n.name}.` : `${n.name} does not name a range.`),
                  )
                }>
                Place
              </button>{" "}
              <button type="button" data-sheet-name-delete={n.name} style={st.button}
                onClick={() => report(session.deleteName(n.name, n.scope ?? null), `Name ${n.name} deleted.`)}>
                Delete
              </button>
            </li>
          ))}
        </ul>
      )}

      {msg && (
        <div data-sheet-format-msg role="status" style={{ ...st.body, marginTop: 4 }}>
          {msg}
        </div>
      )}
    </div>
  );
}
