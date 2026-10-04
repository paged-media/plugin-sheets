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

//! Wave 4 session operations: the page-lowering door (styles + conditional
//! formatting), the host clock, iterative calculation, worksheet
//! add/rename/delete and the row/column insert/delete door.
//!
//! Each operation keeps the ENGINE (model + dependency graph) and the
//! CONTAINER (`XlsxDocument` — parts, bindings, formula texts) in step, the
//! same coherence dance `save_xlsx` documents: model surgery happens on the
//! model taken out of the engine, which is then rebuilt.

use std::collections::BTreeSet;

use sheet_calc::Engine;
use sheet_core::ast::{Expr, LitValue};
use sheet_core::names::{NameDef, NameScope, NameTable, NameTarget};
use sheet_core::{CellError, DateSystem, SheetId, SheetModel};
use sheet_format::{FormatCache, FormatCtx};
use sheet_lower::{lower_range_condfmt, ViewOptions};
use sheet_parser::{print, Edit};
use sheet_xlsx::structure::{quote_sheet, rewrite_sheet_refs};

use super::{
    cell_display, CellChange, CircularRef, LowerOptions, ModelSheetNames, SessionError,
    SetCellResult, SheetSession, T0_LOWER_CELL_CAP,
};

/// Days from the 1900-system epoch to 1970-01-01 (Excel serial of the Unix
/// epoch; the 1900 leap-bug day is already inside it).
const UNIX_EPOCH_SERIAL_1900: f64 = 25569.0;
/// The same for the 1904 system.
const UNIX_EPOCH_SERIAL_1904: f64 = 24107.0;
const MS_PER_DAY: f64 = 86_400_000.0;

/// Which axis a structural edit shifts, and whether it inserts or deletes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum StructuralEdit {
    InsertRows,
    DeleteRows,
    InsertCols,
    DeleteCols,
}

/// The calc settings the panel shows (`<calcPr>` iteration knobs).
#[derive(serde::Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CalcSettingsInfo {
    pub iterative: bool,
    pub max_iter: u32,
    pub max_change: f64,
}

/// Excel's sheet-name rules: 1–31 chars, none of `: \ / ? * [ ]`, not
/// starting or ending with `'`, and not the reserved `History`.
fn validate_sheet_name(name: &str) -> Result<(), SessionError> {
    let n = name.chars().count();
    if n == 0 || n > 31 {
        return Err(SessionError(format!(
            "sheet name must be 1–31 characters (got {n})"
        )));
    }
    if let Some(c) = name
        .chars()
        .find(|c| matches!(c, ':' | '\\' | '/' | '?' | '*' | '[' | ']'))
    {
        return Err(SessionError(format!("sheet name may not contain {c:?}")));
    }
    if name.starts_with('\'') || name.ends_with('\'') {
        return Err(SessionError(
            "sheet name may not start or end with an apostrophe".into(),
        ));
    }
    if name.eq_ignore_ascii_case("History") {
        return Err(SessionError("\"History\" is a reserved sheet name".into()));
    }
    Ok(())
}

/// Rewrite the sheet ids an expression references after sheet `gone` is
/// deleted: references INTO it become `#REF!`, later sheets shift down.
/// Returns whether a reference to the deleted sheet was replaced.
fn drop_sheet_refs(e: &mut Expr, gone: SheetId) -> bool {
    match e {
        Expr::Ref(r) => {
            if r.sheet == gone {
                *e = Expr::Lit(LitValue::Error(CellError::Ref));
                return true;
            }
            if r.sheet > gone {
                r.sheet -= 1;
            }
            false
        }
        Expr::Range(r) => {
            if r.start.sheet == gone || r.end.sheet == gone {
                *e = Expr::Lit(LitValue::Error(CellError::Ref));
                return true;
            }
            if r.start.sheet > gone {
                r.start.sheet -= 1;
            }
            if r.end.sheet > gone {
                r.end.sheet -= 1;
            }
            false
        }
        Expr::Unary(_, a) | Expr::SpillRef(a) => drop_sheet_refs(a, gone),
        Expr::Binary(_, a, b) => {
            let x = drop_sheet_refs(a, gone);
            drop_sheet_refs(b, gone) || x
        }
        Expr::Func(_, args) => args
            .iter_mut()
            .fold(false, |acc, a| drop_sheet_refs(a, gone) || acc),
        Expr::Array(rows) => rows
            .iter_mut()
            .flatten()
            .fold(false, |acc, a| drop_sheet_refs(a, gone) || acc),
        Expr::Call(callee, args) => {
            let x = drop_sheet_refs(callee, gone);
            args.iter_mut()
                .fold(x, |acc, a| drop_sheet_refs(a, gone) || acc)
        }
        Expr::Lit(_) | Expr::Name(_) | Expr::StructuredRef(_) | Expr::Local(_) => false,
    }
}

impl SheetSession {
    /// Run `f` on the model taken out of the engine, then rebuild the engine
    /// (the `save_xlsx` dance). `recalc` re-evaluates every formula after.
    fn with_model<R>(&mut self, recalc: bool, f: impl FnOnce(&mut SheetModel) -> R) -> R {
        let engine = self.engine.take().expect("engine present outside save");
        let mut model = engine.into_model();
        let out = f(&mut model);
        let mut engine = Engine::new(model, self.config);
        if recalc {
            engine.recalc_all();
        }
        self.engine = Some(engine);
        out
    }

    /// Map an engine recalc result to the wire shape (formatted displays).
    fn changes_of(&self, result: &sheet_calc::RecalcResult) -> SetCellResult {
        let model = self.engine().model();
        let mut cache = FormatCache::default();
        let ctx = FormatCtx::new(model.calc.date_system, model.calc.locale);
        SetCellResult {
            changed: result
                .changed
                .iter()
                .map(|c| CellChange {
                    sheet: c.sheet,
                    row: c.row,
                    col: c.col,
                    display: cell_display(model, c.sheet, c.row, c.col, &mut cache, &ctx),
                })
                .collect(),
            circular: result
                .circular
                .iter()
                .map(|c| CircularRef {
                    sheet: c.sheet,
                    row: c.row,
                    col: c.col,
                })
                .collect(),
        }
    }

    // ── 1. the page lowering ────────────────────────────────────────────

    /// Lower a range for the PAGE: the workbook's real per-cell styles (fill,
    /// borders, font facets) with CONDITIONAL FORMATTING folded on top
    /// (`lower_range_condfmt`; data bars ride the IR too), grid rules on by
    /// default. This is the door the placed table uses; `get_range_lowered`
    /// keeps its frozen key-0 contract for its other callers.
    pub fn get_range_page(
        &self,
        sheet: u16,
        range: &str,
        opts: LowerOptions,
    ) -> Result<sheet_lower::LoweredContent, SessionError> {
        let (sheet, cell_range) = self.resolve_range(sheet, range)?;
        let (top, left, bottom, right) = (
            cell_range.r0.min(cell_range.r1) as u64,
            cell_range.c0.min(cell_range.c1) as u64,
            cell_range.r0.max(cell_range.r1) as u64,
            cell_range.c0.max(cell_range.c1) as u64,
        );
        if (bottom - top + 1) * (right - left + 1) > T0_LOWER_CELL_CAP {
            return Err(SessionError(format!(
                "range exceeds the T0 lowering cap ({T0_LOWER_CELL_CAP} cells)"
            )));
        }
        let view = ViewOptions {
            include_grid_rules: opts.include_grid_rules.unwrap_or(true),
            header_rows: opts.header_rows.unwrap_or(0),
        };
        let cf = self.doc.lowered_conditional_formats(sheet);
        Ok(lower_range_condfmt(
            self.engine().model(),
            sheet,
            cell_range,
            &view,
            &self.doc.visual_styles,
            &cf,
        ))
    }

    // ── 4. the host clock ────────────────────────────────────────────────

    /// Set `NOW`/`TODAY` from the host clock: `unix_ms` since the Unix epoch
    /// (UTC) and the host's `Date#getTimezoneOffset()` (minutes, UTC − local),
    /// converted to a LOCAL serial in the workbook's date system. Returns the
    /// serial. Does not recalc — see [`SheetSession::recalc_volatile`].
    pub fn set_clock(&mut self, unix_ms: f64, tz_offset_min: f64) -> f64 {
        let epoch = match self.engine().model().calc.date_system {
            DateSystem::Date1900 => UNIX_EPOCH_SERIAL_1900,
            DateSystem::Date1904 => UNIX_EPOCH_SERIAL_1904,
        };
        let local_ms = unix_ms - tz_offset_min * 60_000.0;
        let serial = epoch + local_ms / MS_PER_DAY;
        self.set_now(serial);
        serial
    }

    /// The current `NOW`/`TODAY` serial.
    pub fn now_serial(&self) -> f64 {
        self.config.now_serial
    }

    /// Recalculate the volatile cells (`NOW`, `TODAY`, `RAND`, …) against the
    /// current clock — the recalc the host runs at boot and before showing
    /// time-dependent values. Returns the changed displays.
    pub fn recalc_volatile(&mut self) -> SetCellResult {
        let result = self.engine_mut().recalc_dirty();
        self.changes_of(&result)
    }

    // ── 7. iterative calculation ─────────────────────────────────────────

    /// The `<calcPr>` iteration knobs in effect (loaded from the file).
    pub fn calc_settings(&self) -> CalcSettingsInfo {
        let c = &self.engine().model().calc;
        CalcSettingsInfo {
            iterative: c.iterative,
            max_iter: c.max_iter,
            max_change: c.max_change,
        }
    }

    /// Toggle iterative calculation; written back to `<calcPr>` on save.
    pub fn set_iterative(&mut self, on: bool, max_iter: u32, max_change: f64) -> SetCellResult {
        let result = self.engine_mut().set_iterative(on, max_iter, max_change);
        self.structure_changed = true;
        self.changes_of(&result)
    }

    // ── 5. worksheets ────────────────────────────────────────────────────

    /// The first free default name `SheetN`.
    fn default_sheet_name(&self) -> String {
        let model = self.engine().model();
        (1..)
            .map(|n| format!("Sheet{n}"))
            .find(|cand| model.sheet_id(cand).is_none())
            .expect("an unbounded range finds a free name")
    }

    /// ADD a worksheet at the end (empty `name` = the next free `SheetN`).
    /// Returns its id. The new sheet is written as its own worksheet part.
    pub fn add_sheet(&mut self, name: &str) -> Result<u16, SessionError> {
        let name = if name.trim().is_empty() {
            self.default_sheet_name()
        } else {
            name.to_string()
        };
        validate_sheet_name(&name)?;
        if self.engine().model().sheet_id(&name).is_some() {
            return Err(SessionError(format!("a sheet named {name:?} exists")));
        }
        let id = self.engine().model().sheets.len() as SheetId;
        self.doc
            .add_sheet_part(id, &name)
            .map_err(|e| SessionError(e.to_string()))?;
        self.with_model(false, |m| m.add_sheet(name.as_str()));
        self.structure_changed = true;
        Ok(id)
    }

    /// RENAME a worksheet. Formulas reference sheets by id, so their values
    /// are unaffected; their stored TEXT (and defined names) is re-printed
    /// with the new name on save.
    pub fn rename_sheet(&mut self, sheet: u16, name: &str) -> Result<(), SessionError> {
        self.validate_sheet(sheet)?;
        validate_sheet_name(name)?;
        let old = self.engine().model().sheets[sheet as usize]
            .name
            .to_string();
        if old == name {
            return Ok(());
        }
        if let Some(other) = self.engine().model().sheet_id(name) {
            if other != sheet {
                return Err(SessionError(format!("a sheet named {name:?} exists")));
            }
        }
        self.doc
            .rename_sheet_part(sheet, &old, name)
            .map_err(|e| SessionError(e.to_string()))?;
        let quoted = quote_sheet(name);
        self.with_model(false, |m| {
            m.sheets[sheet as usize].name = name.into();
            m.names = rebuild_names(&m.names, |scope, target| match target {
                NameTarget::Formula(text) => (
                    Some(scope),
                    NameTarget::Formula(rewrite_sheet_refs(text, &old, &quoted).0.into()),
                ),
                other => (Some(scope), other.clone()),
            });
        });
        // Every formula whose stored text names the sheet is re-printed.
        let touched: Vec<((SheetId, u32, u32), String)> = self
            .doc
            .formula_texts
            .iter()
            .filter_map(|(k, t)| {
                let (new, changed) = rewrite_sheet_refs(t, &old, &quoted);
                changed.then_some((*k, new))
            })
            .collect();
        for (key, text) in touched {
            self.retext_formula(key, text);
        }
        self.structure_changed = true;
        Ok(())
    }

    /// A formula cell's stored text changed without an edit: a parsed formula
    /// is re-printed from its AST on save; an UNPARSED one (kept verbatim) is
    /// rewritten in place and its sheet re-encoded.
    fn retext_formula(&mut self, key: (SheetId, u32, u32), text: String) {
        let (s, r, c) = key;
        let parsed = self
            .engine()
            .model()
            .sheet(s)
            .and_then(|ws| ws.cell(r, c))
            .and_then(|cell| cell.formula)
            .is_some();
        if parsed {
            self.edited.insert(key);
        } else {
            self.doc.formula_texts.insert(key, text);
            self.extra_dirty.insert(s);
        }
    }

    /// DELETE a worksheet. References to it in other sheets' formulas and in
    /// defined names become `#REF!` (Excel's behaviour); later sheets shift
    /// down one id. Refused when it is the last sheet, or when the workbook
    /// carries charts (their series ranges are not re-pointed yet).
    pub fn delete_sheet(&mut self, sheet: u16) -> Result<SetCellResult, SessionError> {
        self.validate_sheet(sheet)?;
        if self.engine().model().sheets.len() <= 1 {
            return Err(SessionError(
                "a workbook must keep at least one sheet".into(),
            ));
        }
        if !self.doc.charts.is_empty() {
            return Err(SessionError(
                "this workbook has charts; deleting a sheet would leave their ranges \
                 pointing at the wrong sheet (not supported yet)"
                    .into(),
            ));
        }
        let name = self.engine().model().sheets[sheet as usize]
            .name
            .to_string();
        self.doc
            .remove_sheet_part(sheet, &name)
            .map_err(|e| SessionError(e.to_string()))?;

        let gone = sheet;
        let refd: Vec<(SheetId, u32, u32)> = self.with_model(false, |m| {
            // Formulas: #REF! into the deleted sheet, shift later ids.
            let mut cells: Vec<((SheetId, u32, u32), sheet_core::ast::Formula)> = Vec::new();
            for (si, ws) in m.sheets.iter().enumerate() {
                if si as SheetId == gone {
                    continue;
                }
                for (&(r, c), cell) in ws.iter_cells() {
                    if let Some(f) = cell.formula.and_then(|fid| m.formula(fid)) {
                        cells.push(((si as SheetId, r, c), f.clone()));
                    }
                }
            }
            let mut refd = Vec::new();
            let mut rewritten = Vec::new();
            for ((s, r, c), mut f) in cells {
                let hit = drop_sheet_refs(&mut f.root, gone);
                let s2 = if s > gone { s - 1 } else { s };
                if hit {
                    refd.push((s2, r, c));
                }
                rewritten.push(((s, r, c), f));
            }
            for ((s, r, c), f) in rewritten {
                let fid = m.intern_formula(f);
                if let Some(cell) = m.sheet_mut(s).and_then(|ws| ws.cells.get_mut(&(r, c))) {
                    cell.formula = Some(fid);
                }
            }
            m.sheets.remove(gone as usize);
            for ws in &mut m.sheets {
                for t in &mut ws.tables {
                    if t.range.start.sheet > gone {
                        t.range.start.sheet -= 1;
                    }
                    if t.range.end.sheet > gone {
                        t.range.end.sheet -= 1;
                    }
                }
            }
            m.names = rebuild_names(&m.names, |scope, target| {
                let scope = match scope {
                    NameScope::Sheet(k) if k == gone => None,
                    NameScope::Sheet(k) if k > gone => Some(NameScope::Sheet(k - 1)),
                    other => Some(other),
                };
                let target = match target {
                    NameTarget::Formula(text) => {
                        NameTarget::Formula(rewrite_sheet_refs(text, &name, "#REF").0.into())
                    }
                    NameTarget::Range(r) => {
                        let mut r = *r;
                        if r.start.sheet > gone {
                            r.start.sheet -= 1;
                        }
                        if r.end.sheet > gone {
                            r.end.sheet -= 1;
                        }
                        NameTarget::Range(r)
                    }
                };
                (scope, target)
            });
            refd
        });
        // Re-key the pending edit set; formulas that now hold #REF! re-print.
        let edited = std::mem::take(&mut self.edited);
        for (s, r, c) in edited {
            if s < gone {
                self.edited.insert((s, r, c));
            } else if s > gone {
                self.edited.insert((s - 1, r, c));
            }
        }
        let extra = std::mem::take(&mut self.extra_dirty);
        self.extra_dirty = extra
            .into_iter()
            .filter(|&s| s != gone)
            .map(|s| if s > gone { s - 1 } else { s })
            .collect();
        for key in refd {
            self.edited.insert(key);
        }
        // Unparsed formula texts naming the deleted sheet.
        let touched: Vec<((SheetId, u32, u32), String)> = self
            .doc
            .formula_texts
            .iter()
            .filter_map(|(k, t)| {
                let (new, changed) = rewrite_sheet_refs(t, &name, "#REF");
                changed.then_some((*k, new))
            })
            .collect();
        for (key, text) in touched {
            self.retext_formula(key, text);
        }
        match self.selection.take() {
            Some((s, _)) if s == gone => {}
            Some((s, g)) if s > gone => self.selection = Some((s - 1, g)),
            other => self.selection = other,
        }
        self.structure_changed = true;
        let result = self.engine_mut().recalc_all();
        Ok(self.changes_of(&result))
    }

    // ── 6. rows and columns ──────────────────────────────────────────────

    /// Insert or delete `n` rows/columns at 0-based `at` on `sheet`: cells,
    /// merges, column widths and row heights shift; every formula's
    /// references are rewritten (`#REF!` for deleted spans) and the workbook
    /// recalculates. REFUSED (model untouched) when the sheet carries
    /// preserved content that addresses its cells and that the writer cannot
    /// shift (conditional formats, validations, hyperlinks, tables, drawings,
    /// comments, defined names, …) — "Paged never destroys a workbook".
    pub fn structural_edit(
        &mut self,
        sheet: u16,
        kind: StructuralEdit,
        at: u32,
        n: u32,
    ) -> Result<SetCellResult, SessionError> {
        self.validate_sheet(sheet)?;
        if n == 0 {
            return Err(SessionError("count must be at least 1".into()));
        }
        let (limit, axis) = match kind {
            StructuralEdit::InsertRows | StructuralEdit::DeleteRows => (1_048_576u32, "row"),
            StructuralEdit::InsertCols | StructuralEdit::DeleteCols => (16_384u32, "column"),
        };
        if at.checked_add(n).is_none_or(|end| end > limit) {
            return Err(SessionError(format!(
                "{axis} span {at}+{n} is past the sheet edge"
            )));
        }
        let mut blockers = self
            .doc
            .structural_edit_blockers(sheet, self.engine().model());
        // A formula kept verbatim (unparsed) cannot have its references
        // rewritten — on this sheet, or elsewhere naming this sheet.
        let sheet_name = self.engine().model().sheets[sheet as usize]
            .name
            .to_string();
        let model = self.engine().model();
        if self.doc.formula_texts.iter().any(|(&(s, r, c), t)| {
            let unparsed = model
                .sheet(s)
                .and_then(|ws| ws.cell(r, c))
                .is_none_or(|cell| cell.formula.is_none());
            unparsed && (s == sheet || rewrite_sheet_refs(t, &sheet_name, "X").1)
        }) {
            blockers.push("a formula Paged keeps verbatim (unparsed)".into());
        }
        if !blockers.is_empty() {
            return Err(SessionError(format!(
                "cannot insert/delete {axis}s here without breaking preserved content: {}",
                blockers.join(", ")
            )));
        }

        let edit = match kind {
            StructuralEdit::InsertRows => Edit::InsertRows { sheet, at, n },
            StructuralEdit::DeleteRows => Edit::DeleteRows { sheet, at, n },
            StructuralEdit::InsertCols => Edit::InsertCols { sheet, at, n },
            StructuralEdit::DeleteCols => Edit::DeleteCols { sheet, at, n },
        };
        let result = self.engine_mut().apply_edit(&edit);

        // The edited sheet's stored formula texts are positional: drop them
        // all and re-print every formula cell there (and re-encode the sheet,
        // whose cells moved even when it holds no formulas).
        self.doc.formula_texts.retain(|&(s, _, _), _| s != sheet);
        self.edited.retain(|&(s, _, _)| s != sheet);
        self.extra_dirty.insert(sheet);
        let mut reprint: BTreeSet<(SheetId, u32, u32)> = BTreeSet::new();
        {
            let model = self.engine().model();
            let names = ModelSheetNames { model };
            for (si, ws) in model.sheets.iter().enumerate() {
                let si = si as SheetId;
                for (&(r, c), cell) in ws.iter_cells() {
                    let Some(f) = cell.formula.and_then(|fid| model.formula(fid)) else {
                        continue;
                    };
                    if si == sheet {
                        reprint.insert((si, r, c));
                        continue;
                    }
                    // Another sheet: re-print only when a reference moved.
                    let text = print(f, si, &names);
                    if self.doc.formula_texts.get(&(si, r, c)) != Some(&text) {
                        reprint.insert((si, r, c));
                    }
                }
            }
        }
        self.edited.extend(reprint);
        Ok(self.changes_of(&result))
    }
}

/// Rebuild a name table, keeping every name's position (so `NameId`s held by
/// formulas stay valid). `f(scope, target)` returns the new scope (`None`
/// orphans a name whose sheet is gone — it stays in the table, unreachable)
/// and the new target.
fn rebuild_names(
    names: &NameTable,
    mut f: impl FnMut(NameScope, &NameTarget) -> (Option<NameScope>, NameTarget),
) -> NameTable {
    let mut out = NameTable::default();
    for (_, def) in names.iter() {
        let (scope, target) = f(def.scope, &def.target);
        out.define(NameDef {
            name: def.name.clone(),
            scope: scope.unwrap_or(NameScope::Sheet(SheetId::MAX)),
            target,
        });
    }
    out
}
