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

//! PERF BUDGETS — the session-level edits (`sheet_js::core::SheetSession`),
//! counted by the calc engine's work counters (`sheet_calc::perf`).
//!
//! Same rules as `sheet-calc/tests/perf_budgets.rs`: a budget is a COUNT,
//! pinned exactly as measured, only ever LOWERED and only in the commit that
//! earns it, with a behaviour assertion beside it. `PERF_SHOW=1` prints.
//!
//! These are the bulk writes the survey read from code as "one recalc per
//! written cell" (sort, paste, replace) and the save that rebuilds the
//! engine. A batch `setCells` door (Wave 2) should take `recalcs` to 1 and
//! the per-cell SUM re-evaluations with it.

// Test names end in `__feat__<cockpit id>` — the cockpit link (CLAUDE.md).
#![allow(non_snake_case)]

use sheet_calc::perf::{self, PerfCounters};
use sheet_js::core::{FindOptions, SheetSession};

fn measure<R>(f: impl FnOnce() -> R) -> (R, PerfCounters) {
    perf::reset();
    let r = f();
    (r, perf::snapshot())
}

fn check(scenario: &str, actual: PerfCounters, budget: PerfCounters) {
    if std::env::var_os("PERF_SHOW").is_some() {
        println!("PERF {scenario} {actual:?}");
    }
    assert_eq!(
        actual, budget,
        "{scenario}: the work moved. Higher = a regression (fix the change, \
         never raise the budget); lower = earned (lower the budget in this commit)"
    );
}

const ROWS: u32 = 1000;

/// A1:A1000 = 1000..1 (descending), C1 = SUM(A1:A1000).
fn descending_with_sum() -> SheetSession {
    let mut s = SheetSession::new();
    for i in 0..ROWS {
        s.set_cell(0, i, 0, &(ROWS - i).to_string()).unwrap();
    }
    s.set_cell(0, 0, 2, &format!("=SUM(A1:A{ROWS})")).unwrap();
    s
}

// COVERS: sort_range's apply lane (core.rs) — every moved cell re-enters
// through Engine::enter, so the SUM over the sorted column recalcs once PER
// MOVED CELL, reading the whole column each time. Inherent: 0 evaluations
// (the sum of a permutation is unchanged — but whole-range invalidation
// must evaluate it once). Batched: 1 recalc, 1 evaluation, 1000 cells.
#[test]
fn perf_sort_1k_rows_with_sum__feat__sheet_edit_ops() {
    let mut s = descending_with_sum();
    let (res, work) = measure(|| s.sort_range(0, &format!("A1:A{ROWS}"), 0, true, false));
    let res = res.expect("a values-only range sorts");
    // Behaviour: ascending, the sum unchanged, every row rewritten (the
    // even-length reversal moves every cell).
    assert_eq!(s.get_cell_display(0, 0, 0), "1");
    assert_eq!(s.get_cell_display(0, ROWS - 1, 0), ROWS.to_string());
    assert_eq!(s.get_cell_display(0, 0, 2), "500500");
    assert_eq!(res.edits.len(), ROWS as usize);
    check(
        "sort 1000 rows with a SUM over them",
        work,
        PerfCounters {
            range_probes: 2_000,
            range_keys_scanned: 2_000,
            precedent_candidates_scanned: 1_000,
            ranges_materialized: 1_000,
            cells_read: 1_000_000, // the column once per moved cell — batched → 1_000
            evaluations: 1_000,    // the SUM once per moved cell — batched → 1
            recalcs: 1_000,        // one per moved cell — batched → 1
            recalc_passes: 1_000,
            cells_marked_dirty: 1_000,
        },
    );
}

// COVERS: a 100×10 paste as the bundle does it today — one set_cell per
// cell (session.ts paste lane) — under a column total per pasted column.
#[test]
fn perf_paste_100x10_under_totals__feat__sheet_edit_ops() {
    let mut s = SheetSession::new();
    for c in 0..10u32 {
        let col = (b'A' + c as u8) as char;
        s.set_cell(0, 100, c, &format!("=SUM({col}1:{col}100)"))
            .unwrap();
    }
    let (_, work) = measure(|| {
        for r in 0..100u32 {
            for c in 0..10u32 {
                s.set_cell(0, r, c, &(r + 1).to_string()).unwrap();
            }
        }
    });
    for c in 0..10u32 {
        assert_eq!(s.get_cell_display(0, 100, c), "5050");
    }
    check(
        "paste 100x10 under 10 column totals",
        work,
        PerfCounters {
            range_probes: 2_000,
            range_keys_scanned: 20_000,
            precedent_candidates_scanned: 1_000,
            ranges_materialized: 1_000,
            cells_read: 100_000,
            evaluations: 1_000, // each total once per cell in its column — batched → 10
            recalcs: 1_000,     // one per pasted cell — batched → 1
            recalc_passes: 1_000,
            cells_marked_dirty: 1_000,
        },
    );
}

// COVERS: replace_all over a column of values feeding a SUM (core.rs replace
// lane) — one re-entry and one SUM recalc per replaced cell.
#[test]
fn perf_replace_500_under_sum__feat__sheet_edit_ops() {
    let mut s = SheetSession::new();
    for r in 0..500u32 {
        s.set_cell(0, r, 0, "7").unwrap();
    }
    s.set_cell(0, 500, 0, "=SUM(A1:A500)").unwrap();
    let (res, work) = measure(|| {
        s.replace_all(
            Some(0),
            "7",
            "8",
            FindOptions {
                entire_cell: true,
                ..FindOptions::default()
            },
        )
    });
    let res = res.expect("replace runs");
    assert_eq!(res.occurrences, 500);
    assert_eq!(s.get_cell_display(0, 500, 0), "4000");
    check(
        "replace 500 cells under a SUM",
        work,
        PerfCounters {
            range_probes: 1_000,
            range_keys_scanned: 1_000,
            precedent_candidates_scanned: 500,
            ranges_materialized: 500,
            cells_read: 250_000, // N²/2-shaped — batched → 500
            evaluations: 500,
            recalcs: 500, // one per replaced cell — batched → 1
            recalc_passes: 500,
            cells_marked_dirty: 500,
        },
    );
}

// COVERS: save_xlsx — it rebuilds the Engine from the model, and
// Engine::new marks EVERY formula dirty (core.rs save step 5). The rebuild
// does not recalc, but the next edit pays for the whole workbook: measured
// here as save + one unrelated edit. Inherent for that edit: 0 evaluations.
#[test]
fn perf_save_then_edit__feat__sheet_xlsx_roundtrip() {
    let mut s = SheetSession::new();
    for r in 0..500u32 {
        s.set_cell(0, r, 0, &(r + 1).to_string()).unwrap();
        s.set_cell(0, r, 1, &format!("=A{}*2", r + 1)).unwrap();
    }
    let (_, work) = measure(|| {
        s.save_xlsx().expect("saves");
        // An edit that no formula reads.
        s.set_cell(0, 0, 5, "x").unwrap();
    });
    assert_eq!(s.get_cell_display(0, 499, 1), "1000");
    check(
        "save + one unrelated edit, 500 formulas",
        work,
        PerfCounters {
            range_probes: 1,
            range_keys_scanned: 0,
            precedent_candidates_scanned: 0,
            ranges_materialized: 0,
            cells_read: 0,
            evaluations: 500, // the whole workbook: save left every formula dirty → 0
            recalcs: 1,
            recalc_passes: 1,
            cells_marked_dirty: 500, // Engine::new's mark_all inside save → 0
        },
    );
}
