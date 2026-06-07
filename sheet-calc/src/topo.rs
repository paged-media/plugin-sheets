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

//! Topological recalc ordering (spec §6.2). Kahn's algorithm over the **dirty
//! subgraph**: nodes are the dirty formula cells; edges run from a precedent
//! (a dependency that is itself dirty) to its dependent. We drain the
//! zero-in-degree frontier in a STABLE order (a [`BinaryHeap`] keyed by a
//! reversed [`CellRef`] so the smallest `CellRef` pops first) — so the emitted
//! order, and therefore every computed value, never depends on hash iteration
//! order. Determinism is a registry row
//! (`sheet.calc.recalc.order-independence`).
//!
//! Cells that still have in-degree > 0 after the drain are members of a
//! **cycle**: they are returned separately as the `cycle` set. Per the
//! `sheet.calc.circular` ruling, the engine stores `CellValue::Error(Ref)` for
//! those (there is no `#CIRCULAR!` wire code — frozen 8) AND reports them on
//! `RecalcResult.circular`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use rustc_hash::{FxHashMap, FxHashSet};
use sheet_core::CellRef;

use crate::graph::DepGraph;

/// The result of ordering a dirty cut: the cells to evaluate in dependency
/// order, plus the cells that could not be ordered because they sit on a cycle.
pub struct TopoOrder {
    /// Cells in a valid evaluation order (every precedent before its
    /// dependent).
    pub order: Vec<CellRef>,
    /// Cells on a cycle (in-degree never reached zero). Sorted.
    pub cycle: Vec<CellRef>,
}

/// Order the dirty subgraph `dirty` using `graph`'s edges. Only edges WITHIN
/// `dirty` matter (a clean precedent already holds its fresh value).
pub fn order(dirty: &FxHashSet<CellRef>, graph: &DepGraph) -> TopoOrder {
    // In-degree of each dirty node = number of its precedents that are also
    // dirty. Build the dependent adjacency (precedent -> dependents) at the
    // same time so we can decrement on drain.
    let mut in_degree: FxHashMap<CellRef, usize> = FxHashMap::default();
    let mut dependents: FxHashMap<CellRef, Vec<CellRef>> = FxHashMap::default();

    for &cell in dirty {
        let precedents = graph.precedents_in(cell, dirty);
        in_degree.entry(cell).or_insert(0);
        for p in precedents {
            // Edge p -> cell.
            *in_degree.entry(cell).or_insert(0) += 1;
            dependents.entry(p).or_default().push(cell);
            in_degree.entry(p).or_insert(0);
        }
    }

    // Stable frontier: min-CellRef pops first (Reverse over a max-heap).
    let mut frontier: BinaryHeap<Reverse<CellRef>> = BinaryHeap::new();
    for (&cell, &deg) in &in_degree {
        if deg == 0 {
            frontier.push(Reverse(cell));
        }
    }

    let mut order: Vec<CellRef> = Vec::with_capacity(dirty.len());
    while let Some(Reverse(cell)) = frontier.pop() {
        order.push(cell);
        if let Some(deps) = dependents.get(&cell) {
            // Sort the freed dependents so equal-rank reveals are deterministic
            // even before they reach the heap (belt-and-suspenders with the
            // heap's own ordering).
            let mut deps = deps.clone();
            deps.sort();
            for d in deps {
                if let Some(deg) = in_degree.get_mut(&d) {
                    *deg -= 1;
                    if *deg == 0 {
                        frontier.push(Reverse(d));
                    }
                }
            }
        }
    }

    // Anything not emitted is on a cycle.
    let emitted: FxHashSet<CellRef> = order.iter().copied().collect();
    let mut cycle: Vec<CellRef> = dirty
        .iter()
        .filter(|c| !emitted.contains(c))
        .copied()
        .collect();
    cycle.sort();

    TopoOrder { order, cycle }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheet_core::{RangeRef, SheetModel};
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

    fn dirty_of(cells: &[CellRef]) -> FxHashSet<CellRef> {
        cells.iter().copied().collect()
    }

    #[test]
    fn chain_orders_precedent_first() {
        let m = SheetModel::new();
        let mut g = DepGraph::new();
        // B1=A1 ; C1=B1 ; D1=C1.
        g.register(cr(0, 1), &refs(&[cr(0, 0)]), &m);
        g.register(cr(0, 2), &refs(&[cr(0, 1)]), &m);
        g.register(cr(0, 3), &refs(&[cr(0, 2)]), &m);
        let dirty = dirty_of(&[cr(0, 1), cr(0, 2), cr(0, 3)]);
        let to = order(&dirty, &g);
        assert!(to.cycle.is_empty());
        assert_eq!(to.order, vec![cr(0, 1), cr(0, 2), cr(0, 3)]);
    }

    #[test]
    fn cycle_detected() {
        let m = SheetModel::new();
        let mut g = DepGraph::new();
        // A1=B1 ; B1=A1  (a 2-cycle).
        g.register(cr(0, 0), &refs(&[cr(0, 1)]), &m);
        g.register(cr(0, 1), &refs(&[cr(0, 0)]), &m);
        let dirty = dirty_of(&[cr(0, 0), cr(0, 1)]);
        let to = order(&dirty, &g);
        assert!(to.order.is_empty());
        assert_eq!(to.cycle, vec![cr(0, 0), cr(0, 1)]);
    }

    #[test]
    fn stable_order_for_independent_nodes() {
        let m = SheetModel::new();
        let mut g = DepGraph::new();
        // Three independent formulas all reading A1.
        g.register(cr(0, 1), &refs(&[cr(0, 0)]), &m);
        g.register(cr(0, 2), &refs(&[cr(0, 0)]), &m);
        g.register(cr(0, 3), &refs(&[cr(0, 0)]), &m);
        let dirty = dirty_of(&[cr(0, 3), cr(0, 1), cr(0, 2)]);
        let to = order(&dirty, &g);
        // Stable: emitted by ascending CellRef.
        assert_eq!(to.order, vec![cr(0, 1), cr(0, 2), cr(0, 3)]);
    }

    #[test]
    fn range_precedent_inside_box_orders_first() {
        let m = SheetModel::new();
        let mut g = DepGraph::new();
        // B1 = SUM(A1:A3) ; A2 = 1 (a formula inside the box).
        let range = RangeRef {
            start: cr(0, 0),
            end: cr(2, 0),
        };
        g.register(
            cr(0, 1),
            &RefSet {
                ranges: vec![range.normalized()],
                ..Default::default()
            },
            &m,
        );
        // A2 is a formula precedent inside the range box.
        g.register(cr(1, 0), &refs(&[cr(5, 5)]), &m);
        let dirty = dirty_of(&[cr(0, 1), cr(1, 0)]);
        let to = order(&dirty, &g);
        assert!(to.cycle.is_empty());
        // A2 (inside the box) must be ordered before B1 (reads the box).
        let pos_a2 = to.order.iter().position(|&c| c == cr(1, 0)).unwrap();
        let pos_b1 = to.order.iter().position(|&c| c == cr(0, 1)).unwrap();
        assert!(pos_a2 < pos_b1);
    }
}
