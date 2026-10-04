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
//! it. A range key also fans out to the formula cells that depend on it.
//!
//! ## The interval index (the M1 seam, spec §6.2)
//!
//! "Which boxes contain this cell" is answered by a [`RangeIndex`], not by a
//! scan over every registered box (before 2026-10 it was a scan: a running
//! total of n `SUM($A$1:A{i})` formulas paid n box tests per visited cell,
//! ~n² per edit). The index is a segment tree over rows, one per column
//! (exact) plus one per sheet for wide boxes (column-filtered): a stab walks
//! one root-to-leaf path and meets every containing box exactly once, so a
//! probe costs `O(log rows + hits)`. See [`RangeIndex`].
//!
//! The reverse question the topo sort asks — "which dirty formula cells lie
//! inside this box" — is answered by a [`Candidates`] index over the dirty
//! cut (column-major ordered set), so a box seeks its columns instead of
//! testing every dirty cell.
//!
//! ## Name dependencies
//!
//! A `NameTarget::Range` name registers its range box exactly like a literal
//! range (so a write inside it dirties the dependents). A `NameTarget::Formula`
//! name yields `#NAME?` at eval time (T1) — it contributes no edges here.

use std::collections::BTreeSet;

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
    /// Stabbing index over the keys of `range_dependents` (a key is in the
    /// index exactly while it has at least one dependent).
    index: RangeIndex,
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
    /// Add the reverse edge `key → cell`, indexing `key` when it is new.
    fn add_range_edge(&mut self, key: RangeKey, cell: CellRef) {
        let set = self.range_dependents.entry(key).or_default();
        if set.is_empty() {
            self.index.insert(key);
        }
        set.insert(cell);
    }

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
            self.add_range_edge(key, cell);
        }
        // Name deps: a range-targeted name registers its box.
        for nid in &refs.names {
            if let Some(def) = model.names.get(*nid) {
                if let NameTarget::Range(r) = &def.target {
                    let key = RangeKey::from_range(*r);
                    range_keys.push(key);
                    self.add_range_edge(key, cell);
                }
                // A Formula target's own refs arrive merged into `refs`
                // (`names::refs_with_names`).
            }
        }
        // Structured-reference (table) deps (spec §6.4): each ref is resolved
        // HERE (the graph has the cell and the model) to the EXACT area it
        // reads from this cell — `[@Qty]` in a calculated column is one cell
        // of its own row. Before 2026-10 every ref registered the table's
        // whole extent, which made each calculated column a cycle (#REF!).
        // A table changing shape is a structural edit, which rebuilds the
        // graph. A ref that does not resolve keeps the whole-extent box.
        let mut precise: Vec<&str> = Vec::new();
        let mut self_precise = refs.has_self_table_ref;
        let mut imprecise: Vec<&str> = Vec::new();
        for s in &refs.structured {
            match crate::eval::resolve_structured_at(model, s, cell) {
                Ok(r) => {
                    let key = RangeKey::from_range(r);
                    range_keys.push(key);
                    self.add_range_edge(key, cell);
                    precise.push(s.table.as_str());
                }
                Err(_) => {
                    imprecise.push(s.table.as_str());
                    if s.table.is_empty() {
                        self_precise = false;
                    }
                }
            }
        }
        for name in &refs.tables {
            if precise.iter().any(|p| p.eq_ignore_ascii_case(name))
                && !imprecise.iter().any(|p| p.eq_ignore_ascii_case(name))
            {
                continue;
            }
            if let Some((_sid, t)) = model.resolve_table(name) {
                let key = RangeKey::from_range(t.range);
                range_keys.push(key);
                self.add_range_edge(key, cell);
            }
        }
        // A bare in-table structured ref (`[@Col]`, empty table name) anchors to
        // the table whose ROW span includes the formula's own cell — resolved
        // HERE (the graph has both the cell and the model). Row-span matching
        // (not full containment) mirrors `eval::table_containing`, so a helper
        // column just OUTSIDE the table's columns but row-aligned still depends
        // on the table. Register its box so a write inside the table reflows the
        // in-table formula.
        if refs.has_self_table_ref && !(self_precise && precise.contains(&"")) {
            if let Some(ws) = model.sheet(cell.sheet) {
                if let Some(t) = ws.tables.iter().find(|t| {
                    let n = t.range.normalized();
                    n.start.sheet == cell.sheet && cell.row >= n.start.row && cell.row <= n.end.row
                }) {
                    let key = RangeKey::from_range(t.range);
                    range_keys.push(key);
                    self.add_range_edge(key, cell);
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
                        self.index.remove(k);
                    }
                }
            }
        }
    }

    /// Direct dependents of a single-cell write at `cell`: every formula that
    /// reads `cell` directly, PLUS every formula that reads a range whose box
    /// contains `cell` (answered by the [`RangeIndex`] stab).
    pub fn dependents_of(&self, cell: CellRef) -> Vec<CellRef> {
        let key = strip(cell);
        let mut out: FxHashSet<CellRef> = FxHashSet::default();
        if let Some(set) = self.cell_dependents.get(&key) {
            out.extend(set.iter().copied());
        }
        perf_count!(range_probes, 1);
        self.index.stab(key.sheet, key.row, key.col, |rk| {
            if let Some(deps) = self.range_dependents.get(rk) {
                out.extend(deps.iter().copied());
            }
        });
        let mut v: Vec<CellRef> = out.into_iter().collect();
        v.sort();
        v
    }

    /// The direct cell + range dependencies a formula cell reads, as
    /// single-cell `CellRef`s already registered as formula nodes (used by the
    /// topo sort to compute in-degree over the dirty subgraph). Range deps
    /// expand to the candidate cells INSIDE the box (a [`Candidates`] seek).
    pub fn precedents_in(&self, cell: CellRef, candidate: &Candidates<'_>) -> Vec<CellRef> {
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
                candidate.inside(k, |c| {
                    out.insert(c);
                });
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
                        let cref = CellRef {
                            sheet,
                            row,
                            col,
                            row_abs: false,
                            col_abs: false,
                        };
                        let refs = crate::names::refs_with_names(model, f, cref);
                        self.register(cref, &refs, model);
                    }
                }
            }
        }
    }
}

/// Boxes at most this many columns wide are indexed in every column they
/// span (exact answers); wider boxes go to their sheet's wide lane, whose
/// answers are column-filtered.
const NARROW_COLS: u32 = 8;

/// The lane id of a sheet's wide boxes (no real column is `u32::MAX`).
const WIDE_LANE: u32 = u32::MAX;

/// Leaves of the row segment tree: the whole `u32` row domain, so any row a
/// reference can name has a leaf (Excel's 1 048 576 rows fit many times).
const LEAF_BASE: u64 = 1 << 32;

/// Levels of the row segment tree: root (level 0) to leaves (level 32).
const LEVELS: usize = 33;

/// A row segment tree: heap-numbered node (root 1, leaves
/// `LEAF_BASE + row`) → the boxes whose canonical row decomposition includes
/// that node. Only non-empty nodes are stored, and `per_level` counts them
/// per level so a stab skips the levels that hold nothing (a lane of short
/// boxes touches a handful of levels, not all 33).
struct RowTree {
    nodes: FxHashMap<u64, FxHashSet<RangeKey>>,
    per_level: [u32; LEVELS],
    /// Levels with `per_level > 0`.
    levels_used: u32,
    /// Every box in this lane: when there are no more boxes than used levels,
    /// a flat scan is cheaper than the walk (a lane with one SUM box is one
    /// test per probe, not one per level).
    boxes: FxHashSet<RangeKey>,
}

impl Default for RowTree {
    fn default() -> Self {
        RowTree {
            nodes: FxHashMap::default(),
            per_level: [0; LEVELS],
            levels_used: 0,
            boxes: FxHashSet::default(),
        }
    }
}

/// The level of a heap-numbered node (root = 0).
fn level_of(node: u64) -> usize {
    (63 - node.leading_zeros()) as usize
}

/// The stabbing index over registered range boxes: "which boxes contain
/// `(sheet, row, col)`".
///
/// Each box's row span `[row0, row1]` decomposes into at most 64 canonical
/// segment-tree nodes; the box is stored at each of them, in the row tree of
/// every column it spans (narrow boxes) or of its sheet's wide lane (boxes
/// wider than [`NARROW_COLS`]). A stab at `row` walks the leaf-to-root path
/// of its column's tree (and of the wide lane), visiting only the levels that
/// hold any node (or, when the lane holds no more boxes than used levels,
/// scanning its few boxes instead): every box
/// stored on that path contains `row`, and each containing box is met exactly
/// once (canonical nodes are disjoint). So a probe examines
/// `hits + wide boxes on that row` entries, never every registered box.
#[derive(Default)]
pub struct RangeIndex {
    lanes: FxHashMap<(SheetId, u32), RowTree>,
}

impl RangeIndex {
    /// The lanes a box is stored in.
    fn lanes_of(key: &RangeKey) -> impl Iterator<Item = u32> {
        let narrow = key.col1 - key.col0 < NARROW_COLS;
        let (lo, hi) = if narrow {
            (key.col0, key.col1)
        } else {
            (WIDE_LANE, WIDE_LANE)
        };
        lo..=hi
    }

    /// The canonical segment-tree nodes covering rows `[row0, row1]`.
    fn nodes_of(row0: u32, row1: u32, mut f: impl FnMut(u64)) {
        let mut l = LEAF_BASE + u64::from(row0);
        let mut r = LEAF_BASE + u64::from(row1) + 1; // half-open
        while l < r {
            if l & 1 == 1 {
                f(l);
                l += 1;
            }
            if r & 1 == 1 {
                r -= 1;
                f(r);
            }
            l >>= 1;
            r >>= 1;
        }
    }

    /// Index a box (idempotent).
    pub fn insert(&mut self, key: RangeKey) {
        for lane in Self::lanes_of(&key) {
            let tree = self.lanes.entry((key.sheet, lane)).or_default();
            if !tree.boxes.insert(key) {
                continue;
            }
            Self::nodes_of(key.row0, key.row1, |n| {
                let set = tree.nodes.entry(n).or_default();
                if set.is_empty() {
                    let at = &mut tree.per_level[level_of(n)];
                    if *at == 0 {
                        tree.levels_used += 1;
                    }
                    *at += 1;
                }
                set.insert(key);
            });
        }
    }

    /// Drop a box from the index (no-op when absent).
    pub fn remove(&mut self, key: RangeKey) {
        for lane in Self::lanes_of(&key) {
            let Some(tree) = self.lanes.get_mut(&(key.sheet, lane)) else {
                continue;
            };
            if !tree.boxes.remove(&key) {
                continue;
            }
            Self::nodes_of(key.row0, key.row1, |n| {
                if let Some(set) = tree.nodes.get_mut(&n) {
                    set.remove(&key);
                    if set.is_empty() {
                        tree.nodes.remove(&n);
                        let at = &mut tree.per_level[level_of(n)];
                        *at -= 1;
                        if *at == 0 {
                            tree.levels_used -= 1;
                        }
                    }
                }
            });
            if tree.boxes.is_empty() {
                self.lanes.remove(&(key.sheet, lane));
            }
        }
    }

    /// Call `hit` with every indexed box containing `(sheet, row, col)`,
    /// each exactly once (in no particular order).
    pub fn stab(&self, sheet: SheetId, row: u32, col: u32, mut hit: impl FnMut(&RangeKey)) {
        for lane in [col, WIDE_LANE] {
            let Some(tree) = self.lanes.get(&(sheet, lane)) else {
                continue;
            };
            if tree.boxes.len() <= tree.levels_used as usize {
                perf_count!(range_keys_scanned, tree.boxes.len());
                for k in &tree.boxes {
                    if k.contains(sheet, row, col) {
                        hit(k);
                    }
                }
                continue;
            }
            let leaf = LEAF_BASE + u64::from(row);
            for (level, &count) in tree.per_level.iter().enumerate() {
                if count == 0 {
                    continue;
                }
                perf_count!(range_keys_scanned, 1);
                let node = leaf >> (LEVELS - 1 - level);
                if let Some(set) = tree.nodes.get(&node) {
                    perf_count!(range_keys_scanned, set.len());
                    for k in set {
                        if lane != WIDE_LANE || (col >= k.col0 && col <= k.col1) {
                            hit(k);
                        }
                    }
                }
            }
        }
    }
}

/// The dirty cut as the topo sort's candidate set, indexed so "which
/// candidates lie inside this box" is a seek per column, not a scan.
pub struct Candidates<'a> {
    set: &'a FxHashSet<CellRef>,
    /// `(sheet, col, row)` — column-major, so a box column is one range.
    by_col: BTreeSet<(SheetId, u32, u32)>,
}

impl<'a> Candidates<'a> {
    /// Index a dirty cut (`O(d log d)`).
    pub fn new(set: &'a FxHashSet<CellRef>) -> Self {
        let by_col = set.iter().map(|c| (c.sheet, c.col, c.row)).collect();
        Candidates { set, by_col }
    }

    /// Whether `cell` is a candidate.
    pub fn contains(&self, cell: &CellRef) -> bool {
        self.set.contains(cell)
    }

    /// Call `f` with every candidate inside `k`. Seeks each of the box's
    /// columns; a box with more columns than there are candidates scans the
    /// candidates instead (the cheaper of the two).
    pub fn inside(&self, k: &RangeKey, mut f: impl FnMut(CellRef)) {
        let cols = u64::from(k.col1 - k.col0) + 1;
        if cols > self.by_col.len() as u64 {
            perf_count!(precedent_candidates_scanned, self.set.len());
            for c in self.set {
                if k.contains(c.sheet, c.row, c.col) {
                    f(*c);
                }
            }
            return;
        }
        for col in k.col0..=k.col1 {
            perf_count!(precedent_candidates_scanned, 1);
            for &(sheet, col, row) in self
                .by_col
                .range((k.sheet, col, k.row0)..=(k.sheet, col, k.row1))
            {
                perf_count!(precedent_candidates_scanned, 1);
                f(CellRef {
                    sheet,
                    row,
                    col,
                    row_abs: false,
                    col_abs: false,
                });
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

    // COVERS: the interval index answers exactly what the brute-force scan
    // over every registered box answers (stab + candidate seek), through
    // inserts AND removals, narrow and wide boxes, several sheets.
    #[allow(non_snake_case)]
    mod index_equals_scan {
        use super::super::*;
        use proptest::prelude::*;

        fn key() -> impl Strategy<Value = RangeKey> {
            (0u16..2, 0u32..40, 0u32..4, 0u32..40, 0u32..12).prop_map(|(sheet, r0, c0, h, w)| {
                RangeKey {
                    sheet,
                    row0: r0,
                    col0: c0,
                    row1: r0 + h,
                    col1: c0 + w,
                }
            })
        }

        proptest! {
            #[test]
            fn stab_matches_brute_force__feat__sheet_calc_engine(
                keys in proptest::collection::vec(key(), 0..80),
                drop in proptest::collection::vec(any::<bool>(), 80),
                probes in proptest::collection::vec((0u16..2, 0u32..90, 0u32..20), 1..40),
            ) {
                let mut idx = RangeIndex::default();
                let mut live: FxHashSet<RangeKey> = FxHashSet::default();
                for k in &keys {
                    idx.insert(*k);
                    live.insert(*k);
                }
                for (k, d) in keys.iter().zip(drop.iter()) {
                    if *d {
                        idx.remove(*k);
                        live.remove(k);
                    }
                }
                for (sheet, row, col) in probes {
                    let mut got: Vec<RangeKey> = Vec::new();
                    idx.stab(sheet, row, col, |k| got.push(*k));
                    let n = got.len();
                    got.sort_by_key(|k| (k.sheet, k.row0, k.col0, k.row1, k.col1));
                    got.dedup();
                    prop_assert_eq!(n, got.len(), "a box was reported twice");
                    let mut want: Vec<RangeKey> =
                        live.iter().filter(|k| k.contains(sheet, row, col)).copied().collect();
                    want.sort_by_key(|k| (k.sheet, k.row0, k.col0, k.row1, k.col1));
                    prop_assert_eq!(got, want);
                }
            }

            #[test]
            fn candidates_match_brute_force__feat__sheet_calc_engine(
                cells in proptest::collection::vec((0u16..2, 0u32..60, 0u32..30), 0..60),
                boxes in proptest::collection::vec(key(), 1..20),
            ) {
                let set: FxHashSet<CellRef> = cells
                    .iter()
                    .map(|&(sheet, row, col)| CellRef { sheet, row, col, row_abs: false, col_abs: false })
                    .collect();
                let cands = Candidates::new(&set);
                for k in &boxes {
                    let mut got: Vec<CellRef> = Vec::new();
                    cands.inside(k, |c| got.push(c));
                    got.sort();
                    let mut want: Vec<CellRef> =
                        set.iter().filter(|c| k.contains(c.sheet, c.row, c.col)).copied().collect();
                    want.sort();
                    prop_assert_eq!(got, want);
                }
            }
        }

        #[test]
        fn whole_domain_rows_stab__feat__sheet_calc_engine() {
            // Row bounds at the ends of the u32 domain still decompose and stab.
            let k = RangeKey {
                sheet: 0,
                row0: 0,
                col0: 0,
                row1: u32::MAX,
                col1: 0,
            };
            let mut idx = RangeIndex::default();
            idx.insert(k);
            for row in [0, 1, 1_048_575, u32::MAX] {
                let mut n = 0;
                idx.stab(0, row, 0, |_| n += 1);
                assert_eq!(n, 1, "row {row}");
            }
            idx.remove(k);
            assert!(idx.lanes.is_empty());
        }
    }
}
