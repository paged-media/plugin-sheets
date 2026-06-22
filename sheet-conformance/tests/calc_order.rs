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

//! Topological-recalc and order-independence conformance (spec §6.2/§12.4).
//! Test-fn names match the `registry/features/calc.yaml` pointers:
//! - `sheet_calc_recalc_topo`              (sheet.calc.recalc.topo)
//! - `sheet_calc_recalc_order_independence` (sheet.calc.recalc.order-independence)
//!
//! The order-independence test is a PROPERTY: it generates random DAGs of
//! formula cells, builds the engine, recalcs, then REBUILDS the same model with
//! the cells inserted in a RANDOM order and recalcs again — the resulting value
//! maps must be identical. Determinism is the registry row; the stable Kahn
//! frontier in `sheet_calc::topo` is what guarantees it.

use std::collections::BTreeMap;

use proptest::prelude::*;
use sheet_calc::{Engine, EngineConfig};
use sheet_core::{CellValue, SheetModel};

fn val(e: &Engine, sheet: u16, row: u32, col: u32) -> CellValue {
    e.model()
        .sheet(sheet)
        .and_then(|ws| ws.cell(row, col))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

// =================================================================
// sheet.calc.recalc.topo — chained formulas correct through a 4-level chain
// (including a cross-sheet hop)
// =================================================================

#[test]
fn sheet_calc_recalc_topo() {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    m.add_sheet("Sheet2");
    let mut e = Engine::new(m, EngineConfig::default());

    // A 4-level dependency chain with a cross-sheet hop in the middle:
    //   A1 = 2                         (level 0, literal)
    //   B1 = A1 * 3        -> 6        (level 1)
    //   Sheet2!A1 = B1 + 1 -> 7        (level 2, cross-sheet)
    //   C1 = Sheet2!A1 * 2 -> 14       (level 3)
    e.enter(0, 0, 0, "2").unwrap();
    e.enter(0, 0, 1, "=A1*3").unwrap();
    e.enter(1, 0, 0, "=Sheet1!B1+1").unwrap();
    e.enter(0, 0, 2, "=Sheet2!A1*2").unwrap();

    assert_eq!(val(&e, 0, 0, 1), CellValue::Number(6.0));
    assert_eq!(val(&e, 1, 0, 0), CellValue::Number(7.0));
    assert_eq!(val(&e, 0, 0, 2), CellValue::Number(14.0));

    // Perturb the root: the whole chain re-settles to the correct values.
    e.enter(0, 0, 0, "5").unwrap();
    assert_eq!(val(&e, 0, 0, 1), CellValue::Number(15.0)); // 5*3
    assert_eq!(val(&e, 1, 0, 0), CellValue::Number(16.0)); // +1
    assert_eq!(val(&e, 0, 0, 2), CellValue::Number(32.0)); // *2

    // recalc_all reproduces the same fixpoint (order-independent).
    e.recalc_all();
    assert_eq!(val(&e, 0, 0, 1), CellValue::Number(15.0));
    assert_eq!(val(&e, 1, 0, 0), CellValue::Number(16.0));
    assert_eq!(val(&e, 0, 0, 2), CellValue::Number(32.0));
}

// =================================================================
// sheet.calc.recalc.order-independence — PROPERTY
// =================================================================

/// A generated DAG cell: a single-column formula `=SUM(<earlier refs>) + lit`,
/// where every referenced cell has a strictly smaller row index (so the graph
/// is acyclic by construction).
#[derive(Clone, Debug)]
struct GenCell {
    row: u32,
    deps: Vec<u32>,
    lit: i32,
}

/// Build the formula text for a generated cell. Column A is the single column;
/// refs are `A<dep+1>` (1-based A1 rows). With no deps it is just the literal.
fn formula_text(cell: &GenCell) -> String {
    if cell.deps.is_empty() {
        format!("{}", cell.lit)
    } else {
        let refs: Vec<String> = cell.deps.iter().map(|d| format!("A{}", d + 1)).collect();
        format!("=SUM({}) + {}", refs.join(","), cell.lit)
    }
}

/// Strategy: 30 cells; cell `i` (row i) depends on 0..=3 earlier rows.
fn dag_strategy() -> impl Strategy<Value = Vec<GenCell>> {
    let n = 30usize;
    // For each row i, pick a literal and a set of earlier-row deps.
    (0..n)
        .map(move |i| {
            let lit = -5i32..=5i32;
            // deps are chosen from 0..i (earlier rows); empty for row 0.
            let dep_pool: Vec<u32> = (0..i as u32).collect();
            let deps = if dep_pool.is_empty() {
                Just(Vec::<u32>::new()).boxed()
            } else {
                proptest::sample::subsequence(dep_pool, 0..=3.min(i)).boxed()
            };
            (Just(i as u32), deps, lit).prop_map(|(row, deps, lit)| GenCell { row, deps, lit })
        })
        .collect::<Vec<_>>()
}

/// Build a model + engine by entering the cells in the given `order`, recalc,
/// and return the final value map (A1.. as `(row -> CellValue)`).
fn run_in_order(cells: &[GenCell], order: &[usize]) -> BTreeMap<u32, CellValue> {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    let mut e = Engine::new(m, EngineConfig::default());
    for &idx in order {
        let cell = &cells[idx];
        e.enter(0, cell.row, 0, &formula_text(cell)).unwrap();
    }
    // A final full recalc settles any cells entered before their precedents.
    e.recalc_all();

    let mut out = BTreeMap::new();
    for cell in cells {
        out.insert(cell.row, val(&e, 0, cell.row, 0));
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    #[test]
    fn sheet_calc_recalc_order_independence(
        cells in dag_strategy(),
        // A permutation seed used to shuffle the insertion order.
        shuffle in proptest::collection::vec(0u64..u64::MAX, 30),
    ) {
        // Natural order: row 0, 1, 2, ... (precedents-first).
        let natural: Vec<usize> = (0..cells.len()).collect();
        let natural_values = run_in_order(&cells, &natural);

        // Random order: a deterministic Fisher-Yates using the proptest seed,
        // so cells are inserted in an arbitrary order (often dependents before
        // precedents — the engine must still converge identically).
        let mut shuffled = natural.clone();
        for i in (1..shuffled.len()).rev() {
            let j = (shuffle[i] % (i as u64 + 1)) as usize;
            shuffled.swap(i, j);
        }
        let shuffled_values = run_in_order(&cells, &shuffled);

        prop_assert_eq!(
            natural_values,
            shuffled_values,
            "recalc result depends on insertion order — determinism broken"
        );
    }
}

/// A second, fixed (non-proptest) seed witness so the registry pointer always
/// has a concrete green case even if proptest shrinking is disabled, and so a
/// reviewer can read one concrete instance.
#[test]
fn sheet_calc_recalc_order_independence_fixed_witness() {
    let cells = vec![
        GenCell {
            row: 0,
            deps: vec![],
            lit: 3,
        },
        GenCell {
            row: 1,
            deps: vec![0],
            lit: 1,
        },
        GenCell {
            row: 2,
            deps: vec![0, 1],
            lit: -2,
        },
        GenCell {
            row: 3,
            deps: vec![1, 2],
            lit: 0,
        },
    ];
    let natural = run_in_order(&cells, &[0, 1, 2, 3]);
    // Insert dependents BEFORE precedents.
    let reversed = run_in_order(&cells, &[3, 2, 1, 0]);
    assert_eq!(natural, reversed);
    // Spot-check the arithmetic: A1=3, A2=3+1=4, A3=3+4-2=5, A4=4+5=9.
    assert_eq!(natural[&0], CellValue::Number(3.0));
    assert_eq!(natural[&1], CellValue::Number(4.0));
    assert_eq!(natural[&2], CellValue::Number(5.0));
    assert_eq!(natural[&3], CellValue::Number(9.0));
}
