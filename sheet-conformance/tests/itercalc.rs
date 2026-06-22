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

//! Iterative (circular) calculation conformance (spec §6.2, decision D-7;
//! milestone M2). Drives the FROZEN [`sheet_calc::Engine`] surface end-to-end
//! (`enter` → recalc, `set_iterative` toggle) so these tests exercise the same
//! path `sheet-js` will. Test-fn names use the prefixes the
//! `registry/features/itercalc.yaml` rows point at so the coverage gate (§12.2)
//! finds them:
//! - `sheet_calc_iterative_enable_flag`            (sheet.calc.iterative.enable-flag)
//! - `sheet_calc_iterative_max_iter`               (sheet.calc.iterative.max-iter)
//! - `sheet_calc_iterative_max_change`             (sheet.calc.iterative.max-change)
//! - `sheet_calc_iterative_convergence`            (sheet.calc.iterative.convergence)
//! - `sheet_calc_iterative_circular_with_iteration`(sheet.calc.iterative.circular-with-iteration)
//!
//! ## The two policies (D-7)
//!
//! Iterative calc is OFF by default: a dependency cycle keeps the
//! `sheet.calc.circular` ruling (each member stored `#REF!`, reported on
//! [`sheet_calc::RecalcResult::circular`]). With the flag ON, the cycle is
//! seeded at `0` and iterated in a stable (sorted-[`CellRef`]) order up to
//! `max_iter` passes, stopping early when the largest per-cell change is
//! `<= max_change`; `circular` is then empty and a system that exhausts
//! `max_iter` is reported on [`sheet_calc::RecalcResult::non_converged`].
//!
//! ECMA-376 §18.2.2: `iterate` / `iterateCount` / `iterateDelta` (the
//! [`sheet_core::CalcSettings`] `iterative` / `max_iter` / `max_change`).

use sheet_calc::{Engine, EngineConfig};
use sheet_core::{CellError, CellRef, CellValue, SheetModel};

// ---- helpers ----

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

fn val(e: &Engine, row: u32, col: u32) -> CellValue {
    e.model()
        .sheet(0)
        .and_then(|ws| ws.cell(row, col))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

/// Assert a cell holds a number within `tol` of `target`.
fn approx(e: &Engine, row: u32, col: u32, target: f64, tol: f64) {
    match val(e, row, col) {
        CellValue::Number(n) => assert!(
            (n - target).abs() <= tol,
            "cell ({row},{col}): expected ~{target}, got {n} (tol {tol})"
        ),
        other => panic!("cell ({row},{col}): expected a number ~{target}, got {other:?}"),
    }
}

/// Build the canonical convergent 2-cell system: A1 = B1*0.5 + 10, B1 = A1 + 5.
/// Fixed point: A1 = 0.5*(A1+5)+10  ->  0.5*A1 = 12.5  ->  A1 = 25, B1 = 30.
fn convergent_system(e: &mut Engine) {
    e.enter(0, 0, 0, "=B1*0.5+10").unwrap();
    e.enter(0, 0, 1, "=A1+5").unwrap();
}

// =================================================================
// sheet.calc.iterative.enable-flag — OFF by default; the toggle switches policy
// =================================================================

#[test]
fn sheet_calc_iterative_enable_flag_off_by_default() {
    // Default CalcSettings: iteration OFF. A cycle keeps the circular ruling.
    let mut e = engine();
    convergent_system(&mut e);
    let r = e.enter(0, 0, 1, "=A1+5").unwrap();
    assert!(
        !r.circular.is_empty(),
        "iteration off: the cycle must be reported on `circular`"
    );
    assert!(
        r.non_converged.is_empty(),
        "iteration off: `non_converged` is always empty"
    );
    assert_eq!(val(&e, 0, 0), CellValue::Error(CellError::Ref));
    assert_eq!(val(&e, 0, 1), CellValue::Error(CellError::Ref));
}

#[test]
fn sheet_calc_iterative_enable_flag_on_switches_policy() {
    // Enter the cycle with iteration OFF -> #REF!. Turning it ON recalcs and the
    // SAME cells settle (set_iterative triggers a full recalc — enable flag is
    // the only thing that changed).
    let mut e = engine();
    convergent_system(&mut e);
    assert_eq!(val(&e, 0, 0), CellValue::Error(CellError::Ref));

    let r = e.set_iterative(true, 1000, 1e-9);
    assert!(
        r.circular.is_empty(),
        "iteration on: `circular` must be empty for a converged cycle"
    );
    assert!(r.non_converged.is_empty());
    approx(&e, 0, 0, 25.0, 1e-6);
    approx(&e, 0, 1, 30.0, 1e-6);

    // And toggling back off restores the #REF! ruling.
    let r2 = e.set_iterative(false, 100, 0.001);
    assert!(!r2.circular.is_empty());
    assert_eq!(val(&e, 0, 0), CellValue::Error(CellError::Ref));
}

// =================================================================
// sheet.calc.iterative.max-iter — the iteration cap (divergent system stops)
// =================================================================

#[test]
fn sheet_calc_iterative_max_iter_caps_divergent() {
    // The classic self-referential growth A1 = A1 + 1 never converges. Seeded
    // at 0, +1 each pass, it is capped at exactly `max_iter` passes and reported
    // non-converged.
    let mut e = engine();
    e.set_iterative(true, 50, 1e-9);
    let r = e.enter(0, 0, 0, "=A1+1").unwrap();
    assert!(r.circular.is_empty(), "iteration on: no #REF!");
    assert_eq!(
        r.non_converged,
        vec![cr(0, 0)],
        "a divergent cycle is reported non-converged"
    );
    // 50 passes of +1 from a 0 seed -> 50.
    approx(&e, 0, 0, 50.0, 0.0);
}

#[test]
fn sheet_calc_iterative_max_iter_count_is_honored() {
    // The cap is exactly the configured count: two different caps give two
    // different last-iterate values for the same divergent formula.
    let mut e = engine();
    e.set_iterative(true, 10, 1e-12);
    e.enter(0, 0, 0, "=A1+1").unwrap();
    approx(&e, 0, 0, 10.0, 0.0);

    let mut e2 = engine();
    e2.set_iterative(true, 25, 1e-12);
    e2.enter(0, 0, 0, "=A1+1").unwrap();
    approx(&e2, 0, 0, 25.0, 0.0);

    // max_iter == 0 leaves the seed (0) untouched and reports non-convergence.
    let mut e3 = engine();
    e3.set_iterative(true, 0, 1e-12);
    let r = e3.enter(0, 0, 0, "=A1+1").unwrap();
    assert_eq!(r.non_converged, vec![cr(0, 0)]);
    approx(&e3, 0, 0, 0.0, 0.0);
}

// =================================================================
// sheet.calc.iterative.max-change — stop early when the largest delta is small
// =================================================================

#[test]
fn sheet_calc_iterative_max_change_stops_early() {
    // A loose tolerance stops the convergent system before the exact fixed
    // point; a tight one lands closer. Both are within their own tolerance, and
    // the loose run is provably less accurate (proving early-stop fired).
    let mut e_loose = engine();
    e_loose.set_iterative(true, 1000, 0.5);
    convergent_system(&mut e_loose);
    let loose = match val(&e_loose, 0, 0) {
        CellValue::Number(n) => n,
        other => panic!("expected number, got {other:?}"),
    };

    let mut e_tight = engine();
    e_tight.set_iterative(true, 1000, 1e-9);
    convergent_system(&mut e_tight);
    let tight = match val(&e_tight, 0, 0) {
        CellValue::Number(n) => n,
        other => panic!("expected number, got {other:?}"),
    };

    // The tight run is essentially exact (25); the loose run stopped earlier and
    // is farther from the fixed point.
    assert!(
        (tight - 25.0).abs() < 1e-6,
        "tight run reaches the fixed point"
    );
    assert!(
        (loose - 25.0).abs() > (tight - 25.0).abs(),
        "loose tolerance ({loose}) stops earlier than tight ({tight})"
    );
}

#[test]
fn sheet_calc_iterative_max_change_converged_clears_circular() {
    // With a tolerance reachable inside max_iter, the cycle is declared
    // converged: circular AND non_converged are both empty.
    let mut e = engine();
    let r = e.set_iterative(true, 1000, 1e-6);
    // The flag-toggle recalc had no cycle yet; enter the cycle now.
    assert!(r.circular.is_empty());
    convergent_system(&mut e);
    let r2 = e.enter(0, 0, 1, "=A1+5").unwrap();
    assert!(r2.circular.is_empty());
    assert!(r2.non_converged.is_empty());
}

// =================================================================
// sheet.calc.iterative.convergence — the loop is deterministic and settles
// =================================================================

#[test]
fn sheet_calc_iterative_convergence_is_deterministic() {
    // Determinism (spec §6.2): the stable sorted-CellRef iteration order makes
    // the converged values reproducible run-to-run.
    let run = || {
        let mut e = engine();
        e.set_iterative(true, 1000, 1e-12);
        convergent_system(&mut e);
        (val(&e, 0, 0), val(&e, 0, 1))
    };
    let a = run();
    let b = run();
    let c = run();
    assert_eq!(a, b);
    assert_eq!(b, c);
}

#[test]
fn sheet_calc_iterative_convergence_three_cell_chain() {
    // A larger convergent cycle: A1 = (C1 + 100)/2, B1 = A1/2, C1 = B1.
    // Fixed point: C1 = B1 = A1/2 ; A1 = (A1/2 + 100)/2 = A1/4 + 50 ->
    // 0.75*A1 = 50 -> A1 = 66.6667, B1 = C1 = 33.3333.
    let mut e = engine();
    e.set_iterative(true, 2000, 1e-10);
    e.enter(0, 0, 0, "=(C1+100)/2").unwrap();
    e.enter(0, 0, 1, "=A1/2").unwrap();
    let r = e.enter(0, 0, 2, "=B1").unwrap();
    assert!(r.circular.is_empty());
    assert!(r.non_converged.is_empty());
    approx(&e, 0, 0, 200.0 / 3.0, 1e-4);
    approx(&e, 0, 1, 100.0 / 3.0, 1e-4);
    approx(&e, 0, 2, 100.0 / 3.0, 1e-4);
}

// =================================================================
// sheet.calc.iterative.circular-with-iteration — cycle iterates instead of #REF!
// =================================================================

#[test]
fn sheet_calc_iterative_circular_with_iteration_supersedes_ref() {
    // The headline D-7 ruling: with iteration ON a detected cycle iterates to a
    // fixed point INSTEAD of storing #REF!. Same cycle, two policies.
    let mut off = engine();
    convergent_system(&mut off);
    off.enter(0, 0, 1, "=A1+5").unwrap();
    assert_eq!(val(&off, 0, 0), CellValue::Error(CellError::Ref));

    let mut on = engine();
    on.set_iterative(true, 1000, 1e-9);
    convergent_system(&mut on);
    on.enter(0, 0, 1, "=A1+5").unwrap();
    // No #REF! anywhere — the cycle resolved to numbers.
    approx(&on, 0, 0, 25.0, 1e-6);
    approx(&on, 0, 1, 30.0, 1e-6);
    assert_ne!(val(&on, 0, 0), CellValue::Error(CellError::Ref));
}

#[test]
fn sheet_calc_iterative_circular_with_iteration_self_reference() {
    // The classic A1 = A1 + 1 documents the capped semantics: with iteration
    // ON it does NOT store #REF! — it grows by 1 per pass and stops at max_iter,
    // reported non-converged (Excel leaves the last value in place).
    let mut e = engine();
    e.set_iterative(true, 100, 1e-9);
    let r = e.enter(0, 0, 0, "=A1+1").unwrap();
    assert!(r.circular.is_empty());
    assert_eq!(r.non_converged, vec![cr(0, 0)]);
    assert_ne!(val(&e, 0, 0), CellValue::Error(CellError::Ref));
    approx(&e, 0, 0, 100.0, 0.0);
}

#[test]
fn sheet_calc_iterative_circular_with_iteration_downstream_reads_settled() {
    // A non-cycle cell that reads a converged cycle member sees the settled
    // value (the cycle's fixed point flows downstream like any other result).
    let mut e = engine();
    e.set_iterative(true, 1000, 1e-9);
    convergent_system(&mut e);
    e.enter(0, 0, 1, "=A1+5").unwrap();
    // D1 reads A1 (which settled at 25).
    e.enter(0, 0, 3, "=A1+1").unwrap();
    approx(&e, 0, 3, 26.0, 1e-6);
}

// =================================================================
// HARDENING (Phase 9 conservative sweep) — robustness of the EXISTING
// iterate loop: non-numeric oscillation, multiple disjoint cycles in one
// recalc, and a non-finite/negative max_change. These exercise the SAME
// `iterate_cycle` path (no engine change); the registry-pointer prefixes
// (`sheet_calc_iterative_convergence` / `_max_change` / `_circular_with_iteration`)
// keep the coverage gate satisfied without new rows.
// =================================================================

#[test]
fn sheet_calc_iterative_convergence_nonnumeric_oscillation_runs_to_cap() {
    // A cycle whose value oscillates to/from a non-number can never "settle":
    // `iterate::cell_delta` returns INFINITY across a numeric<->non-numeric
    // flip, so the largest delta never falls below max_change and the system
    // runs to max_iter and is reported non-converged (rather than falsely
    // declaring convergence). A1 = IF(ISNUMBER(A1), "x", 1) flips 1 <-> "x"
    // every pass: seed 0 (number) -> "x" -> 1 -> "x" -> ... never stable.
    let mut e = engine();
    e.set_iterative(true, 20, 1e-9);
    let r = e.enter(0, 0, 0, "=IF(ISNUMBER(A1),\"x\",1)").unwrap();
    assert!(r.circular.is_empty(), "iteration on: no #REF!");
    assert_eq!(
        r.non_converged,
        vec![cr(0, 0)],
        "a numeric<->non-numeric oscillation cannot settle — must hit the cap"
    );
    // The last iterate is left in place (Excel-style); it is one of the two
    // oscillation states, never a spurious convergence value.
    match val(&e, 0, 0) {
        CellValue::Number(_) | CellValue::Text(_) => {}
        other => panic!("expected the last oscillation state, got {other:?}"),
    }
}

#[test]
fn sheet_calc_iterative_circular_with_iteration_two_disjoint_cycles() {
    // Two structurally-independent cycles present in one recalc are iterated as
    // ONE circular region (Excel-style: the engine iterates the whole set of
    // circular cells together under a single global convergence verdict — the
    // topo layer returns all un-orderable cells as one `cycle` set, and
    // `iterate_cycle` runs them in one Gauss-Seidel sweep). Cycle 1 (convergent,
    // cols A/B): A1 = B1*0.5+10, B1 = A1+5 -> A1=25, B1=30. Cycle 2 (divergent,
    // col D): D1 = D1+1 -> grows unboundedly. The convergent members reach their
    // correct fixed point, but because the GLOBAL max-delta (driven by the
    // never-settling D1) stays above max_change, the whole region is reported
    // non-converged — the values are right, the convergence LABEL is whole-set.
    let mut e = engine();
    e.set_iterative(true, 100, 1e-9);
    e.enter(0, 0, 0, "=B1*0.5+10").unwrap();
    e.enter(0, 0, 1, "=A1+5").unwrap();
    e.enter(0, 0, 3, "=D1+1").unwrap();
    let r = e.recalc_all();
    assert!(r.circular.is_empty(), "iteration on: no #REF!");
    // The divergent member forces a whole-region non-convergence report.
    assert!(
        r.non_converged.contains(&cr(0, 3)),
        "the divergent member (D1) must be reported non-converged, got {:?}",
        r.non_converged
    );
    assert!(
        r.non_converged.contains(&cr(0, 0)) && r.non_converged.contains(&cr(0, 1)),
        "the whole circular region shares ONE convergence verdict (global \
         max-delta) — the convergent members ride the same non-converged report \
         while D1 diverges, got {:?}",
        r.non_converged
    );
    // CRUCIALLY: the convergent members still hold their CORRECT fixed-point
    // values — the global non-convergence label does not corrupt them. This is
    // the robustness property the hardening test pins: a divergent neighbour in
    // the same region does not poison a convergent cell's value.
    approx(&e, 0, 0, 25.0, 1e-6);
    approx(&e, 0, 1, 30.0, 1e-6);
    approx(&e, 0, 3, 100.0, 0.0); // 100 passes of +1 from the 0 seed.
}

#[test]
fn sheet_calc_iterative_max_change_nonfinite_or_negative_runs_to_cap() {
    // Robustness of the convergence predicate `max_delta <= max_change` for a
    // pathological delta. A NEGATIVE max_change can never be satisfied by a
    // non-negative max_delta, so a convergent system runs to the full max_iter
    // (it still keeps its last — correct — iterate, just without early-exit).
    // This proves the loop terminates cleanly (the cap, not the delta, bounds
    // it) rather than looping forever or panicking.
    let mut e_neg = engine();
    e_neg.set_iterative(true, 200, -1.0);
    convergent_system(&mut e_neg);
    let r_neg = e_neg.enter(0, 0, 1, "=A1+5").unwrap();
    assert!(r_neg.circular.is_empty());
    // No early-exit possible -> the whole cycle is reported non-converged even
    // though the values are at the fixed point (the predicate, not accuracy,
    // decides the label). The stored values are still the correct fixed point.
    assert!(
        !r_neg.non_converged.is_empty(),
        "a negative max_change never early-exits — runs to the cap, reported non-converged"
    );
    approx(&e_neg, 0, 0, 25.0, 1e-6);

    // A NaN max_change: every `max_delta <= NaN` is false, so likewise no
    // early-exit — the cap bounds the loop and it terminates cleanly.
    let mut e_nan = engine();
    e_nan.set_iterative(true, 200, f64::NAN);
    convergent_system(&mut e_nan);
    let r_nan = e_nan.enter(0, 0, 1, "=A1+5").unwrap();
    assert!(r_nan.circular.is_empty());
    assert!(
        !r_nan.non_converged.is_empty(),
        "a NaN max_change never early-exits — runs to the cap"
    );
    approx(&e_nan, 0, 0, 25.0, 1e-6);
}
