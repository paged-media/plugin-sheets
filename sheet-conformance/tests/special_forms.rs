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

//! Evaluator special-form conformance (spec §13 M2 Phase A): `OFFSET`,
//! `INDIRECT`, `FORMULATEXT`, `ISFORMULA`. These functions READ THE MODEL (a
//! reference/formula by address) and so cannot be pure `fn(&[Arg], &EvalCtx)`
//! kernels — they are handled in `sheet-calc/eval.rs` BEFORE dispatch (the
//! registry rows carry `special_form: true`; the pure dispatch door returns
//! `#NAME?` for them). The REAL behavior is therefore tested HERE, through the
//! FROZEN [`sheet_calc::Engine`] surface (`enter` → parse → commit → recalc),
//! the same path `sheet-js` takes. Test-fn names use the prefixes the
//! `registry/functions/{lookup2,info2}.yaml` rows point at so the §12.2 coverage
//! gate finds them:
//! - `sheet_fn_lookup_offset`      (sheet.fn.lookup.offset)
//! - `sheet_fn_lookup_indirect`    (sheet.fn.lookup.indirect)
//! - `sheet_fn_lookup_formulatext` (sheet.fn.lookup.formulatext)
//! - `sheet_fn_info_isformula`     (sheet.fn.info.isformula)

use sheet_calc::{Engine, EngineConfig};
use sheet_core::{CellError, CellValue, SheetModel};

// ---- helpers ----

/// A two-sheet engine (`Sheet1`, `Sheet2`) so INDIRECT cross-sheet refs resolve.
fn engine() -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    m.add_sheet("Sheet2");
    Engine::new(m, EngineConfig::default())
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
fn txt(s: &str) -> CellValue {
    CellValue::from(s)
}
fn err(e: CellError) -> CellValue {
    CellValue::Error(e)
}

// ================= OFFSET =================

#[test]
fn sheet_fn_lookup_offset_single_cell() {
    let mut e = engine();
    // A1=10, B2=20, C3=30 (so the offset target carries a distinct value).
    e.enter(0, 0, 0, "10").unwrap();
    e.enter(0, 1, 1, "20").unwrap();
    e.enter(0, 2, 2, "30").unwrap();
    // OFFSET(A1, 1, 1) -> B2 = 20.
    e.enter(0, 5, 0, "=OFFSET(A1,1,1)").unwrap();
    assert_eq!(val(&e, 0, 5, 0), num(20.0));
    // OFFSET(A1, 2, 2) -> C3 = 30.
    e.enter(0, 5, 1, "=OFFSET(A1,2,2)").unwrap();
    assert_eq!(val(&e, 0, 5, 1), num(30.0));
    // Zero offset reads the base cell itself.
    e.enter(0, 5, 2, "=OFFSET(B2,0,0)").unwrap();
    assert_eq!(val(&e, 0, 5, 2), num(20.0));
}

#[test]
fn sheet_fn_lookup_offset_negative_offset_and_ref_error() {
    let mut e = engine();
    e.enter(0, 0, 0, "10").unwrap(); // A1
    e.enter(0, 2, 2, "30").unwrap(); // C3
                                     // OFFSET(C3, -2, -2) -> A1 = 10.
    e.enter(0, 5, 0, "=OFFSET(C3,-2,-2)").unwrap();
    assert_eq!(val(&e, 0, 5, 0), num(10.0));
    // Off the top-left of the grid -> #REF!.
    e.enter(0, 5, 1, "=OFFSET(A1,-1,0)").unwrap();
    assert_eq!(val(&e, 0, 5, 1), err(CellError::Ref));
    e.enter(0, 5, 2, "=OFFSET(A1,0,-1)").unwrap();
    assert_eq!(val(&e, 0, 5, 2), err(CellError::Ref));
}

#[test]
fn sheet_fn_lookup_offset_multicell_in_scalar_and_as_range_arg() {
    let mut e = engine();
    // A1:A3 = 1,2,3.
    e.enter(0, 0, 0, "1").unwrap();
    e.enter(0, 1, 0, "2").unwrap();
    e.enter(0, 2, 0, "3").unwrap();
    // A multi-cell OFFSET in a SCALAR slot is #VALUE! (range-in-scalar ruling).
    e.enter(0, 5, 0, "=OFFSET(A1,0,0,3,1)").unwrap();
    assert_eq!(val(&e, 0, 5, 0), err(CellError::Value));
    // The SAME OFFSET feeding a range-aware outer fn sees the whole 3x1 area.
    e.enter(0, 5, 1, "=SUM(OFFSET(A1,0,0,3,1))").unwrap();
    assert_eq!(val(&e, 0, 5, 1), num(6.0));
    // Resized window: SUM(OFFSET(A1,1,0,2,1)) = A2+A3 = 5.
    e.enter(0, 5, 2, "=SUM(OFFSET(A1,1,0,2,1))").unwrap();
    assert_eq!(val(&e, 0, 5, 2), num(5.0));
}

#[test]
fn sheet_fn_lookup_offset_volatile_recalcs() {
    let mut e = engine();
    e.enter(0, 0, 0, "10").unwrap(); // A1
    e.enter(0, 1, 1, "20").unwrap(); // B2
    e.enter(0, 5, 0, "=OFFSET(A1,1,1)").unwrap();
    assert_eq!(val(&e, 0, 5, 0), num(20.0));
    // Editing the target cell flows through to the OFFSET result (the registry
    // marks OFFSET volatile; the dependent recalcs).
    e.enter(0, 1, 1, "99").unwrap();
    assert_eq!(val(&e, 0, 5, 0), num(99.0));
}

// ================= INDIRECT =================

#[test]
fn sheet_fn_lookup_indirect_a1_and_concat() {
    let mut e = engine();
    e.enter(0, 0, 0, "42").unwrap(); // A1
                                     // INDIRECT("A1") reads A1.
    e.enter(0, 5, 0, "=INDIRECT(\"A1\")").unwrap();
    assert_eq!(val(&e, 0, 5, 0), num(42.0));
    // A computed address: INDIRECT("A"&1) -> A1.
    e.enter(0, 5, 1, "=INDIRECT(\"A\"&1)").unwrap();
    assert_eq!(val(&e, 0, 5, 1), num(42.0));
    // A range address summed by an outer fn.
    e.enter(0, 1, 0, "8").unwrap(); // A2
    e.enter(0, 5, 2, "=SUM(INDIRECT(\"A1:A2\"))").unwrap();
    assert_eq!(val(&e, 0, 5, 2), num(50.0));
}

#[test]
fn sheet_fn_lookup_indirect_cross_sheet_and_errors() {
    let mut e = engine();
    // Seed Sheet2!A1.
    e.enter(1, 0, 0, "7").unwrap();
    e.enter(0, 5, 0, "=INDIRECT(\"Sheet2!A1\")").unwrap();
    assert_eq!(val(&e, 0, 5, 0), num(7.0));
    // Unparseable address -> #REF!.
    e.enter(0, 5, 1, "=INDIRECT(\"not a ref\")").unwrap();
    assert_eq!(val(&e, 0, 5, 1), err(CellError::Ref));
    // R1C1 mode (a1 = FALSE) is a documented T2 limitation -> #REF!.
    e.enter(0, 5, 2, "=INDIRECT(\"A1\",FALSE)").unwrap();
    assert_eq!(val(&e, 0, 5, 2), err(CellError::Ref));
}

// ================= FORMULATEXT =================

#[test]
fn sheet_fn_lookup_formulatext_prints_with_equals() {
    let mut e = engine();
    e.enter(0, 0, 0, "2").unwrap(); // A1
    e.enter(0, 1, 0, "3").unwrap(); // A2
    e.enter(0, 2, 0, "=A1+A2").unwrap(); // A3 is a formula cell
                                         // FORMULATEXT(A3) returns the printed formula WITH a leading '='.
    e.enter(0, 5, 0, "=FORMULATEXT(A3)").unwrap();
    assert_eq!(val(&e, 0, 5, 0), txt("=A1+A2"));
}

#[test]
fn sheet_fn_lookup_formulatext_non_formula_is_na() {
    let mut e = engine();
    e.enter(0, 0, 0, "42").unwrap(); // A1 is a literal (no formula)
    e.enter(0, 5, 0, "=FORMULATEXT(A1)").unwrap();
    assert_eq!(val(&e, 0, 5, 0), err(CellError::Na));
    // A blank cell is likewise #N/A.
    e.enter(0, 5, 1, "=FORMULATEXT(Z99)").unwrap();
    assert_eq!(val(&e, 0, 5, 1), err(CellError::Na));
}

// ================= ISFORMULA =================

#[test]
fn sheet_fn_info_isformula_true_and_false() {
    let mut e = engine();
    e.enter(0, 0, 0, "42").unwrap(); // A1 literal
    e.enter(0, 1, 0, "=A1*2").unwrap(); // A2 formula
                                        // ISFORMULA(A2) is TRUE (A2 stores a formula).
    e.enter(0, 5, 0, "=ISFORMULA(A2)").unwrap();
    assert_eq!(val(&e, 0, 5, 0), CellValue::Bool(true));
    // ISFORMULA(A1) is FALSE (A1 is a literal).
    e.enter(0, 5, 1, "=ISFORMULA(A1)").unwrap();
    assert_eq!(val(&e, 0, 5, 1), CellValue::Bool(false));
    // ISFORMULA(blank) is FALSE.
    e.enter(0, 5, 2, "=ISFORMULA(Z99)").unwrap();
    assert_eq!(val(&e, 0, 5, 2), CellValue::Bool(false));
}

#[test]
fn sheet_fn_info_isformula_tracks_edits() {
    let mut e = engine();
    e.enter(0, 0, 0, "5").unwrap(); // A1 literal
    e.enter(0, 5, 0, "=ISFORMULA(A1)").unwrap();
    assert_eq!(val(&e, 0, 5, 0), CellValue::Bool(false));
    // Turn A1 into a formula — ISFORMULA must flip (it is not volatile, but A1
    // is a direct precedent of the ISFORMULA cell, so the edit dirties it).
    e.enter(0, 0, 0, "=1+1").unwrap();
    assert_eq!(val(&e, 0, 5, 0), CellValue::Bool(true));
}

// ================= SUBTOTAL / AGGREGATE nested exclusion (FINDING 3) =========
//
// Excel: SUBTOTAL/AGGREGATE EXCLUDE cells in their ranges that are themselves
// SUBTOTAL/AGGREGATE results (ECMA-376 §18.17.7) — the classic total-row case
// `SUBTOTAL(9, A1:A6)` where `A6 = SUBTOTAL(9, A1:A5)` must NOT double-count.
// A pure kernel can't see precedent formulas, so the evaluator masks the
// nested-result cells to blank during range materialization (eval.rs
// plan_args). These pin that behaviour through the frozen Engine surface.

/// The headline regression: a nested SUBTOTAL in the range is excluded, so the
/// outer SUBTOTAL sums the leaf values once (15), not 15+15=30 (the pre-fix
/// silently-wrong scalar).
#[test]
fn sheet_fn_math_subtotal_excludes_nested() {
    let mut e = engine();
    // A1:A5 = 1,2,3,4,5 (sum 15). A6 is a NESTED subtotal over A1:A5.
    for (r, v) in [(0, "1"), (1, "2"), (2, "3"), (3, "4"), (4, "5")] {
        e.enter(0, r, 0, v).unwrap();
    }
    e.enter(0, 5, 0, "=SUBTOTAL(9,A1:A5)").unwrap(); // A6 = 15 (nested)
    assert_eq!(val(&e, 0, 5, 0), num(15.0));

    // SUBTOTAL(9, A1:A6) must EXCLUDE A6 (a nested subtotal) → 15, not 30.
    e.enter(0, 7, 0, "=SUBTOTAL(9,A1:A6)").unwrap(); // A8
    assert_eq!(val(&e, 0, 7, 0), num(15.0));

    // COUNT (function_num 2) also excludes the nested cell: A1:A6 has 6
    // populated cells but the nested A6 is masked → 5 numeric cells.
    e.enter(0, 8, 0, "=SUBTOTAL(2,A1:A6)").unwrap(); // A9
    assert_eq!(val(&e, 0, 8, 0), num(5.0));

    // The 101-111 form excludes nested results identically (T2: no hidden-row
    // distinction, but the nested-exclusion is independent of that).
    e.enter(0, 9, 0, "=SUBTOTAL(109,A1:A6)").unwrap(); // A10
    assert_eq!(val(&e, 0, 9, 0), num(15.0));
}

/// AGGREGATE excludes nested SUBTOTAL/AGGREGATE results too (same eval-side
/// masking), for every supported option.
#[test]
fn sheet_fn_math_subtotal_excludes_nested_aggregate() {
    let mut e = engine();
    for (r, v) in [(0, "10"), (1, "20"), (2, "30")] {
        e.enter(0, r, 0, v).unwrap();
    }
    e.enter(0, 3, 0, "=AGGREGATE(9,0,A1:A3)").unwrap(); // A4 = 60 (nested)
    assert_eq!(val(&e, 0, 3, 0), num(60.0));

    // AGGREGATE(9, 0, A1:A4) excludes the nested A4 → 60, not 120.
    e.enter(0, 5, 0, "=AGGREGATE(9,0,A1:A4)").unwrap(); // A6
    assert_eq!(val(&e, 0, 5, 0), num(60.0));

    // A SUBTOTAL nested inside an AGGREGATE range is also excluded.
    e.enter(0, 6, 0, "=SUBTOTAL(9,A1:A3)").unwrap(); // A7 = 60 (nested subtotal)
    e.enter(0, 8, 0, "=AGGREGATE(9,0,A1:A3,A7)").unwrap(); // A9 = 60, A7 excluded
    assert_eq!(val(&e, 0, 8, 0), num(60.0));
}

/// A plain (non-subtotal) formula cell in the range is NOT excluded — only
/// cells whose formula ROOT is a SUBTOTAL/AGGREGATE are masked. This pins the
/// boundary: `=SUM(...)` results still participate.
#[test]
fn sheet_fn_math_subtotal_includes_plain_formula_cells() {
    let mut e = engine();
    for (r, v) in [(0, "1"), (1, "2"), (2, "3")] {
        e.enter(0, r, 0, v).unwrap();
    }
    e.enter(0, 3, 0, "=SUM(A1:A3)").unwrap(); // A4 = 6 (a SUM, not a SUBTOTAL)
    assert_eq!(val(&e, 0, 3, 0), num(6.0));

    // SUBTOTAL over A1:A4 INCLUDES the SUM cell (it is not a subtotal result):
    // 1+2+3+6 = 12.
    e.enter(0, 5, 0, "=SUBTOTAL(9,A1:A4)").unwrap(); // A6
    assert_eq!(val(&e, 0, 5, 0), num(12.0));

    // A SUBTOTAL buried in a larger expression (`=SUBTOTAL(...)+0`) is NOT a
    // bare subtotal result, so it participates (Excel-matching boundary).
    e.enter(0, 6, 0, "=SUBTOTAL(9,A1:A3)+0").unwrap(); // A7 = 6, root is Binary(+)
    e.enter(0, 8, 0, "=SUBTOTAL(9,A1:A3,A7)").unwrap(); // A9 = 6 + 6 = 12
    assert_eq!(val(&e, 0, 8, 0), num(12.0));
}
