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

//! Wave 6 formatting & layout doors: cell styles (`set_style` /
//! `get_style`), merges, column widths / row heights, frozen panes, defined
//! names, and the range resolver every lowering door shares (an A1 range, a
//! `Sheet!A1:B2` range, a defined name or a table name).
//!
//! None of these doors changes a value, so none rebuilds the engine for a
//! style or a size: they go through the `sheet_calc` layout doors, which
//! leave the dependency graph and the dirty set alone. Every door marks the
//! sheet for re-encode on the next save (`extra_dirty`).

use std::collections::BTreeSet;

use sheet_xlsx::{EdgePatch, StylePatch};

use super::{SessionError, SheetSession, T0_LOWER_CELL_CAP};
use sheet_lower::CellRange;

/// One border edge across the wasm door: `{ style, color? }` (`style`
/// `"none"` removes the line; `color` `#RRGGBB`).
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct EdgeArg {
    pub style: String,
    pub color: Option<String>,
}

/// A partial cell style across the wasm door (camelCase). Absent fields are
/// left as each cell has them; an EMPTY `fontColor` / `fill` clears it.
/// `get_style` answers the same shape, fully populated.
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StylePatchArg {
    pub num_fmt: Option<String>,
    pub font_name: Option<String>,
    pub font_size: Option<f64>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub font_color: Option<String>,
    pub fill: Option<String>,
    pub border_top: Option<EdgeArg>,
    pub border_right: Option<EdgeArg>,
    pub border_bottom: Option<EdgeArg>,
    pub border_left: Option<EdgeArg>,
    pub h_align: Option<String>,
    pub v_align: Option<String>,
    pub wrap: Option<bool>,
}

impl From<StylePatchArg> for StylePatch {
    fn from(a: StylePatchArg) -> StylePatch {
        let edge = |e: Option<EdgeArg>| {
            e.map(|e| EdgePatch {
                style: e.style,
                color: e.color,
            })
        };
        StylePatch {
            num_fmt: a.num_fmt,
            font_name: a.font_name,
            font_size: a.font_size,
            bold: a.bold,
            italic: a.italic,
            underline: a.underline,
            font_color: a.font_color,
            fill: a.fill,
            border_top: edge(a.border_top),
            border_right: edge(a.border_right),
            border_bottom: edge(a.border_bottom),
            border_left: edge(a.border_left),
            h_align: a.h_align,
            v_align: a.v_align,
            wrap: a.wrap,
        }
    }
}

impl From<StylePatch> for StylePatchArg {
    fn from(p: StylePatch) -> StylePatchArg {
        let edge = |e: Option<EdgePatch>| {
            e.map(|e| EdgeArg {
                style: e.style,
                color: e.color,
            })
        };
        StylePatchArg {
            num_fmt: p.num_fmt,
            font_name: p.font_name,
            font_size: p.font_size,
            bold: p.bold,
            italic: p.italic,
            underline: p.underline,
            font_color: p.font_color,
            fill: p.fill,
            border_top: edge(p.border_top),
            border_right: edge(p.border_right),
            border_bottom: edge(p.border_bottom),
            border_left: edge(p.border_left),
            h_align: p.h_align,
            v_align: p.v_align,
            wrap: p.wrap,
        }
    }
}

/// What `set_style` did: how many cells it restyled and how many distinct
/// styles they now use.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SetStyleResult {
    pub cells: u32,
    pub styles: u32,
}

/// `(top, left, bottom, right)` of a range, normalized.
pub(crate) fn bounds(r: &CellRange) -> (u32, u32, u32, u32) {
    (
        r.r0.min(r.r1),
        r.c0.min(r.c1),
        r.r0.max(r.r1),
        r.c0.max(r.c1),
    )
}

impl SheetSession {
    // ── cell styles ──────────────────────────────────────────────────────

    /// Apply a partial style to every cell of `range` on `sheet` (blank
    /// cells included — a fill on an empty cell is real formatting). Each
    /// distinct current style is patched once; `styles.xml` gains only the
    /// records the patch needs (deduped), and the model adopts the re-parsed
    /// style table. No value changes, nothing recalculates. A bad value (a
    /// colour that is not `#RRGGBB`, an unknown alignment or border style) is
    /// a boundary error and changes nothing.
    pub fn set_style(
        &mut self,
        sheet: u16,
        range: &str,
        patch: StylePatchArg,
    ) -> Result<SetStyleResult, SessionError> {
        let (sheet, cr) = self.resolve_range(sheet, range)?;
        let (top, left, bottom, right) = bounds(&cr);
        let area = (bottom - top + 1) as u64 * (right - left + 1) as u64;
        if area > T0_LOWER_CELL_CAP {
            return Err(SessionError(format!(
                "range exceeds the style cap ({T0_LOWER_CELL_CAP} cells)"
            )));
        }
        let patch: StylePatch = patch.into();
        if let Some(code) = &patch.num_fmt {
            // The engine must be able to RENDER the code it stores.
            sheet_format::compile(code)
                .map_err(|e| SessionError(format!("number format {code:?}: {e:?}")))?;
        }
        if patch.is_empty() {
            return Ok(SetStyleResult::default());
        }
        let model = self.engine().model();
        let ws = model
            .sheet(sheet)
            .ok_or_else(|| SessionError(format!("sheet id {sheet} out of range")))?;
        let mut bases: BTreeSet<u32> = BTreeSet::new();
        for r in top..=bottom {
            for c in left..=right {
                bases.insert(ws.cell(r, c).map(|x| x.style.0).unwrap_or(0));
            }
        }
        let (map, table) = self
            .doc
            .apply_style_patch(&bases, &patch)
            .map_err(|e| SessionError(e.to_string()))?;
        let engine = self.engine_mut();
        // Read each cell's current style before replacing the table (ids are
        // positional, so the old id still names the same record).
        let mut assignments = Vec::with_capacity(area as usize);
        {
            let ws = engine.model().sheet(sheet).expect("validated");
            for r in top..=bottom {
                for c in left..=right {
                    let old = ws.cell(r, c).map(|x| x.style.0).unwrap_or(0);
                    assignments.push((r, c, map[&old]));
                }
            }
        }
        engine.replace_styles(table);
        for (r, c, id) in assignments {
            engine.set_cell_style(sheet, r, c, id);
        }
        self.extra_dirty.insert(sheet);
        let styles: BTreeSet<u32> = map.values().map(|s| s.0).collect();
        Ok(SetStyleResult {
            cells: area as u32,
            styles: styles.len() as u32,
        })
    }

    /// The style ids of every cell of `range`, row-major — an exact
    /// snapshot a caller restores with [`Self::set_style_ids`] (the in-session
    /// undo of a format change). Ids are positional and the style table only
    /// grows, so an id read now names the same record later.
    pub fn get_style_ids(&self, sheet: u16, range: &str) -> Result<Vec<u32>, SessionError> {
        let (sheet, cr) = self.resolve_range(sheet, range)?;
        let (top, left, bottom, right) = bounds(&cr);
        let area = (bottom - top + 1) as u64 * (right - left + 1) as u64;
        if area > T0_LOWER_CELL_CAP {
            return Err(SessionError(format!(
                "range exceeds the style cap ({T0_LOWER_CELL_CAP} cells)"
            )));
        }
        let ws = self
            .engine()
            .model()
            .sheet(sheet)
            .ok_or_else(|| SessionError(format!("sheet id {sheet} out of range")))?;
        let mut out = Vec::with_capacity(area as usize);
        for r in top..=bottom {
            for c in left..=right {
                out.push(ws.cell(r, c).map(|x| x.style.0).unwrap_or(0));
            }
        }
        Ok(out)
    }

    /// Put back a [`Self::get_style_ids`] snapshot: `ids` row-major over
    /// `range`. Refused whole (nothing changes) when the count does not
    /// match the range or an id names no style record. No value changes,
    /// nothing recalculates; the sheet re-encodes on the next save.
    pub fn set_style_ids(
        &mut self,
        sheet: u16,
        range: &str,
        ids: &[u32],
    ) -> Result<(), SessionError> {
        let (sheet, cr) = self.resolve_range(sheet, range)?;
        let (top, left, bottom, right) = bounds(&cr);
        let area = (bottom - top + 1) as u64 * (right - left + 1) as u64;
        if ids.len() as u64 != area {
            return Err(SessionError(format!(
                "{} style ids for a range of {area} cells",
                ids.len()
            )));
        }
        let count = self.engine().model().styles.style_count() as u32;
        if let Some(bad) = ids.iter().find(|&&id| id >= count) {
            return Err(SessionError(format!(
                "style id {bad} names no style record"
            )));
        }
        let engine = self.engine_mut();
        let mut it = ids.iter();
        for r in top..=bottom {
            for c in left..=right {
                let id = *it.next().expect("count checked");
                engine.set_cell_style(sheet, r, c, sheet_core::StyleId(id));
            }
        }
        self.extra_dirty.insert(sheet);
        Ok(())
    }

    /// The full style of one cell (every field populated): number-format
    /// code, font, fill, borders, alignment, wrap. Colours are resolved:
    /// explicit RGB, the workbook's indexed palette and its theme part's
    /// scheme, tints applied (empty only for `auto` / no colour).
    pub fn get_style(&self, sheet: u16, row: u32, col: u32) -> Result<StylePatchArg, SessionError> {
        self.validate_sheet(sheet)?;
        let id = self
            .engine()
            .model()
            .sheet(sheet)
            .and_then(|ws| ws.cell(row, col))
            .map(|c| c.style)
            .unwrap_or_default();
        self.doc
            .describe_style(id)
            .map(StylePatchArg::from)
            .map_err(|e| SessionError(e.to_string()))
    }

    // ── range resolution ────────────────────────────────────────────────

    /// Resolve a range argument as seen from `sheet`: an A1 range
    /// (`B2:D9`, `C3`), a sheet-qualified range (`Data!A1:B5`,
    /// `'My Sheet'!A1`), a defined name whose target is a range, or a table
    /// name (its full extent, header and totals rows included). Returns the
    /// sheet the range lives on and the range.
    pub fn resolve_range(&self, sheet: u16, text: &str) -> Result<(u16, CellRange), SessionError> {
        self.validate_sheet(sheet)?;
        let text = text.trim();
        if let Ok(cr) = super::parse_range(text) {
            return Ok((sheet, cr));
        }
        let model = self.engine().model();
        if let Some((sheet_part, range_part)) = text.rsplit_once('!') {
            let name = sheet_part
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .map(|s| s.replace("''", "'"))
                .unwrap_or_else(|| sheet_part.to_string());
            if let Some(sid) = model.sheet_id(&name) {
                let cr = super::parse_range(range_part.replace('$', "").as_str())?;
                return Ok((sid, cr));
            }
        }
        if let Some(id) = model.names.resolve(text, sheet) {
            if let Some(sheet_core::NameTarget::Range(r)) = model.names.get(id).map(|d| &d.target) {
                let n = r.normalized();
                return Ok((
                    n.start.sheet,
                    CellRange {
                        r0: n.start.row,
                        c0: n.start.col,
                        r1: n.end.row,
                        c1: n.end.col,
                    },
                ));
            }
        }
        for (sid, ws) in model.sheets.iter().enumerate() {
            if let Some(t) = ws.tables.iter().find(|t| t.name.eq_ignore_ascii_case(text)) {
                let n = t.range.normalized();
                return Ok((
                    sid as u16,
                    CellRange {
                        r0: n.start.row,
                        c0: n.start.col,
                        r1: n.end.row,
                        c1: n.end.col,
                    },
                ));
            }
        }
        Err(SessionError(format!(
            "invalid range: {text:?} is not a range, a defined name or a table"
        )))
    }
}

/// A resolved range across the wasm door: the sheet it lives on and its A1
/// text (`B2:D9`).
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedRange {
    pub sheet: u16,
    pub range: String,
    /// The normalized 0-based bounds (inclusive), so a host can size the
    /// rows / columns a range covers without parsing A1.
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
}

impl SheetSession {
    /// [`resolve_range`](Self::resolve_range) answered in A1 (the wasm door).
    pub fn resolve_range_a1(&self, sheet: u16, text: &str) -> Result<ResolvedRange, SessionError> {
        let (sheet, cr) = self.resolve_range(sheet, text)?;
        let (t, l, b, r) = bounds(&cr);
        Ok(ResolvedRange {
            sheet,
            range: format!(
                "{}{}:{}{}",
                sheet_core::col_to_a1(l),
                t + 1,
                sheet_core::col_to_a1(r),
                b + 1
            ),
            top: t,
            left: l,
            bottom: b,
            right: r,
        })
    }
}

/// The result of [`SheetSession::merge`]: recomputed displays + circular
/// set (like `set_cell`) and the cells the merge CLEARED (their prior and
/// new input, for the bundle's undo journal).
#[derive(serde::Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub changed: Vec<super::CellChange>,
    pub circular: Vec<super::CircularRef>,
    pub edits: Vec<super::CellEdit>,
}

/// A sheet's layout, for a format panel: explicit column widths
/// (characters) and row heights (points), merges in A1, the frozen split.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct SheetLayoutInfo {
    pub col_widths: Vec<(u32, f64)>,
    pub row_heights: Vec<(u32, f64)>,
    pub merges: Vec<String>,
    pub freeze_rows: u32,
    pub freeze_cols: u32,
}

/// `A1` text of a 0-based cell.
fn a1(row: u32, col: u32) -> String {
    format!("{}{}", sheet_core::col_to_a1(col), row + 1)
}

/// Excel's column-width ceiling (characters) and row-height ceiling (pt).
const MAX_COL_WIDTH: f64 = 255.0;
const MAX_ROW_HEIGHT: f64 = 409.0;

impl SheetSession {
    // ── merges ──────────────────────────────────────────────────────────

    /// Merge `range` (at least two cells). Excel's rule: the top-left cell
    /// keeps its content, every other non-blank cell is CLEARED through the
    /// normal entry lane (dependents recalc; the cleared inputs are returned
    /// for the undo journal). Refused — model untouched — when the range
    /// overlaps an existing merge or any part of a spilled array.
    pub fn merge(&mut self, sheet: u16, range: &str) -> Result<MergeResult, SessionError> {
        let (sheet, cr) = self.resolve_range(sheet, range)?;
        let (top, left, bottom, right) = bounds(&cr);
        if top == bottom && left == right {
            return Err(SessionError("a merge needs at least two cells".into()));
        }
        let overlaps = |m: &sheet_core::RangeRef| {
            let n = m.normalized();
            n.start.row <= bottom && n.end.row >= top && n.start.col <= right && n.end.col >= left
        };
        let engine = self.engine();
        let ws = engine.model().sheet(sheet).expect("validated");
        if ws.merges.iter().any(overlaps) {
            return Err(SessionError(
                "the range overlaps an existing merge (unmerge it first)".into(),
            ));
        }
        let mut to_clear = Vec::new();
        for (&(r, c), _) in ws.cells.range((top, 0)..=(bottom, u32::MAX)) {
            if c < left || c > right {
                continue;
            }
            let at = sheet_core::CellRef {
                sheet,
                row: r,
                col: c,
                row_abs: false,
                col_abs: false,
            };
            let spills = engine.spills();
            if spills.owner_of(at).is_some() || spills.region_of(at).is_some() {
                return Err(SessionError(
                    "the range covers part of a spilled array".into(),
                ));
            }
            if (r, c) != (top, left) {
                let input = super::cell_input_text(engine.model(), sheet, r, c);
                if !input.is_empty() {
                    to_clear.push((r, c, input));
                }
            }
        }
        let mut changed: Vec<super::CellChange> = Vec::new();
        let mut circular = Vec::new();
        let mut edits = Vec::new();
        for (r, c, prev) in to_clear {
            let res = self.set_cell(sheet, r, c, "")?;
            changed.retain(|x| {
                !res.changed
                    .iter()
                    .any(|y| (y.sheet, y.row, y.col) == (x.sheet, x.row, x.col))
            });
            changed.extend(res.changed);
            circular = res.circular;
            edits.push(super::CellEdit {
                sheet,
                row: r,
                col: c,
                prev_input: prev,
                next_input: String::new(),
            });
        }
        let range_ref = sheet_core::RangeRef {
            start: sheet_core::CellRef {
                sheet,
                row: top,
                col: left,
                row_abs: false,
                col_abs: false,
            },
            end: sheet_core::CellRef {
                sheet,
                row: bottom,
                col: right,
                row_abs: false,
                col_abs: false,
            },
        };
        let layout = self
            .engine_mut()
            .sheet_layout_mut(sheet)
            .expect("validated");
        layout.merges.push(range_ref);
        self.extra_dirty.insert(sheet);
        Ok(MergeResult {
            changed,
            circular,
            edits,
        })
    }

    /// Remove every merge that intersects `range`; returns how many.
    pub fn unmerge(&mut self, sheet: u16, range: &str) -> Result<u32, SessionError> {
        let (sheet, cr) = self.resolve_range(sheet, range)?;
        let (top, left, bottom, right) = bounds(&cr);
        let layout = self
            .engine_mut()
            .sheet_layout_mut(sheet)
            .expect("validated");
        let before = layout.merges.len();
        layout.merges.retain(|m| {
            let n = m.normalized();
            !(n.start.row <= bottom
                && n.end.row >= top
                && n.start.col <= right
                && n.end.col >= left)
        });
        let removed = (before - layout.merges.len()) as u32;
        if removed > 0 {
            self.extra_dirty.insert(sheet);
        }
        Ok(removed)
    }

    // ── column widths / row heights ─────────────────────────────────────

    /// Set (or with `None`, clear back to the default) the width of columns
    /// `first..=last`, in characters (the xlsx unit; 0–255).
    pub fn set_col_width(
        &mut self,
        sheet: u16,
        first: u32,
        last: u32,
        width: Option<f64>,
    ) -> Result<(), SessionError> {
        self.validate_sheet(sheet)?;
        if let Some(w) = width {
            if !(w.is_finite() && (0.0..=MAX_COL_WIDTH).contains(&w)) {
                return Err(SessionError(format!(
                    "column width {w} outside 0–{MAX_COL_WIDTH} characters"
                )));
            }
        }
        let (a, b) = (first.min(last), first.max(last).min(sheet_core::MAX_COL));
        let layout = self
            .engine_mut()
            .sheet_layout_mut(sheet)
            .expect("validated");
        for c in a..=b {
            match width {
                Some(w) => layout.col_widths.insert(c, w),
                None => layout.col_widths.remove(&c),
            };
        }
        self.extra_dirty.insert(sheet);
        Ok(())
    }

    /// Set (or with `None`, clear) the height of rows `first..=last`, in
    /// points (0–409).
    pub fn set_row_height(
        &mut self,
        sheet: u16,
        first: u32,
        last: u32,
        height: Option<f64>,
    ) -> Result<(), SessionError> {
        self.validate_sheet(sheet)?;
        if let Some(h) = height {
            if !(h.is_finite() && (0.0..=MAX_ROW_HEIGHT).contains(&h)) {
                return Err(SessionError(format!(
                    "row height {h} outside 0–{MAX_ROW_HEIGHT} pt"
                )));
            }
        }
        let (a, b) = (first.min(last), first.max(last).min(sheet_core::MAX_ROW));
        if b - a > T0_LOWER_CELL_CAP as u32 {
            return Err(SessionError("too many rows in one call".into()));
        }
        let layout = self
            .engine_mut()
            .sheet_layout_mut(sheet)
            .expect("validated");
        for r in a..=b {
            match height {
                Some(h) => layout.row_heights.insert(r, h),
                None => layout.row_heights.remove(&r),
            };
        }
        self.extra_dirty.insert(sheet);
        Ok(())
    }

    /// The sheet's layout: explicit sizes, merges (A1) and frozen split.
    pub fn get_layout(&self, sheet: u16) -> Result<SheetLayoutInfo, SessionError> {
        self.validate_sheet(sheet)?;
        let ws = self.engine().model().sheet(sheet).expect("validated");
        let fp = self.doc.freeze_panes_of(sheet);
        Ok(SheetLayoutInfo {
            col_widths: ws.col_widths.iter().map(|(&c, &w)| (c, w)).collect(),
            row_heights: ws.row_heights.iter().map(|(&r, &h)| (r, h)).collect(),
            merges: ws
                .merges
                .iter()
                .map(|m| {
                    let n = m.normalized();
                    format!(
                        "{}:{}",
                        a1(n.start.row, n.start.col),
                        a1(n.end.row, n.end.col)
                    )
                })
                .collect(),
            freeze_rows: fp.rows,
            freeze_cols: fp.cols,
        })
    }
}

impl SheetSession {
    // ── frozen panes ────────────────────────────────────────────────────

    /// Freeze the first `rows` rows and `cols` columns of `sheet`; `0, 0`
    /// clears the freeze. Persisted to the sheet's `<sheetView><pane>`; the
    /// grid scene reads it.
    pub fn set_freeze(&mut self, sheet: u16, rows: u32, cols: u32) -> Result<(), SessionError> {
        self.validate_sheet(sheet)?;
        if rows > sheet_core::MAX_ROW || cols > sheet_core::MAX_COL {
            return Err(SessionError("freeze split outside the sheet".into()));
        }
        self.doc
            .set_freeze_panes(sheet, rows, cols)
            .map_err(|e| SessionError(e.to_string()))?;
        self.extra_dirty.insert(sheet);
        Ok(())
    }
}

// ── defined names ────────────────────────────────────────────────────────

/// One defined name, for a name manager.
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NameInfo {
    pub name: String,
    /// `None` = workbook scope; `Some(sheet)` = visible on that sheet only.
    pub scope: Option<u16>,
    /// The target as the workbook writes it (`Sheet1!$A$1:$B$3`, or the
    /// raw formula text of a non-range name).
    pub refers_to: String,
}

/// The scope a deleted (or orphaned) name is parked in: no sheet has this
/// id, so it never resolves, while formulas holding its `NameId` keep it.
const PARKED: sheet_core::SheetId = sheet_core::SheetId::MAX;

/// Excel's defined-name rules: starts with a letter, `_` or `\`; then
/// letters, digits, `_`, `.`, `\`; at most 255 characters; not `TRUE` /
/// `FALSE`, not something that reads as a cell reference (`A1`, `XFD9`,
/// `R1C1`, `R`, `C`).
fn validate_name(name: &str) -> Result<(), SessionError> {
    let bad = |why: &str| Err(SessionError(format!("invalid name {name:?}: {why}")));
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return bad("empty");
    };
    if name.chars().count() > 255 {
        return bad("longer than 255 characters");
    }
    if !(first.is_alphabetic() || first == '_' || first == '\\') {
        return bad("must start with a letter, '_' or '\\'");
    }
    if !chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '\\')) {
        return bad("letters, digits, '_', '.' and '\\' only");
    }
    let up = name.to_ascii_uppercase();
    if matches!(up.as_str(), "TRUE" | "FALSE" | "R" | "C") {
        return bad("reserved");
    }
    if sheet_core::parse_a1(name).is_some() {
        return bad("reads as a cell reference");
    }
    // R1C1-style (`R1C1`, `R2`, `C3`).
    let rc = |s: &str| {
        let s = s.strip_prefix('R').unwrap_or(s);
        let (r, rest) = s.split_at(s.find('C').unwrap_or(s.len()));
        r.chars().all(|c| c.is_ascii_digit())
            && rest
                .strip_prefix('C')
                .unwrap_or(rest)
                .chars()
                .all(|c| c.is_ascii_digit())
    };
    if (up.starts_with('R') || up.starts_with('C')) && rc(&up) {
        return bad("reads as an R1C1 reference");
    }
    Ok(())
}

/// Turn every workbook name whose raw target text is a plain reference
/// (`Sheet1!$A$1:$A$3`, `$B$5`) into a RANGE target, so formulas using it
/// evaluate and the dependency graph registers its box. Names whose target
/// is a formula or a constant (`0.5`) keep their text. Called on load,
/// before the engine is built.
pub(crate) fn resolve_name_targets(model: &mut sheet_core::SheetModel) {
    use sheet_core::ast::Expr;
    use sheet_core::NameTarget;
    let mut resolved = Vec::new();
    for (id, def) in model.names.iter() {
        let NameTarget::Formula(text) = &def.target else {
            continue;
        };
        let current = match def.scope {
            sheet_core::NameScope::Sheet(s) => s,
            sheet_core::NameScope::Workbook => 0,
        };
        let ctx = super::ModelParseCtx { model, current };
        let range = match sheet_parser::parse(text, &ctx).map(|f| f.root) {
            Ok(Expr::Range(r)) => r,
            Ok(Expr::Ref(c)) => sheet_core::RangeRef { start: c, end: c },
            _ => continue,
        };
        // A relative part is relative to the REFERRING cell (`$A1048576` is
        // "the row above"); such a name stays a formula target, which the
        // engine re-bases per use (sheet-calc `names`).
        let rel = |c: &sheet_core::CellRef| !c.row_abs || !c.col_abs;
        if rel(&range.start) || rel(&range.end) {
            continue;
        }
        resolved.push((id, range));
    }
    if resolved.is_empty() {
        return;
    }
    let mut out = sheet_core::NameTable::default();
    for (id, def) in model.names.iter() {
        let target = match resolved.iter().find(|(i, _)| *i == id) {
            Some((_, r)) => NameTarget::Range(*r),
            None => def.target.clone(),
        };
        out.define(sheet_core::NameDef {
            name: def.name.clone(),
            scope: def.scope,
            target,
        });
    }
    model.names = out;
}

/// The workbook text of a range target (`Sheet1!$A$1:$B$3`, `Sheet1!$B$5`).
fn range_text(model: &sheet_core::SheetModel, r: &sheet_core::RangeRef) -> String {
    let n = r.normalized();
    let sheet = model
        .sheet(n.start.sheet)
        .map(|w| sheet_xlsx::structure::quote_sheet(&w.name))
        .unwrap_or_else(|| "#REF".into());
    let abs = |row: u32, col: u32| format!("${}${}", sheet_core::col_to_a1(col), row + 1);
    if n.start.row == n.end.row && n.start.col == n.end.col {
        format!("{sheet}!{}", abs(n.start.row, n.start.col))
    } else {
        format!(
            "{sheet}!{}:{}",
            abs(n.start.row, n.start.col),
            abs(n.end.row, n.end.col)
        )
    }
}

impl SheetSession {
    /// Define (or redefine) the name `name` → the range `refers_to` (an A1
    /// range on `sheet`, a `Sheet!A1:B2` range, or another name / table),
    /// workbook-scoped or — with `scope` — visible on that sheet only. A
    /// redefinition keeps the name's slot, so formulas already using it
    /// follow the new target; a previously deleted name is revived in place.
    /// Persisted to `<definedNames>`; every formula recalculates.
    pub fn define_name(
        &mut self,
        sheet: u16,
        name: &str,
        refers_to: &str,
        scope: Option<u16>,
    ) -> Result<(), SessionError> {
        let name = name.trim();
        validate_name(name)?;
        if let Some(s) = scope {
            self.validate_sheet(s)?;
        }
        let (target_sheet, cr) = self.resolve_range(sheet, refers_to)?;
        let (top, left, bottom, right) = bounds(&cr);
        let cell = |row, col| sheet_core::CellRef {
            sheet: target_sheet,
            row,
            col,
            row_abs: true,
            col_abs: true,
        };
        let range = sheet_core::RangeRef {
            start: cell(top, left),
            end: cell(bottom, right),
        };
        let text = range_text(self.engine().model(), &range);
        let new_scope = match scope {
            Some(s) => sheet_core::NameScope::Sheet(s),
            None => sheet_core::NameScope::Workbook,
        };
        // Which slot: the live name in this scope, else a parked one of the
        // same spelling, else a new slot.
        let names = &self.engine().model().names;
        let live = names
            .iter()
            .find(|(_, d)| d.name.eq_ignore_ascii_case(name) && d.scope == new_scope)
            .map(|(id, _)| id);
        let parked = names
            .iter()
            .find(|(_, d)| {
                d.name.eq_ignore_ascii_case(name) && d.scope == sheet_core::NameScope::Sheet(PARKED)
            })
            .map(|(id, _)| id);
        let slot = live.or(parked);
        self.doc
            .set_defined_name(name, scope.map(u32::from), &text)
            .map_err(|e| SessionError(e.to_string()))?;
        let def = sheet_core::NameDef {
            name: name.into(),
            scope: new_scope,
            target: sheet_core::NameTarget::Range(range),
        };
        self.with_names(|old| {
            let mut out = sheet_core::NameTable::default();
            for (id, d) in old.iter() {
                out.define(if Some(id) == slot {
                    def.clone()
                } else {
                    d.clone()
                });
            }
            if slot.is_none() {
                out.define(def.clone());
            }
            out
        });
        Ok(())
    }

    /// Delete the name `name` in `scope` (`None` = workbook). Formulas using
    /// it read `#NAME?` (Excel's rule); its slot is parked so their
    /// `NameId`s stay valid. Removed from `<definedNames>`.
    pub fn delete_name(
        &mut self,
        _sheet: u16,
        name: &str,
        scope: Option<u16>,
    ) -> Result<(), SessionError> {
        let want = match scope {
            Some(s) => sheet_core::NameScope::Sheet(s),
            None => sheet_core::NameScope::Workbook,
        };
        let slot = self
            .engine()
            .model()
            .names
            .iter()
            .find(|(_, d)| d.name.eq_ignore_ascii_case(name.trim()) && d.scope == want)
            .map(|(id, _)| id)
            .ok_or_else(|| SessionError(format!("no defined name {name:?} in that scope")))?;
        let spelled = self
            .engine()
            .model()
            .names
            .get(slot)
            .map(|d| d.name.to_string())
            .unwrap_or_default();
        self.doc
            .remove_defined_name(&spelled, scope.map(u32::from))
            .map_err(|e| SessionError(e.to_string()))?;
        self.with_names(|old| {
            let mut out = sheet_core::NameTable::default();
            for (id, d) in old.iter() {
                if id == slot {
                    out.define(sheet_core::NameDef {
                        name: d.name.clone(),
                        scope: sheet_core::NameScope::Sheet(PARKED),
                        target: sheet_core::NameTarget::Formula("#NAME?".into()),
                    });
                } else {
                    out.define(d.clone());
                }
            }
            out
        });
        Ok(())
    }

    /// Every live defined name (built-in `_xlnm.` names and parked slots
    /// left out), in definition order.
    pub fn list_names(&self) -> Vec<NameInfo> {
        let model = self.engine().model();
        model
            .names
            .iter()
            .filter(|(_, d)| !d.name.starts_with("_xlnm."))
            .filter_map(|(_, d)| {
                let scope = match d.scope {
                    sheet_core::NameScope::Workbook => None,
                    sheet_core::NameScope::Sheet(s) if (s as usize) < model.sheets.len() => Some(s),
                    sheet_core::NameScope::Sheet(_) => return None,
                };
                let refers_to = match &d.target {
                    sheet_core::NameTarget::Range(r) => range_text(model, r),
                    sheet_core::NameTarget::Formula(t) => t.to_string(),
                };
                Some(NameInfo {
                    name: d.name.to_string(),
                    scope,
                    refers_to,
                })
            })
            .collect()
    }

    /// Replace the name table (positions kept by the caller), rebuild the
    /// dependency graph and recalculate every formula.
    fn with_names(&mut self, f: impl FnOnce(&sheet_core::NameTable) -> sheet_core::NameTable) {
        let engine = self.engine.take().expect("engine present outside save");
        let mut model = engine.into_model();
        model.names = f(&model.names);
        let mut engine = sheet_calc::Engine::new(model, self.config);
        engine.recalc_all();
        self.engine = Some(engine);
    }
}
