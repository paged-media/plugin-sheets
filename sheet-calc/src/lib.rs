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

//! # sheet-calc — the calculation engine (spec §6.2/§6.3)
//!
//! The salsa-shaped core that joins the parser, the function library, and the
//! [`SheetModel`]: a dependency graph ([`graph`]), dirty propagation
//! ([`dirty`]), a deterministic topological scheduler ([`topo`]), the
//! tree-walk evaluator ([`eval`]), range-argument materialization
//! ([`argview`]), and volatile-pass support ([`volatile`]).
//!
//! External-workbook references resolve to CACHED values only ([`external`],
//! spec §13 M3; the no-network ruling §1.1) — links are NEVER followed.
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
//! codes). When the scheduler detects a cycle AND iterative calculation is OFF
//! (the default), each cycle member's STORED value becomes
//! [`CellValue::Error`]`(`[`CellError::Ref`]`)` (the `#REF!`-class display the
//! registry row's title calls for), AND the full cycle set is reported on
//! [`RecalcResult::circular`]. The glue surfaces the circular WARNING from
//! `circular`; the cell displays the `#REF!`-class error. Breaking the cycle
//! (re-entering one member as a non-cyclic formula or a literal) clears both on
//! the next recalc.
//!
//! ## Iterative calculation (registry `sheet.calc.iterative.*`, D-7)
//!
//! When iterative calculation is ON ([`Engine::set_iterative`] /
//! [`sheet_core::CalcSettings::iterative`]), a detected cycle is instead
//! evaluated to a fixed point: the members are seeded at `0` and recomputed in a
//! stable order up to `max_iter` passes, stopping early when the largest
//! per-cell change falls to/below `max_change` (see [`iterate`]). On convergence
//! [`RecalcResult::circular`] is EMPTY; a system that exhausts `max_iter` is
//! reported on [`RecalcResult::non_converged`] (its last-iterate values kept).
//! This SUPERSEDES the `#REF!` ruling for that cycle only while the flag is on.
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
pub mod external;
pub mod graph;
pub mod iterate;
pub mod spill;
pub mod topo;
pub mod volatile;

use sheet_core::ast::Formula;
use sheet_core::{Cell, CellError, CellRef, CellValue, NameId, SheetId, SheetModel};
use sheet_fn::FnResult;
use sheet_parser::{Edit, ParseCtx};

use crate::dirty::Dirty;
use crate::graph::DepGraph;
use crate::spill::{SpillRect, SpillState};

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
/// the cells found on a cycle (the `sheet.calc.circular` set), and — when
/// iterative calculation is enabled — the cycle cells that did NOT converge
/// within `max_iter` (the `sheet.calc.iterative.*` set).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecalcResult {
    /// Formula cells whose stored value changed during this recalc.
    pub changed: Vec<CellRef>,
    /// Formula cells on a circular reference stored as `#REF!` (iteration OFF).
    /// EMPTY when iterative calculation is enabled — a cycle then iterates to a
    /// fixed point (its members move to `non_converged` only if they fail to
    /// settle within `max_iter`).
    pub circular: Vec<CellRef>,
    /// Cycle cells that did NOT converge within `max_iter` while iterative
    /// calculation was enabled (their last-iterate values are kept in the
    /// model). Always EMPTY with iteration off, and empty for a cycle that
    /// converged. Sorted [`CellRef`] for determinism.
    pub non_converged: Vec<CellRef>,
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
    /// The dynamic-array spill ledger (spec §6.4, M1 spill track): anchor →
    /// owned region + every owned cell → its anchor. Off-model bookkeeping; the
    /// spilled cells themselves are plain value cells in [`Engine::model`].
    spills: SpillState,
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
            spills: SpillState::new(),
        }
    }

    /// Read-only access to the spill ledger (test/introspection seam — the
    /// `sheet-js` surface reports spill regions through this).
    pub fn spills(&self) -> &SpillState {
        &self.spills
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

    /// Toggle iterative (circular) calculation (spec §6.2, D-7; registry
    /// `sheet.calc.iterative.*`). Writes the three knobs into the workbook's
    /// [`CalcSettings`] (`iterative` / `max_iter` / `max_change`, the OOXML
    /// `<calcPr>` `iterate` / `iterateCount` / `iterateDelta`) and triggers a
    /// full recalc so cycle members switch policy immediately:
    ///
    /// - `on == false`: a cycle keeps the `sheet.calc.circular` ruling (each
    ///   member stored as `#REF!`, reported on [`RecalcResult::circular`]).
    /// - `on == true`: a cycle iterates to a fixed point (seeded at `0`,
    ///   recomputed in stable order up to `max_iter` passes, stopping early at
    ///   `max_change`). [`RecalcResult::circular`] is then empty; cells that do
    ///   not settle land on [`RecalcResult::non_converged`].
    ///
    /// `max_iter` / `max_change` are honored even when `on == false` (they are
    /// stored for the next time iteration is enabled); Excel's defaults are
    /// `100` / `0.001` ([`CalcSettings::default`]).
    ///
    /// [`CalcSettings`]: sheet_core::CalcSettings
    /// [`CalcSettings::default`]: sheet_core::CalcSettings
    pub fn set_iterative(&mut self, on: bool, max_iter: u32, max_change: f64) -> RecalcResult {
        self.model.calc.iterative = on;
        self.model.calc.max_iter = max_iter;
        self.model.calc.max_change = max_change;
        self.recalc_all()
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

        // Spill bookkeeping (spec §6.4). If this cell IS a live spill anchor,
        // release its region first (a user edit replaces the anchor entirely;
        // a clear must not leave the old spilled cells behind). If this cell is
        // an interior (non-anchor) spilled cell, dirty its owning anchor so the
        // anchor re-evaluates and surfaces `#SPILL!` for the now-blocked region.
        if self.spills.region_of(cref).is_some() {
            let mut sink: Vec<CellRef> = Vec::new();
            self.clear_spill_region(cref, &mut sink);
        } else if let Some(anchor) = self.spills.owner_of(cref) {
            self.dirty.mark(anchor);
            self.dirty.propagate_from(anchor, &self.graph);
        }

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

        // Any blocked spill anchor re-evaluates: removing/altering a cell may
        // free its rectangle so it can finally spill (`sheet.calc.spill.collision`).
        for blocked in self.spills.blocked_sorted() {
            if self.graph.is_formula(blocked) {
                self.dirty.mark(blocked);
            } else {
                // No longer a formula — drop it from the blocked set.
                self.spills.unmark_blocked(blocked);
            }
        }

        self.recalc_dirty()
    }

    /// Recalculate EVERY formula cell (marks all dirty, then runs a pass).
    pub fn recalc_all(&mut self) -> RecalcResult {
        self.dirty.mark_all(&self.graph);
        self.recalc_dirty()
    }

    /// Recalculate the current dirty cut (volatiles reseeded). The heart of
    /// the engine.
    ///
    /// ## Spill re-passes (spec §6.4)
    ///
    /// Materializing a spill writes engine-owned cells into the model and dirties
    /// their dependents (a formula reading the spill range/`A1#` must reflow).
    /// Those dependents were not in the pass's initial topo order, so after the
    /// main drain we re-run while new cells remain dirty — a bounded fixpoint
    /// (capped so a pathological spill chain cannot loop forever). Cycle
    /// reporting from the FIRST pass is the authoritative `circular` set.
    pub fn recalc_dirty(&mut self) -> RecalcResult {
        let mut changed: Vec<CellRef> = Vec::new();
        let mut circular: Vec<CellRef> = Vec::new();
        let mut non_converged: Vec<CellRef> = Vec::new();

        // Volatile cells reseed ONCE per recalc (not per spill sub-pass — that
        // would keep the fixpoint alive forever whenever any volatile exists).
        self.dirty.reseed_volatile();

        // Bounded fixpoint: each iteration drains the current dirty cut; spill
        // materialization writes engine-owned cells and dirties their
        // dependents, which the next iteration drains. Capped so a pathological
        // spill chain cannot loop forever.
        const MAX_PASSES: u32 = 64;
        for pass_n in 0..MAX_PASSES {
            let cut = self.dirty.drain_set();
            if cut.is_empty() {
                break;
            }
            self.pass = self.pass.wrapping_add(1);

            let to = topo::order(&cut, &self.graph);

            // Evaluate the orderable cells in dependency order; each write is
            // immediately visible to later cells (topo guarantees freshness).
            // A cell whose formula ROOT spills (a `returns_array` call or an
            // array literal) takes the spill path: clear its prior region,
            // evaluate rich, materialize the 2-D block (or `#SPILL!` on
            // collision). Every other cell stays on the scalar path.
            for cref in &to.order {
                if self.cell_spills(*cref) {
                    self.recompute_spill_anchor(*cref, &mut changed);
                } else {
                    // A non-spilling formula that USED to be a spill anchor must
                    // release its old region (e.g. edited from SEQUENCE to a
                    // scalar formula).
                    self.clear_spill_region(*cref, &mut changed);
                    let new_value = self.evaluate_cell(*cref);
                    if self.commit_value(*cref, new_value) {
                        changed.push(*cref);
                    }
                }
            }

            // Cycle members. Two policies (registry `sheet.calc.iterative.*`,
            // D-7): with iteration OFF (default) store #REF! (the wire encoding
            // of the Circular diagnostic) and report on `circular`; with
            // iteration ON, drive the convergence loop to a fixed point and
            // report only the non-converged remainder. The first pass's cycle
            // set is the authoritative one (later passes only chase spill reflow).
            if self.model.calc.iterative {
                let (nc_changed, nc) = self.iterate_cycle(&to.cycle);
                changed.extend(nc_changed);
                if pass_n == 0 {
                    non_converged = nc;
                }
            } else {
                for cref in &to.cycle {
                    let ref_err = CellValue::Error(CellError::Ref);
                    if self.commit_value(*cref, ref_err) {
                        changed.push(*cref);
                    }
                }
                if pass_n == 0 {
                    circular = to.cycle;
                }
            }
        }

        self.dirty.clear();

        // Dedup + stabilize the changed set (a cell may be touched across
        // passes — clear then refill, or anchor then reflow).
        changed.sort();
        changed.dedup();
        RecalcResult {
            changed,
            circular,
            non_converged,
        }
    }

    /// Apply a structural edit (insert/delete rows or cols): physically shift
    /// the cells, column widths, row heights, and merges; rewrite every
    /// formula's references (`sheet_parser::rewrite`, `#REF!` for deleted
    /// spans); rebuild the dependency graph; then recalc everything.
    pub fn apply_edit(&mut self, edit: &Edit) -> RecalcResult {
        // Spill ledger (spec §6.4): clear every live spill region from the model
        // BEFORE the structural shift, so no stale engine-owned spilled cells get
        // physically moved (they would become spurious blockers). The anchors are
        // formulas — they survive the edit and re-materialize on the recalc below.
        let mut sink: Vec<CellRef> = Vec::new();
        for anchor in self.spills.anchors_sorted() {
            self.clear_spill_region(anchor, &mut sink);
        }
        self.spills.clear();

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
        eval::eval_expr(&self.model, &f.root.clone(), &ctx, &self.spills)
    }

    /// Drive iterative (circular) calculation over one cycle (spec §6.2, D-7;
    /// registry `sheet.calc.iterative.*`). `members` is the cycle set from
    /// [`topo::order`]; it is sorted here so the evaluation order — and thus the
    /// converged values — are reproducible run-to-run (the §6.2 determinism
    /// property).
    ///
    /// Algorithm (Excel-style): seed every member at `0`, then for up to
    /// `model.calc.max_iter` passes recompute each member in sorted order,
    /// committing immediately (Gauss–Seidel — a later member in the same pass
    /// reads the freshly written value of an earlier one). Stop early when the
    /// largest per-cell change ([`iterate::cell_delta`]) across the whole cycle
    /// is `<= model.calc.max_change`.
    ///
    /// Returns `(changed, non_converged)`: the members whose stored value
    /// differs from before the loop, and (empty on convergence) the members
    /// still in the cycle if `max_iter` ran out — their last-iterate values are
    /// kept in the model (matching Excel, which leaves the partial result).
    fn iterate_cycle(&mut self, members: &[CellRef]) -> (Vec<CellRef>, Vec<CellRef>) {
        let mut ordered: Vec<CellRef> = members.to_vec();
        ordered.sort();

        let max_iter = self.model.calc.max_iter;
        let max_change = self.model.calc.max_change;

        // Snapshot the pre-iteration stored values so we can report which cells
        // actually changed (a cell already at its fixed point may not move).
        let before: Vec<CellValue> = ordered.iter().map(|c| self.stored_value(*c)).collect();

        // Seed every cycle cell at 0 (Excel's documented initial value) so the
        // first pass reads a defined precedent rather than the stale value
        // (#REF!, or a prior unrelated result).
        for &m in &ordered {
            self.commit_value(m, CellValue::Number(0.0));
        }

        let mut converged = ordered.is_empty();
        for _ in 0..max_iter {
            let mut max_delta = 0.0_f64;
            for &m in &ordered {
                let old = self.stored_value(m);
                let new = self.evaluate_cell(m);
                let delta = iterate::cell_delta(&old, &new);
                if delta > max_delta {
                    max_delta = delta;
                }
                self.commit_value(m, new);
            }
            if max_delta <= max_change {
                converged = true;
                break;
            }
        }

        // Report changes against the PRE-iteration snapshot.
        let mut changed: Vec<CellRef> = Vec::new();
        for (m, prev) in ordered.iter().zip(before.iter()) {
            if self.stored_value(*m) != *prev {
                changed.push(*m);
            }
        }

        let non_converged = if converged { Vec::new() } else { ordered };
        (changed, non_converged)
    }

    /// The currently stored value of a cell (defaulting to `Empty`), used by the
    /// iterative loop to snapshot precedents and measure convergence deltas.
    fn stored_value(&self, cref: CellRef) -> CellValue {
        self.model
            .sheet(cref.sheet)
            .and_then(|ws| ws.cell(cref.row, cref.col))
            .map(|c| c.value.clone())
            .unwrap_or(CellValue::Empty)
    }

    /// Whether the formula at `cref` has a *spilling* root (a `returns_array`
    /// function call or an array literal — the T1 ruling
    /// `sheet.calc.spill.materialize`). A literal/non-formula cell never spills.
    fn cell_spills(&self, cref: CellRef) -> bool {
        let Some(ws) = self.model.sheet(cref.sheet) else {
            return false;
        };
        let Some(cell) = ws.cell(cref.row, cref.col) else {
            return false;
        };
        let Some(fid) = cell.formula else {
            return false;
        };
        self.model
            .formula(fid)
            .map(|f| eval::expr_spills(&f.root))
            .unwrap_or(false)
    }

    /// Recompute a spill anchor (spec §6.4): clear its prior region, evaluate
    /// the formula through the rich (array) door, then materialize the result.
    /// A `FnResult::Scalar` (the kernel grounded to an error, or a 1×1 array)
    /// commits a single value; a `FnResult::Array` materializes the rectangle
    /// (or stores `#SPILL!` and claims no region on a collision). Cells whose
    /// stored value changes are pushed onto `changed`.
    fn recompute_spill_anchor(&mut self, anchor: CellRef, changed: &mut Vec<CellRef>) {
        // 1. Release the prior region so a shrinking array leaves no stale cells.
        self.clear_spill_region(anchor, changed);

        // 2. Evaluate through the rich door.
        let result = self.evaluate_cell_rich(anchor);
        match result {
            FnResult::Scalar(v) => {
                if self.commit_value(anchor, v) {
                    changed.push(anchor);
                }
            }
            FnResult::Array(grid) => {
                self.materialize_spill(anchor, grid, changed);
            }
        }
    }

    /// Evaluate a formula cell through the rich (array) door, returning the
    /// [`FnResult`]. Mirrors [`Engine::evaluate_cell`] but keeps array blocks.
    fn evaluate_cell_rich(&self, cref: CellRef) -> FnResult {
        let Some(ws) = self.model.sheet(cref.sheet) else {
            return FnResult::Scalar(CellValue::Empty);
        };
        let Some(cell) = ws.cell(cref.row, cref.col) else {
            return FnResult::Scalar(CellValue::Empty);
        };
        let Some(fid) = cell.formula else {
            return FnResult::Scalar(cell.value.clone());
        };
        let Some(f) = self.model.formula(fid) else {
            return FnResult::Scalar(CellValue::Error(CellError::Ref));
        };
        let seed = volatile::cell_seed(self.config.rng_seed, self.pass, cref);
        let ctx = eval::ctx_for(&self.model, cref, self.config.now_serial, seed);
        eval::eval_expr_rich(&self.model, &f.root.clone(), &ctx, &self.spills)
    }

    /// Materialize a `rows × cols` array block anchored at `anchor` (spec §6.4).
    /// Tests the target rectangle for a collision first: every non-anchor cell
    /// must be blank OR already owned by THIS anchor. A blocker → store
    /// `#SPILL!` at the anchor and claim no region (`sheet.calc.spill.collision`).
    /// Otherwise write the top-left into the anchor and the rest as engine-owned
    /// spilled value cells, and record the region in the ledger.
    fn materialize_spill(
        &mut self,
        anchor: CellRef,
        grid: Vec<Vec<CellValue>>,
        changed: &mut Vec<CellRef>,
    ) {
        let rows = grid.len() as u32;
        let cols = grid.first().map(|r| r.len()).unwrap_or(0) as u32;
        // A degenerate (empty) grid should not arise — kernels ground to a
        // scalar error — but stay total: treat it as a single blank.
        if rows == 0 || cols == 0 {
            if self.commit_value(anchor, CellValue::Empty) {
                changed.push(anchor);
            }
            return;
        }
        let rect = SpillRect::for_array(anchor, rows, cols);

        // Collision test: every cell of the rect EXCEPT the anchor must be free
        // (blank or already owned by this anchor — the latter cannot happen now
        // since we cleared the region, but stays defensive).
        for c in rect.cells() {
            if c == anchor {
                continue;
            }
            if self.cell_is_blocker(c, anchor) {
                // Blocked — store #SPILL! at the anchor, claim no region, and
                // remember it so removing the blocker re-triggers the spill.
                self.spills.mark_blocked(anchor);
                if self.commit_value(anchor, CellValue::Error(CellError::Spill)) {
                    changed.push(anchor);
                }
                return;
            }
        }

        // Free — this anchor is no longer blocked.
        self.spills.unmark_blocked(anchor);

        // Free — write the block. The anchor keeps its formula; the spilled
        // cells are plain value cells (engine-owned, recorded in the ledger).
        for (r, row) in grid.into_iter().enumerate() {
            for (c, value) in row.into_iter().enumerate() {
                let cell = CellRef {
                    sheet: anchor.sheet,
                    row: anchor.row + r as u32,
                    col: anchor.col + c as u32,
                    row_abs: false,
                    col_abs: false,
                };
                if cell == anchor {
                    if self.commit_value(anchor, value) {
                        changed.push(anchor);
                    }
                } else {
                    self.write_spilled_cell(cell, value, changed);
                }
            }
        }

        // Record the region and dirty any dependents of the freshly written
        // spilled cells (a formula reading the spill range reflows).
        self.spills.insert(rect);
        for c in rect.cells() {
            self.dirty.propagate_from(c, &self.graph);
        }
    }

    /// Whether `cell` blocks a spill anchored at `anchor`: it holds a non-empty
    /// value or a formula AND is not already owned by `anchor`. A blank cell, or
    /// a cell this anchor owns, is not a blocker.
    fn cell_is_blocker(&self, cell: CellRef, anchor: CellRef) -> bool {
        if self.spills.owner_of(cell) == Some(anchor) {
            return false;
        }
        match self
            .model
            .sheet(cell.sheet)
            .and_then(|ws| ws.cell(cell.row, cell.col))
        {
            Some(c) => c.formula.is_some() || c.value != CellValue::Empty,
            None => false,
        }
    }

    /// Write an engine-owned spilled value cell (no formula, preserving any
    /// existing style). Pushes onto `changed` if the stored value changed.
    fn write_spilled_cell(&mut self, cell: CellRef, value: CellValue, changed: &mut Vec<CellRef>) {
        let style = self.style_of(cell);
        let Some(ws) = self.model.sheet_mut(cell.sheet) else {
            return;
        };
        let prior = ws.cell(cell.row, cell.col).map(|c| c.value.clone());
        if prior.as_ref() == Some(&value) {
            return;
        }
        ws.set_cell(
            cell.row,
            cell.col,
            Cell {
                value,
                formula: None,
                style,
            },
        );
        changed.push(cell);
    }

    /// Clear `anchor`'s spill region from the model + ledger (if it owns one):
    /// every spilled NON-anchor cell becomes blank, and dependents of those
    /// cleared cells are dirtied so they recompute. The anchor cell itself is
    /// left for the recompute to overwrite. No-op if `anchor` owns no region.
    fn clear_spill_region(&mut self, anchor: CellRef, changed: &mut Vec<CellRef>) {
        let Some(rect) = self.spills.remove(anchor) else {
            return;
        };
        for c in rect.cells() {
            if c == anchor {
                continue;
            }
            if let Some(ws) = self.model.sheet_mut(c.sheet) {
                if ws.cell(c.row, c.col).is_some() {
                    ws.remove_cell(c.row, c.col);
                    changed.push(c);
                }
            }
            // Anything that read the now-blank cell must recompute.
            self.dirty.propagate_from(c, &self.graph);
        }
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

    fn approx(v: CellValue, target: f64, tol: f64) {
        match v {
            CellValue::Number(n) => assert!(
                (n - target).abs() <= tol,
                "expected ~{target}, got {n} (tol {tol})"
            ),
            other => panic!("expected a number ~{target}, got {other:?}"),
        }
    }

    #[test]
    fn iterative_convergent_system_settles() {
        // A1 = B1*0.5 + 10 ; B1 = A1 + 5. Fixed point: A1 = 0.5*(A1+5)+10 ->
        // 0.5*A1 = 12.5 -> A1 = 25, B1 = 30.
        let mut e = engine();
        e.set_iterative(true, 1000, 1e-9);
        e.enter(0, 0, 0, "=B1*0.5+10").unwrap();
        let r = e.enter(0, 0, 1, "=A1+5").unwrap();
        // Converged: no #REF!, circular empty, non_converged empty.
        assert!(r.circular.is_empty());
        assert!(r.non_converged.is_empty());
        approx(value_at(&e, 0, 0), 25.0, 1e-6);
        approx(value_at(&e, 0, 1), 30.0, 1e-6);
    }

    #[test]
    fn iterative_off_yields_ref() {
        // Same system, iteration OFF (default): #REF! + circular reported.
        let mut e = engine();
        e.enter(0, 0, 0, "=B1*0.5+10").unwrap();
        let r = e.enter(0, 0, 1, "=A1+5").unwrap();
        assert!(!r.circular.is_empty());
        assert!(r.non_converged.is_empty());
        assert_eq!(value_at(&e, 0, 0), CellValue::Error(CellError::Ref));
        assert_eq!(value_at(&e, 0, 1), CellValue::Error(CellError::Ref));
    }

    #[test]
    fn iterative_divergent_caps_at_max_iter() {
        // A1 = A1 + 1 diverges; it is reported non-converged and capped.
        let mut e = engine();
        e.set_iterative(true, 7, 1e-9);
        let r = e.enter(0, 0, 0, "=A1+1").unwrap();
        assert!(r.circular.is_empty());
        assert_eq!(r.non_converged, vec![cr(0, 0)]);
        // Seeded at 0, +1 per pass, 7 passes -> 7.
        approx(value_at(&e, 0, 0), 7.0, 0.0);
    }

    #[test]
    fn iterative_is_deterministic_across_runs() {
        let run = || {
            let mut e = engine();
            e.set_iterative(true, 1000, 1e-12);
            e.enter(0, 0, 0, "=B1*0.5+10").unwrap();
            e.enter(0, 0, 1, "=A1+5").unwrap();
            (value_at(&e, 0, 0), value_at(&e, 0, 1))
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn set_iterative_toggles_policy_live() {
        // Enter a cycle with iteration off -> #REF!, then turn iteration on and
        // the SAME cells settle (set_iterative recalcs).
        let mut e = engine();
        e.enter(0, 0, 0, "=B1*0.5+10").unwrap();
        e.enter(0, 0, 1, "=A1+5").unwrap();
        assert_eq!(value_at(&e, 0, 0), CellValue::Error(CellError::Ref));
        let r = e.set_iterative(true, 1000, 1e-9);
        assert!(r.circular.is_empty());
        assert!(r.non_converged.is_empty());
        approx(value_at(&e, 0, 0), 25.0, 1e-6);
        // And back off -> #REF! again.
        e.set_iterative(false, 100, 0.001);
        assert_eq!(value_at(&e, 0, 0), CellValue::Error(CellError::Ref));
    }
}
