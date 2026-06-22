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

//! Dirty tracking and volatility (spec §6.2). The dirty set is the cut of
//! formula cells that must be recomputed. A committed write marks the written
//! cell's dependents dirty **transitively** (closure over the reverse edges in
//! [`crate::graph::DepGraph`]). Volatile formula cells (their AST has
//! `has_volatile`, e.g. `NOW`/`RAND`) live in a separate set that is reseeded
//! dirty at the START of every recalc pass — so they recompute every time.

use rustc_hash::FxHashSet;
use sheet_core::CellRef;

use crate::graph::DepGraph;

/// The engine's dirty bookkeeping: the pending dirty cut plus the volatile
/// formula-cell set.
#[derive(Default)]
pub struct Dirty {
    /// Formula cells pending recomputation.
    set: FxHashSet<CellRef>,
    /// Formula cells whose AST is registry-volatile (reseeded every pass).
    volatile: FxHashSet<CellRef>,
}

impl Dirty {
    pub fn new() -> Self {
        Dirty::default()
    }

    /// Mark a single formula cell dirty (no propagation). Used to seed an
    /// initial cut before [`Dirty::propagate_from`].
    pub fn mark(&mut self, cell: CellRef) {
        self.set.insert(cell);
    }

    /// Mark EVERY formula cell dirty (the `recalc_all` seed and the
    /// `Engine::new` "everything dirty" state).
    pub fn mark_all(&mut self, graph: &DepGraph) {
        for c in graph.formula_cells_sorted() {
            self.set.insert(c);
        }
    }

    /// Record (or clear) a cell's volatility. A volatile cell is reseeded dirty
    /// at the start of each pass.
    pub fn set_volatile(&mut self, cell: CellRef, is_volatile: bool) {
        if is_volatile {
            self.volatile.insert(cell);
        } else {
            self.volatile.remove(&cell);
        }
    }

    /// Drop a cell from all tracking (it is no longer a formula).
    pub fn forget(&mut self, cell: CellRef) {
        self.set.remove(&cell);
        self.volatile.remove(&cell);
    }

    /// Propagate dirtiness transitively from a just-written cell `origin`:
    /// every transitive dependent (over reverse cell + range edges) becomes
    /// dirty. `origin` itself is NOT added (a plain value write is not a
    /// formula recompute); only its dependents are. If `origin` is itself a
    /// formula whose input changed, the caller marks it explicitly.
    pub fn propagate_from(&mut self, origin: CellRef, graph: &DepGraph) {
        let mut stack: Vec<CellRef> = graph.dependents_of(origin);
        while let Some(c) = stack.pop() {
            if self.set.insert(c) {
                // Newly dirtied — its own dependents may now be dirty too.
                for d in graph.dependents_of(c) {
                    if !self.set.contains(&d) {
                        stack.push(d);
                    }
                }
            }
        }
    }

    /// Reseed the volatile cells into the dirty cut (called at the start of a
    /// recalc pass) and return the full dirty set as a snapshot.
    pub fn take_pass_seed(&mut self) -> FxHashSet<CellRef> {
        for v in &self.volatile {
            self.set.insert(*v);
        }
        self.set.clone()
    }

    /// Reseed the volatile cells into the dirty cut WITHOUT taking a snapshot
    /// (spill fixpoint: volatiles reseed ONCE per recalc, not per sub-pass, so a
    /// volatile cell does not keep the spill-reflow loop alive forever).
    pub fn reseed_volatile(&mut self) {
        for v in &self.volatile {
            self.set.insert(*v);
        }
    }

    /// Snapshot the current dirty cut and CLEAR it, so cells dirtied during the
    /// snapshot's processing (e.g. spill-reflow dependents) accumulate fresh for
    /// the next drain. Empty result ⇒ the fixpoint has settled.
    pub fn drain_set(&mut self) -> FxHashSet<CellRef> {
        std::mem::take(&mut self.set)
    }

    /// Clear the pending dirty cut (after a pass computed it). Volatile
    /// membership is retained (it reseeds next pass).
    pub fn clear(&mut self) {
        self.set.clear();
    }

    /// Whether a given cell is currently dirty.
    pub fn is_dirty(&self, cell: CellRef) -> bool {
        self.set.contains(&cell)
    }

    /// Whether a cell is tracked volatile.
    pub fn is_volatile(&self, cell: CellRef) -> bool {
        self.volatile.contains(&cell)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::SheetModel;
    use sheet_parser::RefSet;

    fn cr(row: u32, col: u32) -> CellRef {
        CellRef {
            sheet: 0,
            row,
            col,
            row_abs: false,
            col_abs: false,
        }
    }

    fn refs(cells: &[CellRef]) -> RefSet {
        RefSet {
            cells: cells.to_vec(),
            ..Default::default()
        }
    }

    #[test]
    fn transitive_propagation() {
        let m = SheetModel::new();
        let mut g = DepGraph::new();
        // B1 = A1 ; C1 = B1 ; D1 = C1   (a 3-deep chain off A1).
        g.register(cr(0, 1), &refs(&[cr(0, 0)]), &m);
        g.register(cr(0, 2), &refs(&[cr(0, 1)]), &m);
        g.register(cr(0, 3), &refs(&[cr(0, 2)]), &m);

        let mut d = Dirty::new();
        d.propagate_from(cr(0, 0), &g);
        // All three dependents become dirty; A1 itself does not.
        assert!(d.is_dirty(cr(0, 1)));
        assert!(d.is_dirty(cr(0, 2)));
        assert!(d.is_dirty(cr(0, 3)));
        assert!(!d.is_dirty(cr(0, 0)));
    }

    #[test]
    fn volatile_reseeds_each_pass() {
        let mut d = Dirty::new();
        d.set_volatile(cr(0, 0), true);
        // First pass.
        let seed1 = d.take_pass_seed();
        assert!(seed1.contains(&cr(0, 0)));
        d.clear();
        // Second pass: still seeded even though clear() ran.
        let seed2 = d.take_pass_seed();
        assert!(seed2.contains(&cr(0, 0)));
    }

    #[test]
    fn forget_removes_volatile_and_dirty() {
        let mut d = Dirty::new();
        d.mark(cr(0, 0));
        d.set_volatile(cr(0, 0), true);
        d.forget(cr(0, 0));
        assert!(!d.is_dirty(cr(0, 0)));
        assert!(!d.is_volatile(cr(0, 0)));
    }
}
