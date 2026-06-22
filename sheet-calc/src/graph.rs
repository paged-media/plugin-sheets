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

//! The dependency graph (spec §6.2). Each formula cell is a node; its
//! out-edges (the cells/ranges it READS) come from
//! [`sheet_parser::extract_refs`] over its interned AST. We store the
//! **reverse** map too (who reads me), because dirty propagation and topo
//! recalc both walk dependents.
//!
//! ## Range dependencies — single normalized [`RangeKey`] nodes (T0)
//!
//! A range reference is NOT exploded per-cell (a `A1:A1000000` edge set would
//! be ruinous). Instead each distinct normalized range box is a [`RangeKey`]
//! node; a formula that reads a range gets an edge to that key, and a write at
//! `(sheet, row, col)` dirties every registered range key whose box CONTAINS
//! it (a linear scan over the registered keys — the documented M1
//! interval-index upgrade seam, spec §6.2). A range key also fans out to the
//! formula cells that depend on it.
//!
//! ## Name dependencies
//!
//! A `NameTarget::Range` name registers its range box exactly like a literal
//! range (so a write inside it dirties the dependents). A `NameTarget::Formula`
//! name yields `#NAME?` at eval time (T1) — it contributes no edges here.

use rustc_hash::{FxHashMap, FxHashSet};
use sheet_core::names::NameTarget;
use sheet_core::{CellRef, RangeRef, SheetId, SheetModel};

/// A normalized range box used as a single dependency node. Keyed by the
/// normalized corners (absolute flags stripped) so `A1:B2` and `$A$1:$B$2`
/// share one node.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct RangeKey {
    pub sheet: SheetId,
    pub row0: u32,
    pub col0: u32,
    pub row1: u32,
    pub col1: u32,
}

impl RangeKey {
    fn from_range(r: RangeRef) -> RangeKey {
        let n = r.normalized();
        RangeKey {
            sheet: n.start.sheet,
            row0: n.start.row,
            col0: n.start.col,
            row1: n.end.row,
            col1: n.end.col,
        }
    }

    /// True if `(sheet, row, col)` falls inside this box.
    pub fn contains(&self, sheet: SheetId, row: u32, col: u32) -> bool {
        sheet == self.sheet
            && row >= self.row0
            && row <= self.row1
            && col >= self.col0
            && col <= self.col1
    }
}

/// The dependency graph over the workbook's formula cells.
#[derive(Default)]
pub struct DepGraph {
    /// Reverse cell edges: a single-cell `dep` → the formula cells that read
    /// it directly. Walked by dirty propagation and topo recalc.
    cell_dependents: FxHashMap<CellRef, FxHashSet<CellRef>>,
    /// Reverse range edges: a `RangeKey` → the formula cells that read that
    /// range. A range key is "dirtied" when a write lands inside its box.
    range_dependents: FxHashMap<RangeKey, FxHashSet<CellRef>>,
    /// Forward edges (a formula cell → the single-cell deps it reads), so a
    /// re-register can drop the old edges precisely.
    cell_deps_of: FxHashMap<CellRef, Vec<CellRef>>,
    /// Forward range edges (a formula cell → the range keys it reads).
    range_deps_of: FxHashMap<CellRef, Vec<RangeKey>>,
    /// The set of registered formula cells (nodes). Iteration order is made
    /// deterministic by callers that sort `CellRef`.
    formula_cells: FxHashSet<CellRef>,
}

impl DepGraph {
    pub fn new() -> Self {
        DepGraph::default()
    }

    /// All registered formula cells, as a sorted Vec (deterministic order —
    /// callers rely on it for stable recalc-all / cycle reporting).
    pub fn formula_cells_sorted(&self) -> Vec<CellRef> {
        let mut v: Vec<CellRef> = self.formula_cells.iter().copied().collect();
        v.sort();
        v
    }

    pub fn is_formula(&self, cell: CellRef) -> bool {
        self.formula_cells.contains(&cell)
    }

    /// Register (or re-register) a formula cell's dependencies. Drops any
    /// previous edges for `cell` first, then installs edges from the formula's
    /// [`sheet_parser::extract_refs`] result and the model's name table (to
    /// resolve `NameTarget::Range` names to their boxes).
    pub fn register(&mut self, cell: CellRef, refs: &sheet_parser::RefSet, model: &SheetModel) {
        self.unregister(cell);
        self.formula_cells.insert(cell);

        let mut cell_deps: Vec<CellRef> = Vec::new();
        let mut range_keys: Vec<RangeKey> = Vec::new();

        for dep in &refs.cells {
            // Store deps with absolute flags stripped so they key uniformly.
            let key = strip(*dep);
            cell_deps.push(key);
            self.cell_dependents.entry(key).or_default().insert(cell);
        }
        for r in &refs.ranges {
            let key = RangeKey::from_range(*r);
            range_keys.push(key);
            self.range_dependents.entry(key).or_default().insert(cell);
        }
        // Name deps: a range-targeted name registers its box.
        for nid in &refs.names {
            if let Some(def) = model.names.get(*nid) {
                if let NameTarget::Range(r) = &def.target {
                    let key = RangeKey::from_range(*r);
                    range_keys.push(key);
                    self.range_dependents.entry(key).or_default().insert(cell);
                }
                // NameTarget::Formula contributes no edge (T1, eval -> #NAME?).
            }
        }
        // Structured-reference (table) deps: a `Table1[Col]` reads its table by
        // NAME (spec §6.4). We resolve the name to the table's FULL extent box
        // and register that as a range key — so a write ANYWHERE inside the
        // table (a data edit, or extending the header/totals rows) dirties the
        // structured-ref dependents (the table-track dep edge, registered HERE
        // rather than in `extract.rs`, which has no model to resolve against).
        // The whole-extent box is a deliberate over-approximation (cheap and
        // correct: a structured ref's resolved area is always a sub-box of it).
        for name in &refs.tables {
            if let Some((_sid, t)) = model.resolve_table(name) {
                let key = RangeKey::from_range(t.range);
                range_keys.push(key);
                self.range_dependents.entry(key).or_default().insert(cell);
            }
        }
        // A bare in-table structured ref (`[@Col]`, empty table name) anchors to
        // the table whose ROW span includes the formula's own cell — resolved
        // HERE (the graph has both the cell and the model). Row-span matching
        // (not full containment) mirrors `eval::table_containing`, so a helper
        // column just OUTSIDE the table's columns but row-aligned still depends
        // on the table. Register its box so a write inside the table reflows the
        // in-table formula.
        if refs.has_self_table_ref {
            if let Some(ws) = model.sheet(cell.sheet) {
                if let Some(t) = ws.tables.iter().find(|t| {
                    let n = t.range.normalized();
                    n.start.sheet == cell.sheet && cell.row >= n.start.row && cell.row <= n.end.row
                }) {
                    let key = RangeKey::from_range(t.range);
                    range_keys.push(key);
                    self.range_dependents.entry(key).or_default().insert(cell);
                }
            }
        }

        if !cell_deps.is_empty() {
            self.cell_deps_of.insert(cell, cell_deps);
        }
        if !range_keys.is_empty() {
            self.range_deps_of.insert(cell, range_keys);
        }
    }

    /// Drop a formula cell entirely from the graph (it is no longer a formula,
    /// or it is about to be re-registered). Removes both its node membership
    /// and every edge mentioning it.
    pub fn unregister(&mut self, cell: CellRef) {
        self.formula_cells.remove(&cell);
        if let Some(deps) = self.cell_deps_of.remove(&cell) {
            for d in deps {
                if let Some(set) = self.cell_dependents.get_mut(&d) {
                    set.remove(&cell);
                    if set.is_empty() {
                        self.cell_dependents.remove(&d);
                    }
                }
            }
        }
        if let Some(keys) = self.range_deps_of.remove(&cell) {
            for k in keys {
                if let Some(set) = self.range_dependents.get_mut(&k) {
                    set.remove(&cell);
                    if set.is_empty() {
                        self.range_dependents.remove(&k);
                    }
                }
            }
        }
    }

    /// Direct dependents of a single-cell write at `cell`: every formula that
    /// reads `cell` directly, PLUS every formula that reads a range whose box
    /// contains `cell` (the bounding-box invalidation, linear scan over range
    /// keys — the M1 interval-index seam).
    pub fn dependents_of(&self, cell: CellRef) -> Vec<CellRef> {
        let key = strip(cell);
        let mut out: FxHashSet<CellRef> = FxHashSet::default();
        if let Some(set) = self.cell_dependents.get(&key) {
            out.extend(set.iter().copied());
        }
        for (rk, deps) in &self.range_dependents {
            if rk.contains(key.sheet, key.row, key.col) {
                out.extend(deps.iter().copied());
            }
        }
        let mut v: Vec<CellRef> = out.into_iter().collect();
        v.sort();
        v
    }

    /// The direct cell + range dependencies a formula cell reads, as
    /// single-cell `CellRef`s already registered as formula nodes (used by the
    /// topo sort to compute in-degree over the dirty subgraph). Range deps
    /// expand to the formula cells INSIDE the box that are themselves nodes.
    pub fn precedents_in<'a>(
        &'a self,
        cell: CellRef,
        candidate: &'a FxHashSet<CellRef>,
    ) -> Vec<CellRef> {
        let mut out: FxHashSet<CellRef> = FxHashSet::default();
        if let Some(deps) = self.cell_deps_of.get(&cell) {
            for d in deps {
                if candidate.contains(d) {
                    out.insert(*d);
                }
            }
        }
        if let Some(keys) = self.range_deps_of.get(&cell) {
            for k in keys {
                // Any candidate formula cell inside this range box is a
                // precedent (intra-dirty-cut edge).
                for cand in candidate {
                    if k.contains(cand.sheet, cand.row, cand.col) {
                        out.insert(*cand);
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// Rebuild the whole graph from scratch: register every cell in `model`
    /// that carries a `FormulaId`. Used after a structural edit (apply_edit)
    /// or on `Engine::new`.
    pub fn rebuild(&mut self, model: &SheetModel) {
        *self = DepGraph::new();
        for (sheet_idx, ws) in model.sheets.iter().enumerate() {
            let sheet = sheet_idx as SheetId;
            for (&(row, col), cell) in ws.iter_cells() {
                if let Some(fid) = cell.formula {
                    if let Some(f) = model.formula(fid) {
                        let refs = sheet_parser::extract_refs(f);
                        let cref = CellRef {
                            sheet,
                            row,
                            col,
                            row_abs: false,
                            col_abs: false,
                        };
                        self.register(cref, &refs, model);
                    }
                }
            }
        }
    }
}

/// Canonicalize a `CellRef` for graph keys: strip absolute (`$`) flags so the
/// same physical cell keys identically regardless of how it was written.
fn strip(c: CellRef) -> CellRef {
    CellRef {
        sheet: c.sheet,
        row: c.row,
        col: c.col,
        row_abs: false,
        col_abs: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_parser::RefSet;

    fn cr(sheet: u16, row: u32, col: u32) -> CellRef {
        CellRef {
            sheet,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    fn refs_cells(cells: &[CellRef]) -> RefSet {
        RefSet {
            cells: cells.to_vec(),
            ..Default::default()
        }
    }

    fn refs_range(r: RangeRef) -> RefSet {
        RefSet {
            ranges: vec![r.normalized()],
            ..Default::default()
        }
    }

    #[test]
    fn cell_edge_dependents() {
        let m = SheetModel::new();
        let mut g = DepGraph::new();
        // B1 = A1.
        g.register(cr(0, 0, 1), &refs_cells(&[cr(0, 0, 0)]), &m);
        assert_eq!(g.dependents_of(cr(0, 0, 0)), vec![cr(0, 0, 1)]);
        // Absolute flags do not change the keying.
        let abs = CellRef {
            row_abs: true,
            col_abs: true,
            ..cr(0, 0, 0)
        };
        assert_eq!(g.dependents_of(abs), vec![cr(0, 0, 1)]);
    }

    #[test]
    fn range_box_invalidation() {
        let m = SheetModel::new();
        let mut g = DepGraph::new();
        // C1 = SUM(A1:A3).
        let range = RangeRef {
            start: cr(0, 0, 0),
            end: cr(0, 2, 0),
        };
        g.register(cr(0, 0, 2), &refs_range(range), &m);
        // A write inside the box dirties C1.
        assert_eq!(g.dependents_of(cr(0, 1, 0)), vec![cr(0, 0, 2)]);
        // A write outside does not.
        assert!(g.dependents_of(cr(0, 3, 0)).is_empty());
    }

    #[test]
    fn structured_ref_registers_table_box_dep() {
        // A formula that reads a structured ref (RefSet carries the table NAME)
        // gets a range edge to the table's full extent, so a write inside the
        // table dirties the dependent (the tables-track dep edge).
        let mut m = SheetModel::new();
        m.add_sheet("Sheet1");
        let table = sheet_core::Table {
            name: "Sales".into(),
            range: RangeRef {
                start: cr(0, 0, 0),
                end: cr(0, 3, 2),
            }, // A1:C4
            columns: vec!["Region".into(), "Units".into(), "Total".into()],
            header_row: true,
            totals_row: false,
            style_name: None,
        };
        m.sheet_mut(0).unwrap().tables.push(table);

        let mut g = DepGraph::new();
        // E1 = SUM(Sales[Units]) — the RefSet records the table by name.
        let mut refs = RefSet::default();
        refs.tables.push("Sales".into());
        g.register(cr(0, 0, 4), &refs, &m);
        // A write inside the table extent dirties E1.
        assert_eq!(g.dependents_of(cr(0, 2, 1)), vec![cr(0, 0, 4)]);
        // A write outside the table extent does not.
        assert!(g.dependents_of(cr(0, 9, 9)).is_empty());
        // An unknown table name registers no edge (no panic, no dep).
        let mut g2 = DepGraph::new();
        let mut refs2 = RefSet::default();
        refs2.tables.push("Nope".into());
        g2.register(cr(0, 0, 4), &refs2, &m);
        assert!(g2.dependents_of(cr(0, 2, 1)).is_empty());
    }

    #[test]
    fn unregister_drops_edges() {
        let m = SheetModel::new();
        let mut g = DepGraph::new();
        g.register(cr(0, 0, 1), &refs_cells(&[cr(0, 0, 0)]), &m);
        g.unregister(cr(0, 0, 1));
        assert!(g.dependents_of(cr(0, 0, 0)).is_empty());
        assert!(!g.is_formula(cr(0, 0, 1)));
    }

    #[test]
    fn reregister_replaces_edges() {
        let m = SheetModel::new();
        let mut g = DepGraph::new();
        g.register(cr(0, 0, 1), &refs_cells(&[cr(0, 0, 0)]), &m);
        // Re-register B1 to read A2 instead of A1.
        g.register(cr(0, 0, 1), &refs_cells(&[cr(0, 1, 0)]), &m);
        assert!(g.dependents_of(cr(0, 0, 0)).is_empty());
        assert_eq!(g.dependents_of(cr(0, 1, 0)), vec![cr(0, 0, 1)]);
    }
}
