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

//! # sheet-calc — the calculation engine (spec §6.2/§6.3)
//!
//! The salsa-shaped core that joins the parser, the function library, and the
//! [`SheetModel`]: a dependency graph ([`graph`]), dirty propagation
//! ([`dirty`]), a deterministic topological scheduler ([`topo`]), the
//! tree-walk evaluator ([`eval`]), range-argument materialization
//! ([`argview`]), and volatile-pass support ([`volatile`]).
//!
//! ## The public surface (FROZEN — `sheet-js` builds against exactly this)
//!
//! [`Engine`] owns the model + graph + dirty set + config. The cell-entry
//! door is [`Engine::enter`] (Excel-like literal/formula detection lives HERE,
//! in Rust); [`Engine::set_cell`] is the structured door; the recalc family is
//! [`Engine::recalc_all`] / [`Engine::recalc_dirty`]; structural edits go
//! through [`Engine::apply_edit`].
//!
//! ## The circular-reference encoding (registry `sheet.calc.circular`)
//!
//! [`CellError`] has no `Circular` variant (the wire enum is frozen at 8
//! codes). When the scheduler detects a cycle, each cycle member's STORED
//! value becomes [`CellValue::Error`]`(`[`CellError::Ref`]`)` (the
//! `#REF!`-class display the registry row's title calls for), AND the full
//! cycle set is reported on [`RecalcResult::circular`]. The glue surfaces the
//! circular WARNING from `circular`; the cell displays the `#REF!`-class
//! error. Breaking the cycle (re-entering one member as a non-cyclic formula
//! or a literal) clears both on the next recalc.
//!
//! ## Determinism
//!
//! Recalc order is independent of hash iteration order: the scheduler drains a
//! stable frontier ([`topo`]). The volatile RNG seed is derived from the
//! config seed mixed with a monotonic pass counter and the cell address
//! ([`volatile::cell_seed`]) — so `RAND` varies across passes yet is
//! reproducible under a fixed seed.

pub mod argview;
pub mod dirty;
pub mod eval;
pub mod graph;
pub mod topo;
pub mod volatile;

use sheet_core::ast::Formula;
use sheet_core::{Cell, CellError, CellRef, CellValue, NameId, SheetId, SheetModel};
use sheet_parser::{Edit, ParseCtx};

use crate::dirty::Dirty;
use crate::graph::DepGraph;

/// Per-engine configuration. `now_serial` feeds `NOW`/`TODAY`; `rng_seed`
/// seeds the deterministic volatile RNG. Defaults: `0.0` / `0x5EED`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EngineConfig {
    pub now_serial: f64,
    pub rng_seed: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            now_serial: 0.0,
            rng_seed: 0x5EED,
        }
    }
}

/// The outcome of a recalc: the cells whose stored value CHANGED in this pass,
/// and the cells found on a cycle (the `sheet.calc.circular` set).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecalcResult {
    /// Formula cells whose stored value changed during this recalc.
    pub changed: Vec<CellRef>,
    /// Formula cells on a circular reference (stored as `#REF!`).
    pub circular: Vec<CellRef>,
}

/// The cell-entry payload for [`Engine::set_cell`] — the structured door
/// (`enter` is the Excel-like raw-text door above it).
pub enum SetInput {
    /// Clear the cell (blank).
    Empty,
    /// A literal value.
    Value(CellValue),
    /// A parsed formula.
    Formula(Formula),
}

/// The calculation engine. Owns the [`SheetModel`], the dependency
/// [`DepGraph`], the [`Dirty`] cut, the [`EngineConfig`], and a monotonic
/// pass counter that seeds volatile RNG.
pub struct Engine {
    model: SheetModel,
    graph: DepGraph,
    dirty: Dirty,
    config: EngineConfig,
    /// Monotonic recalc-pass counter (mixed into the volatile RNG seed so
    /// `RAND` changes each pass but stays deterministic under a fixed seed).
    pass: u64,
}

impl Engine {
    /// Build from an existing model (e.g. an xlsx load): registers every cell
    /// that has a [`sheet_core::FormulaId`], marks everything dirty. Does NOT
    /// recalc — the caller chooses `recalc_all`/`recalc_dirty`.
    pub fn new(model: SheetModel, config: EngineConfig) -> Engine {
        let mut graph = DepGraph::new();
        graph.rebuild(&model);
        let mut dirty = Dirty::new();
        dirty.mark_all(&graph);
        // Seed volatility from each formula's extracted refs.
        seed_volatility(&model, &graph, &mut dirty);
        Engine {
            model,
            graph,
            dirty,
            config,
            pass: 0,
        }
    }

    /// Read-only access to the underlying model.
    pub fn model(&self) -> &SheetModel {
        &self.model
    }

    /// Escape hatch for structural consumers (the `sheet-js` save path).
    ///
    /// MUTATING cells through the returned model WITHOUT going back through
    /// [`Engine::set_cell`]/[`Engine::enter`] DESYNCS the dependency graph and
    /// dirty set — the engine cannot observe edits made directly on the model.
    /// Only use this when you are DONE with the engine (e.g. to serialize).
    pub fn into_model(self) -> SheetModel {
        self.model
    }

    /// Update the `NOW`/`TODAY` serial. Marks volatile cells for recompute on
    /// the next recalc (they are already reseeded each pass, but this makes the
    /// new clock visible without a separate write).
    pub fn set_now(&mut self, now_serial: f64) {
        self.config.now_serial = now_serial;
    }

    /// THE cell-entry door (Excel-like semantics live HERE, in Rust).
    ///
    /// - `raw` starting with `'='` parses as a formula (a [`ParseError`]
    ///   surfaces as `Err`).
    /// - Otherwise literal detection runs: a number (incl. a leading `+`/`-`
    ///   and scientific notation), `TRUE`/`FALSE` (case-insensitive), an error
    ///   literal (`#REF!` etc. via [`CellError::parse`]), the empty string →
    ///   [`CellValue::Empty`], anything else → [`CellValue::Text`].
    ///
    /// **NO date-literal parsing in T0** (`"2024-01-01"` is stored as Text,
    /// not a serial — dates are a number-format concern, deferred).
    ///
    /// Commits the cell and recalcs the resulting dirty cut.
    ///
    /// [`ParseError`]: sheet_parser::ParseError
    pub fn enter(
        &mut self,
        sheet: SheetId,
        row: u32,
        col: u32,
        raw: &str,
    ) -> Result<RecalcResult, sheet_parser::ParseError> {
        let input = if let Some(body) = raw.strip_prefix('=') {
            let ctx = ModelParseCtx {
                model: &self.model,
                current: sheet,
            };
            let formula = sheet_parser::parse(body, &ctx)?;
            SetInput::Formula(formula)
        } else {
            SetInput::Value(literal_of(raw))
        };
        Ok(self.set_cell(sheet, row, col, input))
    }

    /// The structured cell-entry door. Commits the input, updates the graph
    /// and dirty set, then recalcs the dirty cut.
    pub fn set_cell(
        &mut self,
        sheet: SheetId,
        row: u32,
        col: u32,
        input: SetInput,
    ) -> RecalcResult {
        let cref = CellRef {
            sheet,
            row,
            col,
            row_abs: false,
            col_abs: false,
        };
        self.ensure_sheet(sheet);

        // Drop any prior formula registration for this cell.
        self.graph.unregister(cref);
        self.dirty.forget(cref);

        match input {
            SetInput::Empty => {
                if let Some(ws) = self.model.sheet_mut(sheet) {
                    ws.remove_cell(row, col);
                }
            }
            SetInput::Value(v) => {
                let style = self.style_of(cref);
                if let Some(ws) = self.model.sheet_mut(sheet) {
                    ws.set_cell(
                        row,
                        col,
                        Cell {
                            value: v,
                            formula: None,
                            style,
                        },
                    );
                }
            }
            SetInput::Formula(f) => {
                let refs = sheet_parser::extract_refs(&f);
                let volatile = refs.has_volatile;
                let fid = self.model.intern_formula(f);
                let style = self.style_of(cref);
                if let Some(ws) = self.model.sheet_mut(sheet) {
                    ws.set_cell(
                        row,
                        col,
                        Cell {
                            value: CellValue::Empty,
                            formula: Some(fid),
                            style,
                        },
                    );
                }
                self.graph.register(cref, &refs, &self.model);
                self.dirty.set_volatile(cref, volatile);
                // The formula cell itself is dirty.
                self.dirty.mark(cref);
            }
        }

        // A write at this cell dirties its transitive dependents.
        self.dirty.propagate_from(cref, &self.graph);

        self.recalc_dirty()
    }

    /// Recalculate EVERY formula cell (marks all dirty, then runs a pass).
    pub fn recalc_all(&mut self) -> RecalcResult {
        self.dirty.mark_all(&self.graph);
        self.recalc_dirty()
    }

    /// Recalculate the current dirty cut (volatiles reseeded). The heart of
    /// the engine.
    pub fn recalc_dirty(&mut self) -> RecalcResult {
        self.pass = self.pass.wrapping_add(1);

        let cut = self.dirty.take_pass_seed();
        if cut.is_empty() {
            self.dirty.clear();
            return RecalcResult::default();
        }

        let to = topo::order(&cut, &self.graph);

        let mut changed: Vec<CellRef> = Vec::new();

        // Evaluate the orderable cells in dependency order; each write is
        // immediately visible to later cells (topo guarantees freshness).
        for cref in &to.order {
            let new_value = self.evaluate_cell(*cref);
            if self.commit_value(*cref, new_value) {
                changed.push(*cref);
            }
        }

        // Cycle members: store #REF! (the wire encoding of the Circular
        // diagnostic) and report them on `circular`.
        for cref in &to.cycle {
            let ref_err = CellValue::Error(CellError::Ref);
            if self.commit_value(*cref, ref_err) {
                changed.push(*cref);
            }
        }

        self.dirty.clear();

        changed.sort();
        RecalcResult {
            changed,
            circular: to.cycle,
        }
    }

    /// Apply a structural edit (insert/delete rows or cols): physically shift
    /// the cells, column widths, row heights, and merges; rewrite every
    /// formula's references (`sheet_parser::rewrite`, `#REF!` for deleted
    /// spans); rebuild the dependency graph; then recalc everything.
    pub fn apply_edit(&mut self, edit: &Edit) -> RecalcResult {
        self.apply_edit_structural(edit);
        self.rewrite_all_formulas(edit);
        // Rebuild the graph + dirty set from the mutated model.
        self.graph.rebuild(&self.model);
        self.dirty = Dirty::new();
        self.dirty.mark_all(&self.graph);
        seed_volatility(&self.model, &self.graph, &mut self.dirty);
        self.recalc_dirty()
    }

    // ---- internals ----

    /// Evaluate a single formula cell's expression (a literal cell evaluates
    /// to its stored value — it is never in the dirty cut as a formula, but
    /// stay total).
    fn evaluate_cell(&self, cref: CellRef) -> CellValue {
        let Some(ws) = self.model.sheet(cref.sheet) else {
            return CellValue::Empty;
        };
        let Some(cell) = ws.cell(cref.row, cref.col) else {
            return CellValue::Empty;
        };
        let Some(fid) = cell.formula else {
            return cell.value.clone();
        };
        let Some(f) = self.model.formula(fid) else {
            return CellValue::Error(CellError::Ref);
        };
        let seed = volatile::cell_seed(self.config.rng_seed, self.pass, cref);
        let ctx = eval::ctx_for(&self.model, cref, self.config.now_serial, seed);
        eval::eval_expr(&self.model, &f.root.clone(), &ctx)
    }

    /// Write a computed value into the cell, returning whether it changed.
    fn commit_value(&mut self, cref: CellRef, value: CellValue) -> bool {
        let Some(ws) = self.model.sheet_mut(cref.sheet) else {
            return false;
        };
        match ws.cell(cref.row, cref.col) {
            Some(existing) => {
                if existing.value == value {
                    return false;
                }
                let mut updated = existing.clone();
                updated.value = value;
                ws.set_cell(cref.row, cref.col, updated);
                true
            }
            None => {
                // A cycle/eval result for a cell that lost its record — store a
                // bare value cell (rare; keeps the engine total).
                ws.set_cell(
                    cref.row,
                    cref.col,
                    Cell {
                        value,
                        ..Default::default()
                    },
                );
                true
            }
        }
    }

    /// The current style id of a cell (preserved across re-entry), defaulting
    /// to `StyleId(0)`.
    fn style_of(&self, cref: CellRef) -> sheet_core::StyleId {
        self.model
            .sheet(cref.sheet)
            .and_then(|ws| ws.cell(cref.row, cref.col))
            .map(|c| c.style)
            .unwrap_or_default()
    }

    /// Ensure the target sheet exists (auto-create up to `sheet` so a write to
    /// a fresh single-sheet engine just works).
    fn ensure_sheet(&mut self, sheet: SheetId) {
        while (self.model.sheets.len() as u32) <= sheet as u32 {
            let n = self.model.sheets.len();
            self.model.add_sheet(format!("Sheet{}", n + 1));
        }
    }

    /// Physically shift the cell grid, col widths, row heights, and merges for
    /// a structural edit on its sheet.
    fn apply_edit_structural(&mut self, edit: &Edit) {
        let (sheet, axis, kind, at, n) = decompose(edit);
        let Some(ws) = self.model.sheet_mut(sheet) else {
            return;
        };

        // Shift cells.
        let old_cells = std::mem::take(&mut ws.cells);
        for ((row, col), cell) in old_cells {
            let coord = if axis == EditAxis::Row { row } else { col };
            match shift_coord(coord, kind, at, n) {
                Some(nc) => {
                    let (nr, ncc) = if axis == EditAxis::Row {
                        (nc, col)
                    } else {
                        (row, nc)
                    };
                    ws.cells.insert((nr, ncc), cell);
                }
                None => { /* deleted — drop the cell */ }
            }
        }

        // Shift the sizing maps (col_widths keyed by col, row_heights by row).
        match axis {
            EditAxis::Row => {
                ws.row_heights = shift_map(std::mem::take(&mut ws.row_heights), kind, at, n);
            }
            EditAxis::Col => {
                ws.col_widths = shift_map(std::mem::take(&mut ws.col_widths), kind, at, n);
            }
        }

        // Shift merges (drop any that collapse).
        let old_merges = std::mem::take(&mut ws.merges);
        for m in old_merges {
            if let Some(nm) = shift_merge(m, axis, kind, at, n) {
                ws.merges.push(nm);
            }
        }
    }

    /// Rewrite every formula in the model for the structural edit. Re-interns
    /// the rewritten formula and repoints the cell's `FormulaId`.
    fn rewrite_all_formulas(&mut self, edit: &Edit) {
        // Collect (cref, old_fid) for every formula cell.
        let mut targets: Vec<(CellRef, sheet_core::FormulaId)> = Vec::new();
        for (sheet_idx, ws) in self.model.sheets.iter().enumerate() {
            for (&(row, col), cell) in ws.iter_cells() {
                if let Some(fid) = cell.formula {
                    targets.push((
                        CellRef {
                            sheet: sheet_idx as SheetId,
                            row,
                            col,
                            row_abs: false,
                            col_abs: false,
                        },
                        fid,
                    ));
                }
            }
        }
        for (cref, fid) in targets {
            let Some(old) = self.model.formula(fid).cloned() else {
                continue;
            };
            let rewritten = sheet_parser::rewrite(&old, edit);
            let new_fid = self.model.intern_formula(rewritten);
            if let Some(ws) = self.model.sheet_mut(cref.sheet) {
                if let Some(existing) = ws.cell(cref.row, cref.col) {
                    let mut updated = existing.clone();
                    updated.formula = Some(new_fid);
                    ws.set_cell(cref.row, cref.col, updated);
                }
            }
        }
    }
}

/// Seed the dirty tracker's volatile set from each formula's extracted refs.
fn seed_volatility(model: &SheetModel, graph: &DepGraph, dirty: &mut Dirty) {
    for cref in graph.formula_cells_sorted() {
        if let Some(ws) = model.sheet(cref.sheet) {
            if let Some(cell) = ws.cell(cref.row, cref.col) {
                if let Some(fid) = cell.formula {
                    if let Some(f) = model.formula(fid) {
                        let refs = sheet_parser::extract_refs(f);
                        dirty.set_volatile(cref, refs.has_volatile);
                    }
                }
            }
        }
    }
}

/// Excel-like literal detection for a non-formula raw cell entry (the `enter`
/// ruling). Order: empty → blank; number (incl. leading `+`/`-`, scientific) →
/// Number; `TRUE`/`FALSE` (case-insensitive) → Bool; error literal → Error;
/// otherwise → Text. NO date-literal parsing in T0.
fn literal_of(raw: &str) -> CellValue {
    if raw.is_empty() {
        return CellValue::Empty;
    }
    // TRUE / FALSE (case-insensitive).
    if raw.eq_ignore_ascii_case("TRUE") {
        return CellValue::Bool(true);
    }
    if raw.eq_ignore_ascii_case("FALSE") {
        return CellValue::Bool(false);
    }
    // Error literal (#REF! etc).
    if let Some(e) = CellError::parse(raw) {
        return CellValue::Error(e);
    }
    // Number (mirrors the coerce::to_number text ruling: leading +/-, sci, no
    // thousands separators / inf / nan / underscores).
    if let Some(n) = parse_number_literal(raw) {
        return CellValue::Number(n);
    }
    CellValue::Text(raw.into())
}

/// Parse a numeric literal exactly like `sheet_fn::coerce::to_number`'s text
/// branch (kept in sync, but `sheet-calc` cannot reach that private fn). A
/// number literal does NOT trim surrounding whitespace here (cell entry of
/// `" 1"` is text in Excel) — only an exact numeric spelling parses.
fn parse_number_literal(s: &str) -> Option<f64> {
    if s != s.trim() {
        return None;
    }
    let probe = s.strip_prefix(['+', '-']).unwrap_or(s);
    let head = probe.as_bytes().first().copied();
    let numeric_head = matches!(head, Some(b'0'..=b'9') | Some(b'.'));
    if !numeric_head {
        return None;
    }
    if s.bytes().any(|b| b == b'_') {
        return None;
    }
    s.parse::<f64>().ok().filter(|n| n.is_finite())
}

// ---- structural-edit geometry helpers ----

#[derive(Copy, Clone, PartialEq, Eq)]
enum EditAxis {
    Row,
    Col,
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
}

/// Pull the `(sheet, axis, kind, at, n)` tuple out of an [`Edit`].
fn decompose(edit: &Edit) -> (SheetId, EditAxis, EditKind, u32, u32) {
    match *edit {
        Edit::InsertRows { sheet, at, n } => (sheet, EditAxis::Row, EditKind::Insert, at, n),
        Edit::DeleteRows { sheet, at, n } => (sheet, EditAxis::Row, EditKind::Delete, at, n),
        Edit::InsertCols { sheet, at, n } => (sheet, EditAxis::Col, EditKind::Insert, at, n),
        Edit::DeleteCols { sheet, at, n } => (sheet, EditAxis::Col, EditKind::Delete, at, n),
    }
}

/// Shift one coordinate physically (the data analogue of the parser's
/// reference shift). `None` = the row/col was deleted.
fn shift_coord(c: u32, kind: EditKind, at: u32, n: u32) -> Option<u32> {
    match kind {
        EditKind::Insert => {
            if c >= at {
                let nc = c as u64 + n as u64;
                let max = sheet_core::MAX_ROW.max(sheet_core::MAX_COL) as u64;
                if nc > max {
                    None
                } else {
                    Some(nc as u32)
                }
            } else {
                Some(c)
            }
        }
        EditKind::Delete => {
            let end = at as u64 + n as u64;
            if (c as u64) < at as u64 {
                Some(c)
            } else if (c as u64) < end {
                None
            } else {
                Some((c as u64 - n as u64) as u32)
            }
        }
    }
}

/// Shift a `BTreeMap<u32, f64>` (col widths or row heights) by a structural
/// edit on its own axis. Deleted keys are dropped.
fn shift_map(
    map: std::collections::BTreeMap<u32, f64>,
    kind: EditKind,
    at: u32,
    n: u32,
) -> std::collections::BTreeMap<u32, f64> {
    let mut out = std::collections::BTreeMap::new();
    for (k, v) in map {
        if let Some(nk) = shift_coord(k, kind, at, n) {
            out.insert(nk, v);
        }
    }
    out
}

/// Shift a merge box along `axis`; `None` if it fully collapses (both
/// endpoints deleted).
fn shift_merge(
    m: sheet_core::RangeRef,
    axis: EditAxis,
    kind: EditKind,
    at: u32,
    n: u32,
) -> Option<sheet_core::RangeRef> {
    let norm = m.normalized();
    let (s, e) = match axis {
        EditAxis::Row => (norm.start.row, norm.end.row),
        EditAxis::Col => (norm.start.col, norm.end.col),
    };

    // Reuse the parser's range-shift semantics: clip to the survivor.
    let (ns, ne) = match kind {
        EditKind::Insert => match (shift_coord(s, kind, at, n), shift_coord(e, kind, at, n)) {
            (Some(a), Some(b)) => (a, b),
            _ => return None,
        },
        EditKind::Delete => {
            let span_end = at as u64 + n as u64;
            if (s as u64) >= at as u64 && (e as u64) < span_end {
                return None; // fully inside the deleted span
            }
            let ns = if (s as u64) < at as u64 {
                s
            } else if (s as u64) < span_end {
                at
            } else {
                (s as u64 - n as u64) as u32
            };
            let ne = if (e as u64) < at as u64 {
                e
            } else if (e as u64) < span_end {
                at.saturating_sub(1)
            } else {
                (e as u64 - n as u64) as u32
            };
            (ns, ne)
        }
    };

    let mut start = norm.start;
    let mut end = norm.end;
    match axis {
        EditAxis::Row => {
            start.row = ns;
            end.row = ne;
        }
        EditAxis::Col => {
            start.col = ns;
            end.col = ne;
        }
    }
    Some(sheet_core::RangeRef { start, end })
}

/// A [`ParseCtx`] over the engine's [`SheetModel`]: resolves sheet names and
/// defined names for the parser, with the entered cell's sheet as home.
struct ModelParseCtx<'a> {
    model: &'a SheetModel,
    current: SheetId,
}

impl ParseCtx for ModelParseCtx<'_> {
    fn sheet_id(&self, name: &str) -> Option<SheetId> {
        self.model.sheet_id(name)
    }
    fn name_id(&self, name: &str) -> Option<NameId> {
        self.model.names.resolve(name, self.current)
    }
    fn current_sheet(&self) -> SheetId {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        let mut m = SheetModel::new();
        m.add_sheet("Sheet1");
        Engine::new(m, EngineConfig::default())
    }

    fn cr(row: u32, col: u32) -> CellRef {
        CellRef {
            sheet: 0,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    fn value_at(e: &Engine, row: u32, col: u32) -> CellValue {
        e.model()
            .sheet(0)
            .unwrap()
            .cell(row, col)
            .map(|c| c.value.clone())
            .unwrap_or(CellValue::Empty)
    }

    #[test]
    fn enter_literal_kinds() {
        let mut e = engine();
        e.enter(0, 0, 0, "42").unwrap();
        assert_eq!(value_at(&e, 0, 0), CellValue::Number(42.0));
        e.enter(0, 1, 0, "-3.5").unwrap();
        assert_eq!(value_at(&e, 1, 0), CellValue::Number(-3.5));
        e.enter(0, 2, 0, "1e3").unwrap();
        assert_eq!(value_at(&e, 2, 0), CellValue::Number(1000.0));
        e.enter(0, 3, 0, "true").unwrap();
        assert_eq!(value_at(&e, 3, 0), CellValue::Bool(true));
        e.enter(0, 4, 0, "#REF!").unwrap();
        assert_eq!(value_at(&e, 4, 0), CellValue::Error(CellError::Ref));
        e.enter(0, 5, 0, "hello").unwrap();
        assert_eq!(value_at(&e, 5, 0), CellValue::from("hello"));
        e.enter(0, 6, 0, "").unwrap();
        assert_eq!(value_at(&e, 6, 0), CellValue::Empty);
        // " 1" (leading space) is text, not a number.
        e.enter(0, 7, 0, " 1").unwrap();
        assert_eq!(value_at(&e, 7, 0), CellValue::from(" 1"));
    }

    #[test]
    fn enter_formula_recalcs() {
        let mut e = engine();
        e.enter(0, 0, 0, "2").unwrap();
        e.enter(0, 1, 0, "3").unwrap();
        let r = e.enter(0, 2, 0, "=A1+A2").unwrap();
        assert_eq!(value_at(&e, 2, 0), CellValue::Number(5.0));
        assert!(r.changed.contains(&cr(2, 0)));
        // Edit A1 -> the formula recalcs.
        e.enter(0, 0, 0, "10").unwrap();
        assert_eq!(value_at(&e, 2, 0), CellValue::Number(13.0));
    }

    #[test]
    fn enter_bad_formula_is_err() {
        let mut e = engine();
        assert!(e.enter(0, 0, 0, "=1+").is_err());
        // Unknown function -> ParseError.
        assert!(e.enter(0, 0, 0, "=NOTAFUNC(1)").is_err());
    }

    #[test]
    fn circular_stores_ref_and_reports() {
        let mut e = engine();
        e.enter(0, 0, 0, "=B1").unwrap();
        let r = e.enter(0, 0, 1, "=A1").unwrap();
        assert!(r.circular.contains(&cr(0, 0)) || r.circular.contains(&cr(0, 1)));
        assert_eq!(value_at(&e, 0, 0), CellValue::Error(CellError::Ref));
        assert_eq!(value_at(&e, 0, 1), CellValue::Error(CellError::Ref));
        // Break the cycle.
        let r2 = e.enter(0, 0, 1, "5").unwrap();
        assert!(r2.circular.is_empty());
        assert_eq!(value_at(&e, 0, 0), CellValue::Number(5.0));
    }

    #[test]
    fn into_model_returns_model() {
        let mut e = engine();
        e.enter(0, 0, 0, "7").unwrap();
        let m = e.into_model();
        assert_eq!(
            m.sheet(0).unwrap().cell(0, 0).unwrap().value,
            CellValue::Number(7.0)
        );
    }
}
