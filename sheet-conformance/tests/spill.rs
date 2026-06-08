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

//! Dynamic-array SPILL conformance (spec §6.4, M1 spill track). End-to-end
//! through the FROZEN [`sheet_calc::Engine`] — the SAME path `sheet-js` drives:
//! a formula whose ROOT is a `returns_array` function (or an array literal)
//! MATERIALIZES its 2-D result into a spill region anchored at the formula's
//! cell. These tests cover the five `spill.yaml` rows:
//!
//! - `materialize` — `=SEQUENCE(3,1)` spills `A1:A3` (anchor holds the top-left).
//! - `collision`   — a non-empty cell in the target rectangle yields `#SPILL!`;
//!   removing the blocker reflows the spill.
//! - `ref-operator`— the spill-range operator `A1#` addresses the whole region
//!   (built as an `Expr::SpillRef` AST, since the parser does not yet LEX `A1#`
//!   — that is the parser track's job; the eval/materialization is ours).
//! - `cse-parse`   — a legacy CSE array literal `{1,2;3,4}` evaluates through the
//!   SAME spill machinery (built as an `Expr::Array` AST).
//! - `blocked-recalc` — editing a precedent reflows the spilled footprint.
//!
//! ## T1 scope (documented)
//!
//! A formula spills iff its ROOT is a `returns_array` call or an `Expr::Array`
//! literal; scalar formulas stay on the scalar path. The spill-range operator
//! `A1#` and array literals are exercised via the constructed AST through
//! [`SetInput::Formula`] because the parser does not yet emit `Expr::SpillRef`
//! / lex the `#` postfix (parser track). The eval + spill engine are ours.

use sheet_calc::{Engine, EngineConfig, SetInput};
use sheet_core::ast::{Expr, Formula, LitValue, OrderedF64};
use sheet_core::{CellError, CellRef, CellValue, FuncId, SheetModel};

// ---- harness ---------------------------------------------------------------

const SHEET: u16 = 0;

fn engine() -> Engine {
    let mut m = SheetModel::new();
    m.add_sheet("Sheet1");
    Engine::new(m, EngineConfig::default())
}

fn cr(row: u32, col: u32) -> CellRef {
    CellRef {
        sheet: SHEET,
        row,
        col,
        row_abs: false,
        col_abs: false,
    }
}

/// The stored value at `(row, col)` (Empty if blank).
fn value_at(e: &Engine, row: u32, col: u32) -> CellValue {
    e.model()
        .sheet(SHEET)
        .and_then(|ws| ws.cell(row, col))
        .map(|c| c.value.clone())
        .unwrap_or(CellValue::Empty)
}

fn func(name: &str) -> FuncId {
    sheet_core::funcs::lookup_func(name).unwrap_or_else(|| panic!("{name} not in registry"))
}

fn num_lit(n: f64) -> Expr {
    Expr::Lit(LitValue::Number(OrderedF64::new(n)))
}

/// Set a formula whose AST is built directly (for forms the parser cannot yet
/// emit — `Expr::SpillRef`, `Expr::Array`).
fn set_formula(e: &mut Engine, row: u32, col: u32, root: Expr) {
    e.set_cell(SHEET, row, col, SetInput::Formula(Formula { root }));
}

// ---- materialize -----------------------------------------------------------

#[test]
fn sheet_calc_spill_materialize() {
    // `=SEQUENCE(3,1)` at A1 spills A1:A3 = 1,2,3 (anchor holds the top-left).
    let mut e = engine();
    set_formula(
        &mut e,
        0,
        0,
        Expr::Func(func("SEQUENCE"), vec![num_lit(3.0), num_lit(1.0)]),
    );
    assert_eq!(value_at(&e, 0, 0), CellValue::Number(1.0));
    assert_eq!(value_at(&e, 1, 0), CellValue::Number(2.0));
    assert_eq!(value_at(&e, 2, 0), CellValue::Number(3.0));
    // The ledger records the 3x1 region anchored at A1.
    let rect = e.spills().region_of(cr(0, 0)).expect("A1 owns a region");
    assert_eq!((rect.row0, rect.col0, rect.row1, rect.col1), (0, 0, 2, 0));
    // Spilled (non-anchor) cells are engine-owned, not user formulas.
    assert!(e.spills().is_spilled_non_anchor(cr(1, 0)));
    assert!(e.spills().is_spilled_non_anchor(cr(2, 0)));
    assert!(!e.spills().is_spilled_non_anchor(cr(0, 0)));

    // A 2-D spill: `=SEQUENCE(2,2)` at C1 fills C1:D2 = 1,2;3,4.
    set_formula(
        &mut e,
        0,
        2,
        Expr::Func(func("SEQUENCE"), vec![num_lit(2.0), num_lit(2.0)]),
    );
    assert_eq!(value_at(&e, 0, 2), CellValue::Number(1.0));
    assert_eq!(value_at(&e, 0, 3), CellValue::Number(2.0));
    assert_eq!(value_at(&e, 1, 2), CellValue::Number(3.0));
    assert_eq!(value_at(&e, 1, 3), CellValue::Number(4.0));
}

// ---- collision -------------------------------------------------------------

#[test]
fn sheet_calc_spill_collision() {
    // Put a blocker at A2, then spill `=SEQUENCE(3,1)` at A1 -> #SPILL!.
    let mut e = engine();
    e.enter(SHEET, 1, 0, "99").unwrap(); // A2 = 99 (the blocker)
    set_formula(
        &mut e,
        0,
        0,
        Expr::Func(func("SEQUENCE"), vec![num_lit(3.0), num_lit(1.0)]),
    );
    // A1 is #SPILL!; the blocker is untouched; no region claimed.
    assert_eq!(value_at(&e, 0, 0), CellValue::Error(CellError::Spill));
    assert_eq!(value_at(&e, 1, 0), CellValue::Number(99.0));
    assert!(e.spills().region_of(cr(0, 0)).is_none());

    // Remove the blocker -> the spill reflows on the next recalc.
    e.set_cell(SHEET, 1, 0, SetInput::Empty);
    assert_eq!(value_at(&e, 0, 0), CellValue::Number(1.0));
    assert_eq!(value_at(&e, 1, 0), CellValue::Number(2.0));
    assert_eq!(value_at(&e, 2, 0), CellValue::Number(3.0));
    assert!(e.spills().region_of(cr(0, 0)).is_some());
}

// ---- ref-operator (A1#) ----------------------------------------------------

#[test]
fn sheet_calc_spill_ref_operator() {
    // A1 spills A1:A3 = 1,2,3; B1 = SUM(A1#) sums the whole spilled region = 6.
    let mut e = engine();
    set_formula(
        &mut e,
        0,
        0,
        Expr::Func(func("SEQUENCE"), vec![num_lit(3.0), num_lit(1.0)]),
    );
    // B1 = SUM(A1#) — the spill-range operator (constructed AST).
    let a1_spill = Expr::SpillRef(Box::new(Expr::Ref(cr(0, 0))));
    set_formula(&mut e, 0, 1, Expr::Func(func("SUM"), vec![a1_spill]));
    assert_eq!(value_at(&e, 0, 1), CellValue::Number(6.0));

    // A SpillRef in SCALAR position (the anchor of a multi-cell region) is
    // #VALUE! (documented): C1 = A1# directly (no enclosing range function).
    set_formula(&mut e, 0, 2, Expr::SpillRef(Box::new(Expr::Ref(cr(0, 0)))));
    assert_eq!(value_at(&e, 0, 2), CellValue::Error(CellError::Value));
}

// ---- cse-parse (array literal through the same machinery) -------------------

#[test]
fn sheet_calc_spill_cse_parse() {
    // A legacy CSE array literal `{1,2;3,4}` evaluates through the SAME spill
    // machinery: it spills a 2x2 block anchored at the formula cell.
    let mut e = engine();
    let lit = Expr::Array(vec![
        vec![num_lit(1.0), num_lit(2.0)],
        vec![num_lit(3.0), num_lit(4.0)],
    ]);
    set_formula(&mut e, 0, 0, lit);
    assert_eq!(value_at(&e, 0, 0), CellValue::Number(1.0));
    assert_eq!(value_at(&e, 0, 1), CellValue::Number(2.0));
    assert_eq!(value_at(&e, 1, 0), CellValue::Number(3.0));
    assert_eq!(value_at(&e, 1, 1), CellValue::Number(4.0));
    let rect = e
        .spills()
        .region_of(cr(0, 0))
        .expect("array literal spills");
    assert_eq!((rect.row1, rect.col1), (1, 1));
}

// ---- blocked-recalc (precedent edit reflows the footprint) ------------------

#[test]
fn sheet_calc_spill_blocked_recalc() {
    // A1 = a literal count; B1 = SEQUENCE(A1,1) spills B1:B(A1).
    let mut e = engine();
    e.enter(SHEET, 0, 0, "3").unwrap(); // A1 = 3
    set_formula(
        &mut e,
        0,
        1,
        Expr::Func(func("SEQUENCE"), vec![Expr::Ref(cr(0, 0)), num_lit(1.0)]),
    );
    // B1:B3 = 1,2,3.
    assert_eq!(value_at(&e, 0, 1), CellValue::Number(1.0));
    assert_eq!(value_at(&e, 1, 1), CellValue::Number(2.0));
    assert_eq!(value_at(&e, 2, 1), CellValue::Number(3.0));

    // Shrink the precedent: A1 = 2 -> the spill reflows to B1:B2, and the old
    // B3 spilled cell is cleared (a shrinking array leaves no stale cells).
    e.enter(SHEET, 0, 0, "2").unwrap();
    assert_eq!(value_at(&e, 0, 1), CellValue::Number(1.0));
    assert_eq!(value_at(&e, 1, 1), CellValue::Number(2.0));
    assert_eq!(value_at(&e, 2, 1), CellValue::Empty, "B3 must clear");
    let rect = e.spills().region_of(cr(0, 1)).expect("B1 still anchors");
    assert_eq!(rect.row1, 1);

    // Grow it back: A1 = 4 -> B1:B4 = 1,2,3,4.
    e.enter(SHEET, 0, 0, "4").unwrap();
    assert_eq!(value_at(&e, 3, 1), CellValue::Number(4.0));

    // Clearing the anchor clears the whole region (`set_cell Empty` at B1).
    e.set_cell(SHEET, 0, 1, SetInput::Empty);
    assert_eq!(value_at(&e, 0, 1), CellValue::Empty);
    assert_eq!(value_at(&e, 1, 1), CellValue::Empty);
    assert_eq!(value_at(&e, 2, 1), CellValue::Empty);
    assert_eq!(value_at(&e, 3, 1), CellValue::Empty);
    assert!(e.spills().region_of(cr(0, 1)).is_none());
}
