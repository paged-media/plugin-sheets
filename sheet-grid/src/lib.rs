/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 *
 * This file is part of paged (https://paged.media) and is additionally
 * available under the Paged Media Enterprise License (PMEL). Full
 * copyright and license information is available in LICENSE.md which is
 * distributed with this source code.
 *
 *  @copyright  Copyright (c) And The Next GmbH
 *  @license    MPL-2.0 OR Paged Media Enterprise License (PMEL)
 */

//! # sheet-grid — the sheets-mode vector grid scene (spec §8.1, T1)
//!
//! Pure `&SheetModel -> GridScene`: a windowed, virtualized view of one
//! worksheet for the SDK rendering surface (S-02). Where `sheet-lower`
//! compiles a FIXED range into page content, `sheet-grid` produces a
//! SCROLLABLE viewport — only the cells visible from a `(first_row,
//! first_col)` scroll origin, bounded by `(max_width_pt, max_height_pt)`,
//! are materialized. This is what makes a 1M-row sheet cheap: windowing
//! touches O(visible cells), never O(sheet).
//!
//! ## What Phase A lands (this file)
//!
//! - the FROZEN [`GridScene`] / [`GridViewport`] / [`GridCell`] /
//!   [`GridSelection`] / [`GridOptions`] IR (serde camelCase);
//! - [`grid_scene`] — the **viewport windowing**: cumulative pt offsets from
//!   `col_widths`/`row_heights`, cell materialization ONLY for visible
//!   populated cells, formatted text via `sheet-format` (the same number-
//!   format render the page lowering uses, so a cell reads identically on
//!   both surfaces).
//!
//! Selection is always `None` in Phase A — the grid panel supplies it
//! later (the editing/selection contract is a later track).
//!
//! ## IR reuse (documented spec §4 deviation)
//!
//! The scene's `styles`/`gridlines` reuse `sheet_lower::LoweredStyle` and
//! `sheet_lower::RuleSet` so the grid and the page lowering share ONE
//! style/rule wire shape (see the crate's Cargo.toml note). `sheet-lower`
//! is pure IR, so this does not cross the §4 SDK-isolation rule.

use sheet_core::{Align, Cell, CellValue, SheetId, SheetModel};
use sheet_format::{FormatCache, FormatCtx};
use sheet_lower::{LoweredStyle, RuleSet};

// Re-use the page-lowering geometry rulings so a column is the same width on
// both surfaces (spec §8.2 — character-unit columns, point rows).
use sheet_lower::{CHAR_TO_PT, DEFAULT_COL_CHARS, DEFAULT_ROW_PT};

// ---- The grid scene IR (FROZEN wire shape; serde camelCase). ----

/// The complete vector scene for one grid viewport (spec §8.1). Pure data
/// the SDK rendering surface draws: the windowed viewport geometry, the
/// visible populated cells, the style table, the gridlines, and the
/// (optional) selection rectangle.
#[derive(serde::Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GridScene {
    pub viewport: GridViewport,
    pub cells: Vec<GridCell>,
    pub styles: Vec<LoweredStyle>,
    pub gridlines: RuleSet,
    pub selection: Option<GridSelection>,
    /// The frozen row/column split rendered in this viewport (spec §8.1). The
    /// renderer holds the first `rows`/`cols` of the sheet fixed while the rest
    /// scrolls under them — the classic frozen-header view. `None` (the common
    /// case) when no pane is frozen. When the scroll origin is past the frozen
    /// band, the split still names how many leading sheet rows/cols are pinned
    /// (the renderer composites the frozen band over the scrolled body). */
    pub freeze: Option<GridFreeze>,
}

/// The frozen-pane split shown in a grid viewport (spec §8.1). `rows`/`cols`
/// are the number of leading SHEET rows/columns held fixed; `frozen_width_pt`
/// / `frozen_height_pt` are the pt extents of those frozen bands (the split
/// line sits there), so a renderer can draw the pinned band + the split rule
/// without re-summing widths. The mirror of `sheet_xlsx::FreezePanes` enriched
/// with the band geometry the grid already computed.
#[derive(serde::Serialize, Debug, PartialEq, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub struct GridFreeze {
    /// Leading sheet rows held fixed at the top.
    pub rows: u32,
    /// Leading sheet columns held fixed at the left.
    pub cols: u32,
    /// pt width of the frozen column band (sum of the frozen columns' widths).
    pub frozen_width_pt: f64,
    /// pt height of the frozen row band (sum of the frozen rows' heights).
    pub frozen_height_pt: f64,
}

/// The windowed viewport: the first visible `(row, col)`, how many of each
/// fit, and the cumulative pt offsets along each axis. `x_offsets` /
/// `y_offsets` carry `cols + 1` / `rows + 1` entries — the leading edge of
/// every visible track PLUS a trailing edge (so a renderer has every
/// boundary). Offsets are viewport-local (the first visible track starts at
/// 0).
#[derive(serde::Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GridViewport {
    pub first_row: u32,
    pub first_col: u32,
    pub rows: u32,
    pub cols: u32,
    /// Cumulative pt boundaries along x (len == `cols + 1`, incl. trailing).
    pub x_offsets: Vec<f64>,
    /// Cumulative pt boundaries along y (len == `rows + 1`, incl. trailing).
    pub y_offsets: Vec<f64>,
}

/// One materialized visible cell: its absolute `(row, col)`, the FORMATTED
/// text (the number-format render IS the text, spec §8.3), the resolved
/// alignment, and its style key. Only POPULATED cells inside the viewport
/// are emitted (empty cells are absent — the renderer draws blanks from the
/// viewport geometry).
#[derive(serde::Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GridCell {
    pub row: u32,
    pub col: u32,
    pub text: String,
    /// Lowercase serde like the page lowering (`"general"`/`"left"`/… ).
    #[serde(serialize_with = "serialize_align")]
    pub align: Align,
    pub style_key: u32,
}

/// A selection rectangle, anchored at `(anchor_row, anchor_col)` and
/// spanning `rows`×`cols`. `None` on the scene in Phase A (the panel
/// supplies selection later).
#[derive(serde::Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GridSelection {
    pub anchor_row: u32,
    pub anchor_col: u32,
    pub rows: u32,
    pub cols: u32,
}

/// Per-scene options. `include_gridlines` toggles the [`RuleSet`] at every
/// visible track boundary (like the page lowering's grid rules); `freeze_rows`
/// / `freeze_cols` are the frozen-pane split (spec §8.1) to render — the number
/// of leading SHEET rows/columns held fixed (0 = none). The grid resolves these
/// to the [`GridFreeze`] band geometry on the scene.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GridOptions {
    pub include_gridlines: bool,
    /// Leading frozen rows (spec §8.1); 0 = no frozen row band.
    pub freeze_rows: u32,
    /// Leading frozen columns (spec §8.1); 0 = no frozen column band.
    pub freeze_cols: u32,
}

impl Default for GridOptions {
    fn default() -> Self {
        GridOptions {
            include_gridlines: true,
            freeze_rows: 0,
            freeze_cols: 0,
        }
    }
}

/// serde hook: render `Align` as the agreed lowercase variant name (the
/// same wire token the page lowering uses).
fn serialize_align<S>(align: &Align, s: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    s.serialize_str(align_str(*align))
}

fn align_str(align: Align) -> &'static str {
    match align {
        Align::General => "general",
        Align::Left => "left",
        Align::Center => "center",
        Align::Right => "right",
    }
}

/// Generate the [`GridScene`] for a worksheet viewport (spec §8.1).
///
/// Windowing: starting from the `(first_row, first_col)` scroll origin,
/// columns are accumulated (their pt widths from `col_widths`, the xlsx
/// default otherwise) until the next column would exceed `max_width_pt`;
/// rows likewise against `max_height_pt`. At least one row/col is always
/// included so the viewport is never degenerate. Only POPULATED cells
/// inside the resulting window are materialized — so the scene cost is
/// O(visible populated cells), not O(sheet). An unknown `sheet` yields an
/// empty-but-shaped scene (geometry from the bounds, no cells).
pub fn grid_scene(
    model: &SheetModel,
    sheet: SheetId,
    first_row: u32,
    first_col: u32,
    max_width_pt: f64,
    max_height_pt: f64,
    opts: &GridOptions,
) -> GridScene {
    let ws = model.sheet(sheet);

    // ---- Window the columns: accumulate widths until max_width_pt. ----
    let mut x_offsets: Vec<f64> = vec![0.0];
    let mut last_col = first_col;
    let mut cols = 0u32;
    let mut x = 0.0_f64;
    loop {
        let width = col_width_pt(ws, last_col);
        // Always include the first column; otherwise stop before overflow.
        if cols > 0 && x + width > max_width_pt {
            break;
        }
        x += width;
        x_offsets.push(x);
        cols += 1;
        // Guard the grid's right edge (XFD) so we never run past the model.
        if last_col == sheet_core::MAX_COL {
            break;
        }
        last_col += 1;
    }

    // ---- Window the rows: accumulate heights until max_height_pt. ----
    let mut y_offsets: Vec<f64> = vec![0.0];
    let mut last_row = first_row;
    let mut rows = 0u32;
    let mut y = 0.0_f64;
    loop {
        let height = row_height_pt(ws, last_row);
        if rows > 0 && y + height > max_height_pt {
            break;
        }
        y += height;
        y_offsets.push(y);
        rows += 1;
        if last_row == sheet_core::MAX_ROW {
            break;
        }
        last_row += 1;
    }

    let total_width = x;
    let total_height = y;
    // The last absolute row/col that is IN the window (inclusive).
    let row_end = first_row + rows - 1;
    let col_end = first_col + cols - 1;

    // ---- Materialize ONLY visible populated cells (virtualization). ----
    let ctx = FormatCtx::new(model.calc.date_system, model.calc.locale);
    let mut cache = FormatCache::default();
    let mut cells: Vec<GridCell> = Vec::new();
    if let Some(ws) = ws {
        // Iterate the sparse cell map's window via a range query so we touch
        // O(populated cells in window), never the full row span. The map is
        // keyed (row, col) row-major, so a range over the row band yields
        // the window's rows in order; we filter columns per row.
        for (&(r, c), cell) in ws.cells.range((first_row, first_col)..=(row_end, col_end)) {
            if c < first_col || c > col_end {
                continue; // outside the column band for this row
            }
            let (text, align) = lower_cell_text(model, cell, &mut cache, &ctx);
            // An empty/blank cell carries no text — skip it (the renderer
            // draws the blank from the viewport geometry).
            if text.is_empty() && matches!(cell.value, CellValue::Empty) {
                continue;
            }
            cells.push(GridCell {
                row: r,
                col: c,
                text,
                align,
                // T0/Phase A: default style key (the style-map track wires
                // real keys later, like the page lowering).
                style_key: 0,
            });
        }
    }

    // ---- Gridlines at every visible track boundary (viewport-local). ----
    let gridlines = if opts.include_gridlines {
        let h = y_offsets
            .iter()
            .map(|&at| sheet_lower::Rule {
                at,
                from: 0.0,
                to: total_width,
            })
            .collect();
        let v = x_offsets
            .iter()
            .map(|&at| sheet_lower::Rule {
                at,
                from: 0.0,
                to: total_height,
            })
            .collect();
        RuleSet { h, v }
    } else {
        RuleSet::default()
    };

    // ---- Frozen-pane split (spec §8.1). Sum the frozen leading rows'/cols'
    // pt extents from the sheet origin (independent of the scroll origin — the
    // band is always the FIRST `freeze_rows`/`freeze_cols` of the sheet). ----
    let freeze = if opts.freeze_rows == 0 && opts.freeze_cols == 0 {
        None
    } else {
        let frozen_width_pt = (0..opts.freeze_cols).map(|c| col_width_pt(ws, c)).sum();
        let frozen_height_pt = (0..opts.freeze_rows).map(|r| row_height_pt(ws, r)).sum();
        Some(GridFreeze {
            rows: opts.freeze_rows,
            cols: opts.freeze_cols,
            frozen_width_pt,
            frozen_height_pt,
        })
    };

    GridScene {
        viewport: GridViewport {
            first_row,
            first_col,
            rows,
            cols,
            x_offsets,
            y_offsets,
        },
        cells,
        // Phase A: a single default style entry (key 0), mirroring the page
        // lowering's IR-v2 default style.
        styles: vec![LoweredStyle::default_key0()],
        gridlines,
        // The panel supplies selection later (spec §8.1).
        selection: None,
        freeze,
    }
}

/// Column width in pt: explicit `col_widths` (xlsx chars × `CHAR_TO_PT`) or
/// the xlsx default. Mirrors the page lowering so a column matches on both
/// surfaces.
fn col_width_pt(ws: Option<&sheet_core::Worksheet>, col: u32) -> f64 {
    let chars = ws
        .and_then(|w| w.col_widths.get(&col).copied())
        .unwrap_or(DEFAULT_COL_CHARS);
    chars * CHAR_TO_PT
}

/// Row height in pt: explicit `row_heights` (already pt) or the default.
fn row_height_pt(ws: Option<&sheet_core::Worksheet>, row: u32) -> f64 {
    ws.and_then(|w| w.row_heights.get(&row).copied())
        .unwrap_or(DEFAULT_ROW_PT)
}

/// Format one cell to `(text, align)` — the SAME number-format render +
/// Excel-default alignment as the page lowering, so a value reads
/// identically on the grid and the page (spec §8.3).
fn lower_cell_text(
    model: &SheetModel,
    cell: &Cell,
    cache: &mut FormatCache,
    ctx: &FormatCtx,
) -> (String, Align) {
    let code = model.styles.num_fmt_of(cell.style);
    let text = match cache.get(code) {
        Ok(fmt) => sheet_format::format_value(&cell.value, fmt, ctx),
        Err(_) => sheet_format::format_general(&cell.value),
    };
    let style_align = model.styles.style(cell.style).align;
    let align = resolve_align(style_align, &cell.value);
    (text, align)
}

/// Excel-default alignment (matches the page lowering's ruling): explicit
/// non-`General` style alignment wins; otherwise numbers right, text left,
/// bool/error centered, blank stays `General`.
fn resolve_align(style_align: Align, value: &CellValue) -> Align {
    if style_align != Align::General {
        return style_align;
    }
    match value {
        CellValue::Number(_) => Align::Right,
        CellValue::Text(_) => Align::Left,
        CellValue::Bool(_) | CellValue::Error(_) => Align::Center,
        CellValue::Empty => Align::General,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::CellStyle;
    use std::time::Instant;

    fn num(n: f64) -> Cell {
        Cell {
            value: CellValue::Number(n),
            ..Default::default()
        }
    }

    fn text(s: &str) -> Cell {
        Cell {
            value: CellValue::from(s),
            ..Default::default()
        }
    }

    /// A one-sheet model with the given `(row, col, cell)` seeds.
    fn model_with(cells: &[(u32, u32, Cell)]) -> (SheetModel, SheetId) {
        let mut m = SheetModel::new();
        let s = m.add_sheet("Sheet1");
        let ws = m.sheet_mut(s).unwrap();
        for (r, c, cell) in cells {
            ws.set_cell(*r, *c, cell.clone());
        }
        (m, s)
    }

    #[test]
    fn sheet_grid_scene_windowing_touches_only_viewport_on_sparse_million_rows() {
        // A sparse 1M-row sheet: one cell in the viewport, a handful FAR
        // outside it. Windowing must materialize only the in-viewport cell,
        // and do so fast (no O(sheet) scan).
        let mut m = SheetModel::new();
        let s = m.add_sheet("Big");
        {
            let ws = m.sheet_mut(s).unwrap();
            ws.set_cell(0, 0, num(1.0)); // in viewport
            ws.set_cell(5, 1, num(2.0)); // in viewport (small window below)
                                         // Far away — must NOT be materialized.
            ws.set_cell(500_000, 0, num(9.0));
            ws.set_cell(1_000_000, 50, text("far"));
            ws.set_cell(900_000, 5, num(7.0));
        }

        let start = Instant::now();
        // A small viewport: ~3 default cols wide, ~6 default rows tall.
        let scene = grid_scene(&m, s, 0, 0, 140.0, 95.0, &GridOptions::default());
        let elapsed = start.elapsed();

        // Only the two in-window populated cells are materialized.
        assert_eq!(scene.cells.len(), 2, "windowing must skip far-away cells");
        let coords: Vec<(u32, u32)> = scene.cells.iter().map(|c| (c.row, c.col)).collect();
        assert!(coords.contains(&(0, 0)));
        assert!(coords.contains(&(5, 1)));
        // Timing sanity: a windowed query over a 5-cell sparse map is
        // sub-millisecond; generously bound it so CI noise never flakes.
        assert!(
            elapsed.as_millis() < 50,
            "windowing should be fast, took {elapsed:?}"
        );
    }

    #[test]
    fn sheet_grid_scene_offsets_are_cumulative_with_trailing_edge() {
        let (m, s) = model_with(&[]);
        // Default cols (44.2575 pt) and rows (15 pt).
        let scene = grid_scene(&m, s, 0, 0, 100.0, 35.0, &GridOptions::default());
        let vp = &scene.viewport;
        // x_offsets carries cols+1 entries, cumulative from 0.
        assert_eq!(vp.x_offsets.len() as u32, vp.cols + 1);
        assert_eq!(vp.y_offsets.len() as u32, vp.rows + 1);
        assert_eq!(vp.x_offsets[0], 0.0);
        assert_eq!(vp.y_offsets[0], 0.0);
        for w in vp.x_offsets.windows(2) {
            assert!(w[1] > w[0], "x offsets strictly increasing");
        }
        // 100pt / 44.2575 ≈ 2 cols fit (the 3rd would overflow).
        assert_eq!(vp.cols, 2);
        // 35pt / 15 ≈ 2 rows fit.
        assert_eq!(vp.rows, 2);
        // Trailing x edge equals the summed widths.
        let last = *vp.x_offsets.last().unwrap();
        assert!((last - 2.0 * 44.2575).abs() < 1e-9);
    }

    #[test]
    fn sheet_grid_scene_formatted_text_matches_lower() {
        // The grid cell text must equal what sheet-format/the page lowering
        // produces for the same value+format (spec §8.3 parity).
        let mut m = SheetModel::new();
        let s = m.add_sheet("Sheet1");
        let fmt = m.styles.intern_num_fmt("0.00");
        let style = m.styles.intern_style(CellStyle {
            num_fmt: fmt,
            ..Default::default()
        });
        m.sheet_mut(s).unwrap().set_cell(
            0,
            0,
            Cell {
                value: CellValue::Number(3.5),
                style,
                ..Default::default()
            },
        );
        let scene = grid_scene(&m, s, 0, 0, 200.0, 60.0, &GridOptions::default());
        let cell = scene
            .cells
            .iter()
            .find(|c| c.row == 0 && c.col == 0)
            .unwrap();
        assert_eq!(cell.text, "3.50");
        assert_eq!(cell.align, Align::Right);

        // Cross-check against the page lowering for the identical model/cell.
        let lc = sheet_lower::lower_range(
            &m,
            s,
            sheet_lower::CellRange {
                r0: 0,
                c0: 0,
                r1: 0,
                c1: 0,
            },
            &sheet_lower::ViewOptions::default(),
        );
        assert_eq!(cell.text, lc.rows[0].cells[0].text);
        assert_eq!(cell.align, lc.rows[0].cells[0].align);
    }

    #[test]
    fn sheet_grid_scene_offsets_first_col_scroll_origin() {
        // Scrolling right: the viewport starts at first_col, offsets local.
        let (m, s) = model_with(&[(0, 5, num(5.0))]);
        let scene = grid_scene(&m, s, 0, 5, 200.0, 30.0, &GridOptions::default());
        assert_eq!(scene.viewport.first_col, 5);
        assert_eq!(scene.viewport.x_offsets[0], 0.0); // viewport-local
                                                      // The seeded cell at (0,5) is visible.
        assert!(scene.cells.iter().any(|c| c.row == 0 && c.col == 5));
    }

    #[test]
    fn sheet_grid_scene_selection_is_none_in_phase_a() {
        let (m, s) = model_with(&[]);
        let scene = grid_scene(&m, s, 0, 0, 100.0, 30.0, &GridOptions::default());
        assert!(scene.selection.is_none());
        assert_eq!(scene.styles, vec![LoweredStyle::default_key0()]);
    }

    #[test]
    fn sheet_grid_scene_gridlines_toggle() {
        let (m, s) = model_with(&[]);
        let on = grid_scene(&m, s, 0, 0, 100.0, 35.0, &GridOptions::default());
        assert!(!on.gridlines.h.is_empty());
        assert!(!on.gridlines.v.is_empty());
        let off = grid_scene(
            &m,
            s,
            0,
            0,
            100.0,
            35.0,
            &GridOptions {
                include_gridlines: false,
                ..GridOptions::default()
            },
        );
        assert!(off.gridlines.h.is_empty());
        assert!(off.gridlines.v.is_empty());
    }

    #[test]
    fn sheet_grid_scene_unknown_sheet_is_shaped_empty() {
        let (m, _s) = model_with(&[]);
        let scene = grid_scene(&m, 99, 0, 0, 100.0, 35.0, &GridOptions::default());
        // Geometry still derives from the bounds; no cells crash.
        assert!(scene.cells.is_empty());
        assert!(scene.viewport.cols >= 1);
        assert!(scene.viewport.rows >= 1);
    }

    #[test]
    fn sheet_grid_scene_freeze_panes_band_geometry() {
        // Frozen 1 col + 2 rows: the scene carries the split + the band pt
        // extents (summed from the sheet origin, independent of scroll).
        let (m, s) = model_with(&[]);
        let scene = grid_scene(
            &m,
            s,
            0,
            0,
            200.0,
            90.0,
            &GridOptions {
                include_gridlines: true,
                freeze_rows: 2,
                freeze_cols: 1,
            },
        );
        let fz = scene.freeze.expect("freeze present");
        assert_eq!(fz.rows, 2);
        assert_eq!(fz.cols, 1);
        // 1 default col = 44.2575 pt; 2 default rows = 30 pt.
        assert!((fz.frozen_width_pt - 44.2575).abs() < 1e-9);
        assert!((fz.frozen_height_pt - 30.0).abs() < 1e-9);
    }

    #[test]
    fn sheet_grid_scene_freeze_none_when_no_split() {
        let (m, s) = model_with(&[]);
        let scene = grid_scene(&m, s, 0, 0, 200.0, 90.0, &GridOptions::default());
        assert!(scene.freeze.is_none());
        // The wire omits `freeze` only as null (the field is always present).
        let json = serde_json::to_string(&scene).unwrap();
        assert!(json.contains("\"freeze\":null"));
    }

    #[test]
    fn sheet_grid_scene_freeze_band_uses_explicit_widths() {
        // Explicit col width / row height feed the frozen band extents.
        let mut m = SheetModel::new();
        let s = m.add_sheet("Sheet1");
        {
            let ws = m.sheet_mut(s).unwrap();
            ws.col_widths.insert(0, 10.0); // 10 ch → 52.5 pt
            ws.row_heights.insert(0, 30.0); // pt
        }
        let scene = grid_scene(
            &m,
            s,
            0,
            0,
            300.0,
            120.0,
            &GridOptions {
                include_gridlines: true,
                freeze_rows: 1,
                freeze_cols: 1,
            },
        );
        let fz = scene.freeze.unwrap();
        assert!((fz.frozen_width_pt - 52.5).abs() < 1e-9);
        assert!((fz.frozen_height_pt - 30.0).abs() < 1e-9);
    }

    #[test]
    fn sheet_grid_scene_camelcase_wire_shape() {
        let (m, s) = model_with(&[(0, 0, num(1.0))]);
        let scene = grid_scene(&m, s, 0, 0, 100.0, 30.0, &GridOptions::default());
        let json = serde_json::to_string(&scene).unwrap();
        assert!(json.contains("\"firstRow\""));
        assert!(json.contains("\"xOffsets\""));
        assert!(json.contains("\"styleKey\""));
        assert!(!json.contains("first_row"));
        assert!(!json.contains("x_offsets"));
    }
}
