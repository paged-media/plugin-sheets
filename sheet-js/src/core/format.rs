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

    /// The full style of one cell (every field populated): number-format
    /// code, font, fill, borders, alignment, wrap. Theme / indexed colours
    /// read as empty (only explicit RGB is reported).
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
        })
    }
}
