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

//! Dependency-graph, dirty-propagation, circular-reference, and volatility
//! conformance (spec §6.2). Drives the FROZEN [`sheet_calc::Engine`] surface
//! end-to-end (enter → commit → recalc) so the tests exercise the same path
//! `sheet-js` will. Test-fn names use the prefixes the
//! `registry/features/calc.yaml` rows point at so the coverage gate (§12.2)
//! finds them:
//! - `sheet_calc_graph_cell_edges`     (sheet.calc.graph.cell-edges)
//! - `sheet_calc_graph_range_invalidation` (sheet.calc.graph.range-invalidation)
//! - `sheet_calc_dirty_transitive`     (sheet.calc.dirty.transitive)
//! - `sheet_calc_circular`             (sheet.calc.circular)
//! - `sheet_calc_volatile`             (sheet.calc.volatile)

use sheet_calc::{Engine, EngineConfig};
use sheet_core::{CellError, CellRef, CellValue, SheetModel};

// ---- helpers ----

fn engine() -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    m.add_sheet("Sheet2");
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

fn val(e: &Engine, sheet: u16, row: u32, col: u32) -> CellValue {
    e.model()
        .sheet(sheet)
        .and_then(|ws| ws.cell(row, col))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

fn num(n: f64) -> CellValue {
    CellValue::Number(n)
}

// =================================================================
// sheet.calc.graph.cell-edges — per-cell edges from reference extraction
// =================================================================

#[test]
fn sheet_calc_graph_cell_edges() {
    let mut e = engine();
    // A1 = 10 ; B1 = A1 * 2 — B1 has a cell edge to A1.
    e.enter(0, 0, 0, "10").unwrap();
    e.enter(0, 0, 1, "=A1*2").unwrap();
    assert_eq!(val(&e, 0, 0, 1), num(20.0));

    // Writing A1 recalcs ONLY its dependent (B1 changes; nothing else).
    let r = e.enter(0, 0, 0, "100").unwrap();
    assert_eq!(val(&e, 0, 0, 1), num(200.0));
    assert!(
        r.changed.contains(&cr(0, 1)),
        "B1 must be in the changed set: {:?}",
        r.changed
    );

    // A cell with no dependents: writing it changes nothing downstream.
    e.enter(0, 5, 5, "7").unwrap();
    let r2 = e.enter(0, 5, 5, "8").unwrap();
    assert!(
        !r2.changed.iter().any(|c| *c == cr(0, 1)),
        "unrelated B1 must not recalc"
    );
}

#[test]
fn sheet_calc_graph_cell_edges_cross_sheet() {
    let mut e = engine();
    // Sheet2!A1 = 5 ; Sheet1!A1 = Sheet2!A1 + 1.
    e.enter(1, 0, 0, "5").unwrap();
    e.enter(0, 0, 0, "=Sheet2!A1+1").unwrap();
    assert_eq!(val(&e, 0, 0, 0), num(6.0));
    // Editing the cross-sheet precedent recalcs the dependent.
    e.enter(1, 0, 0, "9").unwrap();
    assert_eq!(val(&e, 0, 0, 0), num(10.0));
}

// =================================================================
// sheet.calc.graph.range-invalidation — range box bounding invalidation
// =================================================================

#[test]
fn sheet_calc_graph_range_invalidation() {
    let mut e = engine();
    // C1 = SUM(A1:A3).
    e.enter(0, 0, 0, "1").unwrap();
    e.enter(0, 1, 0, "2").unwrap();
    e.enter(0, 2, 0, "3").unwrap();
    e.enter(0, 0, 2, "=SUM(A1:A3)").unwrap();
    assert_eq!(val(&e, 0, 0, 2), num(6.0));

    // A write INSIDE the box dirties + recalcs C1.
    let r = e.enter(0, 1, 0, "20").unwrap();
    assert_eq!(val(&e, 0, 0, 2), num(24.0));
    assert!(
        r.changed.contains(&cr(0, 2)),
        "C1 must recalc on in-box write"
    );

    // A write OUTSIDE the box (A4) does NOT recalc C1.
    let before = val(&e, 0, 0, 2);
    let r2 = e.enter(0, 3, 0, "999").unwrap();
    assert_eq!(val(&e, 0, 0, 2), before, "C1 unchanged by out-of-box write");
    assert!(
        !r2.changed.contains(&cr(0, 2)),
        "C1 must not be in changed for an out-of-box write"
    );
}

// =================================================================
// sheet.calc.dirty.transitive — transitive dirty propagation
// =================================================================

#[test]
fn sheet_calc_dirty_transitive() {
    let mut e = engine();
    // A1 = 1 ; B1 = A1+1 ; C1 = B1+1 ; D1 = C1+1 (3-deep chain).
    e.enter(0, 0, 0, "1").unwrap();
    e.enter(0, 0, 1, "=A1+1").unwrap();
    e.enter(0, 0, 2, "=B1+1").unwrap();
    e.enter(0, 0, 3, "=C1+1").unwrap();
    assert_eq!(val(&e, 0, 0, 3), num(4.0));

    // One write at A1 dirties the whole chain transitively.
    let r = e.enter(0, 0, 0, "10").unwrap();
    assert_eq!(val(&e, 0, 0, 1), num(11.0));
    assert_eq!(val(&e, 0, 0, 2), num(12.0));
    assert_eq!(val(&e, 0, 0, 3), num(13.0));
    for c in [cr(0, 1), cr(0, 2), cr(0, 3)] {
        assert!(
            r.changed.contains(&c),
            "{c:?} must be transitively recalculated: {:?}",
            r.changed
        );
    }
}

// =================================================================
// sheet.calc.circular — cycle members store #REF! + reported on circular
// =================================================================

#[test]
fn sheet_calc_circular() {
    let mut e = engine();
    // A1 = B1 ; B1 = A1 — a 2-cycle.
    e.enter(0, 0, 0, "=B1").unwrap();
    let r = e.enter(0, 0, 1, "=A1").unwrap();

    // Both members report on `circular` and STORE #REF! (the wire encoding of
    // the internal Circular diagnostic — CellError has no Circular variant).
    assert!(r.circular.contains(&cr(0, 0)));
    assert!(r.circular.contains(&cr(0, 1)));
    assert_eq!(val(&e, 0, 0, 0), CellValue::Error(CellError::Ref));
    assert_eq!(val(&e, 0, 0, 1), CellValue::Error(CellError::Ref));

    // A longer cycle (3 members) is fully reported.
    let mut e3 = engine();
    e3.enter(0, 0, 0, "=A2").unwrap();
    e3.enter(0, 1, 0, "=A3").unwrap();
    let r3 = e3.enter(0, 2, 0, "=A1").unwrap();
    for c in [cr(0, 0), cr(1, 0), cr(2, 0)] {
        assert!(r3.circular.contains(&c), "{c:?} on the 3-cycle");
        assert_eq!(val(&e3, 0, c.row, c.col), CellValue::Error(CellError::Ref));
    }

    // Breaking the cycle clears it: re-enter B1 as a literal.
    let cleared = e.enter(0, 0, 1, "5").unwrap();
    assert!(cleared.circular.is_empty(), "cycle cleared on break");
    assert_eq!(val(&e, 0, 0, 1), num(5.0));
    assert_eq!(val(&e, 0, 0, 0), num(5.0), "A1=B1 now resolves to 5");
}

// =================================================================
// sheet.calc.volatile — volatiles reseed every pass; non-volatiles don't
// =================================================================

#[test]
fn sheet_calc_volatile() {
    // RAND() recomputes every pass; a non-volatile untouched formula does not.
    let mut e = engine();
    e.enter(0, 0, 0, "=RAND()").unwrap(); // volatile
    e.enter(0, 1, 0, "1").unwrap();
    e.enter(0, 1, 1, "=A2+1").unwrap(); // non-volatile, depends on A2

    let rand_pass1 = val(&e, 0, 0, 0);
    let b2_pass1 = val(&e, 0, 1, 1);

    // recalc_dirty with no edits: the volatile RAND reseeds and recomputes; the
    // non-volatile B2 (no dirty input) does NOT recompute.
    let r = e.recalc_dirty();
    let rand_pass2 = val(&e, 0, 0, 0);
    assert_ne!(rand_pass1, rand_pass2, "RAND must change across passes");
    assert!(
        r.changed.contains(&cr(0, 0)),
        "RAND cell must be in changed set"
    );
    assert_eq!(val(&e, 0, 1, 1), b2_pass1, "non-volatile B2 unchanged");
    assert!(
        !r.changed.contains(&cr(1, 1)),
        "non-volatile untouched cell must not recalc: {:?}",
        r.changed
    );

    // NOW() is volatile too: it reseeds and tracks set_now.
    let mut e2 = engine();
    e2.enter(0, 0, 0, "=NOW()").unwrap();
    e2.set_now(45000.0);
    e2.recalc_dirty();
    assert_eq!(val(&e2, 0, 0, 0), num(45000.0));
    e2.set_now(45001.0);
    e2.recalc_dirty();
    assert_eq!(
        val(&e2, 0, 0, 0),
        num(45001.0),
        "NOW tracks set_now each pass"
    );
}

#[test]
fn sheet_calc_volatile_deterministic_under_seed() {
    // Same config seed -> same RAND sequence across two fresh engines.
    let mk = || {
        let mut m = SheetModel::new();
        m.add_sheet("Sheet1");
        Engine::new(m, EngineConfig::default())
    };
    let mut a = mk();
    let mut b = mk();
    a.enter(0, 0, 0, "=RAND()").unwrap();
    b.enter(0, 0, 0, "=RAND()").unwrap();
    assert_eq!(
        val(&a, 0, 0, 0),
        val(&b, 0, 0, 0),
        "seeded RAND reproducible"
    );
    a.recalc_dirty();
    b.recalc_dirty();
    assert_eq!(
        val(&a, 0, 0, 0),
        val(&b, 0, 0, 0),
        "seeded RAND reproducible after a pass"
    );
}
