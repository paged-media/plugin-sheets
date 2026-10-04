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

// COVERS: sort_range's apply lane (core.rs) — the moved cells re-enter
// through ONE Engine::set_cells batch, so the SUM over the sorted column
// evaluates once (whole-range invalidation must evaluate it once; the sum of
// a permutation is unchanged). Before the batch door: one recalc and one
// whole-column SUM per moved cell (1000 / 1 000 000 cells).
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
            range_probes: 1_001, // 1000 written cells + the SUM's own probe
            range_keys_scanned: 1_000,
            precedent_candidates_scanned: 1,
            ranges_materialized: 1,
            cells_read: 1_000, // the column ONCE (was 1 000 000: once per moved cell)
            evaluations: 1,    // the SUM once (was 1 000)
            recalcs: 1,        // one batch (was 1 000: one per moved cell)
            recalc_passes: 1,
            cells_marked_dirty: 1,
        },
    );
}

// COVERS: a 100×10 paste as the bundle does it today — one set_cell per
// cell (session.ts paste lane) — under a column total per pasted column.
// The batched lane below is what the bundle moves to (`setCells`).
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
            range_keys_scanned: 2_000, // one box per column lane (was 20 000: every probe × 10 boxes)
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

// COVERS: the same paste through the batch door (`set_cells`, the wasm
// `setCells`): one recalc, each column total evaluated once.
#[test]
fn perf_paste_100x10_batched__feat__sheet_edit_ops() {
    use sheet_js::core::CellInput;
    let mut s = SheetSession::new();
    for c in 0..10u32 {
        let col = (b'A' + c as u8) as char;
        s.set_cell(0, 100, c, &format!("=SUM({col}1:{col}100)"))
            .unwrap();
    }
    let batch: Vec<CellInput> = (0..100u32)
        .flat_map(|r| {
            (0..10u32).map(move |c| CellInput {
                sheet: 0,
                row: r,
                col: c,
                input: (r + 1).to_string(),
            })
        })
        .collect();
    let (res, work) = measure(|| s.set_cells(&batch));
    let res = res.expect("the batch applies");
    for c in 0..10u32 {
        assert_eq!(s.get_cell_display(0, 100, c), "5050");
    }
    assert_eq!(res.changed_count, 1_010); // 1000 written + 10 totals
    check(
        "paste 100x10 under 10 column totals, one set_cells batch",
        work,
        PerfCounters {
            range_probes: 1_010,
            range_keys_scanned: 1_010,
            precedent_candidates_scanned: 10,
            ranges_materialized: 10,
            cells_read: 1_000, // each column once (per-cell lane: 100 000)
            evaluations: 10,   // each total once (per-cell lane: 1 000)
            recalcs: 1,        // per-cell lane: 1 000
            recalc_passes: 1,
            cells_marked_dirty: 10,
        },
    );
}

// COVERS: a batch is all-or-nothing on bad input — a parse error or a bad
// sheet id rejects it before any cell is written.
#[test]
fn set_cells_rejects_whole_batch_on_bad_input__feat__sheet_edit_ops() {
    use sheet_js::core::CellInput;
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "1").unwrap();
    let cell = |row: u32, input: &str| CellInput {
        sheet: 0,
        row,
        col: 0,
        input: input.to_string(),
    };
    let err = s
        .set_cells(&[cell(0, "5"), cell(1, "=SUM(")])
        .expect_err("a parse error rejects the batch");
    assert!(err.0.contains("A2"), "{}", err.0);
    assert_eq!(s.get_cell_display(0, 0, 0), "1");
    let bad_sheet = CellInput {
        sheet: 9,
        ..cell(0, "5")
    };
    assert!(s.set_cells(&[cell(0, "5"), bad_sheet]).is_err());
    assert_eq!(s.get_cell_display(0, 0, 0), "1");
    // A later input for the same cell wins; each input reports its prior
    // input (the second write's prior is the first write).
    let res = s
        .set_cells(&[cell(0, "5"), cell(0, "6"), cell(1, "=A1*2")])
        .unwrap();
    assert_eq!(s.get_cell_display(0, 0, 0), "6");
    assert_eq!(res.prev_inputs, vec!["1", "5", ""]);
    let res = s.set_cells(&[cell(1, "7")]).unwrap();
    assert_eq!(res.prev_inputs, vec!["=A1*2"]);
}

// COVERS: replace_all over a column of values feeding a SUM (core.rs replace
// lane) — the survivors commit as one set_cells batch: one SUM recalc
// (before: one per replaced cell).
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
            range_probes: 501,
            range_keys_scanned: 501,
            precedent_candidates_scanned: 1,
            ranges_materialized: 1,
            cells_read: 500, // the column once (was 250 000)
            evaluations: 1,  // was 500
            recalcs: 1,      // one batch (was 500: one per replaced cell)
            recalc_passes: 1,
            cells_marked_dirty: 1,
        },
    );
}

// COVERS: save_xlsx — the writer borrows the engine's model, so the engine
// (graph, dirty set) survives the save and the next unrelated edit
// evaluates nothing. Before 2026-10 the save rebuilt the Engine with every
// formula dirty and that edit re-evaluated all 500.
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
            evaluations: 0, // was 500: the save rebuilt the engine all-dirty
            recalcs: 1,
            recalc_passes: 0,      // nothing dirty — the recalc drains an empty cut
            cells_marked_dirty: 0, // was 500 (Engine::new's mark_all)
        },
    );
    // Behaviour: the engine kept its graph across the save — a precedent
    // edit still reaches its dependent, and only that one.
    let (_, work) = measure(|| s.set_cell(0, 499, 0, "1").unwrap());
    assert_eq!(s.get_cell_display(0, 499, 1), "2");
    assert_eq!(work.evaluations, 1);
    // And the saved bytes reload to the same values.
    let bytes = s.save_xlsx().expect("saves");
    let back = SheetSession::load_xlsx(&bytes).expect("reloads");
    assert_eq!(back.get_cell_display(0, 499, 1), "2");
    assert_eq!(back.get_cell_display(0, 0, 1), "2");
}

// COVERS: the fill handle (`fill_range`, Wave 5) — a 2-cell number series
// dragged down 998 rows under a column total. Applied through the batch
// door: ONE recalc, the total evaluated once.
#[test]
fn perf_fill_series_1k_under_sum__feat__sheet_edit_ops() {
    let mut s = SheetSession::new();
    s.set_cell(0, 0, 0, "1").unwrap();
    s.set_cell(0, 1, 0, "2").unwrap();
    s.set_cell(0, ROWS, 0, &format!("=SUM(A1:A{ROWS})"))
        .unwrap();
    let (res, work) = measure(|| s.fill_range(0, "A1:A2", &format!("A1:A{ROWS}"), true));
    let res = res.expect("the fill applies");
    assert_eq!(res.edits.len(), (ROWS - 2) as usize);
    assert_eq!(s.get_cell_display(0, ROWS - 1, 0), ROWS.to_string());
    assert_eq!(s.get_cell_display(0, ROWS, 0), "500500");
    check(
        "fill a series down 998 rows under a SUM",
        work,
        PerfCounters {
            range_probes: 999,
            range_keys_scanned: 999,
            precedent_candidates_scanned: 1,
            ranges_materialized: 1,
            cells_read: 1_000, // the total once (per-cell lane: one read of A1:A1000 per cell)
            evaluations: 1,
            recalcs: 1, // one batch (per-cell lane: 998)
            recalc_passes: 1,
            cells_marked_dirty: 1,
        },
    );
}
