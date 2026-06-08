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

//! M1 dynamic-array (array) family conformance (spec §6.4 / §13 M1).
//! SELF-CONTAINED direct-dispatch tests: each case resolves the function name
//! through `sheet_core::funcs::lookup_func` and routes the call through
//! [`sheet_fn::dispatch_rich`] — the RICH (array) door, the same choke point a
//! spilling formula crosses in `sheet-calc`. The scalar [`sheet_fn::dispatch`]
//! door returns `#VALUE!` for these `returns_array` rows by construction (it has
//! no `-> CellValue` kernel for them); the rich door returns the 2-D block.
//!
//! Every kernel gets at least one `fn sheet_fn_array_<name>…` test (the prefix
//! the registry rows in `array.yaml` point at, which the coverage gate greps
//! for). Cases cover the happy 2-D shape, each named Excel ruling, and the
//! "ground #CALC! to a wire error" decisions (a degenerate shape → `#VALUE!`;
//! an empty FILTER with no `if_empty` → `#N/A`).

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::arg::{Arg, RangeView};
use sheet_fn::{dispatch, dispatch_rich, EvalCtx, FnResult};

// ---- harness ---------------------------------------------------------------

fn cell() -> CellRef {
    CellRef {
        sheet: 0,
        row: 0,
        col: 0,
        row_abs: false,
        col_abs: false,
    }
}

/// A deterministic context (fixed now-serial + seed) — RANDARRAY reproduces.
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cell(), 45000.5, 42)
}

/// Dispatch an array function by registry name through the RICH door.
fn call(name: &str, args: &[Arg]) -> FnResult {
    let id = sheet_core::funcs::lookup_func(name)
        .unwrap_or_else(|| panic!("function {name} not in registry"));
    dispatch_rich(id, args, &ctx())
}

/// Dispatch through the SCALAR door (to prove the by-construction `#VALUE!`).
fn call_scalar(name: &str, args: &[Arg]) -> CellValue {
    let id = sheet_core::funcs::lookup_func(name)
        .unwrap_or_else(|| panic!("function {name} not in registry"));
    dispatch(id, args, &ctx())
}

fn num(x: f64) -> Arg<'static> {
    Arg::Scalar(CellValue::Number(x))
}
fn txt(s: &str) -> Arg<'static> {
    Arg::Scalar(CellValue::from(s))
}
fn boolean(b: bool) -> Arg<'static> {
    Arg::Scalar(CellValue::Bool(b))
}
fn n(x: f64) -> CellValue {
    CellValue::Number(x)
}
fn t(s: &str) -> CellValue {
    CellValue::from(s)
}

/// Assert an `FnResult::Array` equals an expected row-major grid.
fn assert_array(got: &FnResult, want: &[&[CellValue]]) {
    match got {
        FnResult::Array(grid) => {
            let want: Vec<Vec<CellValue>> = want.iter().map(|r| r.to_vec()).collect();
            assert_eq!(grid, &want, "array mismatch");
        }
        other => panic!("expected Array {want:?}, got {other:?}"),
    }
}

/// Assert an `FnResult::Scalar(Error(e))` (the grounded error rulings).
fn assert_err(got: &FnResult, e: CellError) {
    assert_eq!(
        got,
        &FnResult::Scalar(CellValue::Error(e)),
        "expected #{e:?}"
    );
}

/// A row-major `RangeView` backing (owned cells the view borrows).
struct Grid {
    rows: u32,
    cols: u32,
    cells: Vec<CellValue>,
}
impl Grid {
    fn new(rows: u32, cols: u32, cells: Vec<CellValue>) -> Self {
        assert_eq!(cells.len() as u32, rows * cols);
        Grid { rows, cols, cells }
    }
    fn arg(&self) -> Arg<'_> {
        Arg::Range(RangeView::from_slice(
            cell(),
            self.rows,
            self.cols,
            &self.cells,
        ))
    }
}

// ---- SEQUENCE --------------------------------------------------------------

#[test]
fn sheet_fn_array_sequence() {
    // SEQUENCE(3) -> a 3x1 column 1,2,3 (row-major fill).
    assert_array(
        &call("SEQUENCE", &[num(3.0)]),
        &[&[n(1.0)], &[n(2.0)], &[n(3.0)]],
    );
    // SEQUENCE(2,3) -> 2x3 filled 1..6 row-major.
    assert_array(
        &call("SEQUENCE", &[num(2.0), num(3.0)]),
        &[&[n(1.0), n(2.0), n(3.0)], &[n(4.0), n(5.0), n(6.0)]],
    );
    // start/step: SEQUENCE(2,2,10,5) -> 10,15;20,25.
    assert_array(
        &call("SEQUENCE", &[num(2.0), num(2.0), num(10.0), num(5.0)]),
        &[&[n(10.0), n(15.0)], &[n(20.0), n(25.0)]],
    );
    // A non-positive shape grounds #CALC! to #VALUE!.
    assert_err(&call("SEQUENCE", &[num(0.0)]), CellError::Value);
    assert_err(&call("SEQUENCE", &[num(-2.0)]), CellError::Value);
    // The scalar door returns #VALUE! for this returns_array row by construction.
    assert_eq!(
        call_scalar("SEQUENCE", &[num(3.0)]),
        CellValue::Error(CellError::Value)
    );
    // Arity guard (min 1) fires through the rich door too.
    assert_err(&call("SEQUENCE", &[]), CellError::Value);
}

// ---- TRANSPOSE -------------------------------------------------------------

#[test]
fn sheet_fn_array_transpose() {
    // 2x3 -> 3x2: out[c][r] = in[r][c].
    let g = Grid::new(2, 3, vec![n(1.0), n(2.0), n(3.0), n(4.0), n(5.0), n(6.0)]);
    assert_array(
        &call("TRANSPOSE", &[g.arg()]),
        &[&[n(1.0), n(4.0)], &[n(2.0), n(5.0)], &[n(3.0), n(6.0)]],
    );
    // A 1x1 scalar transposes to itself.
    assert_array(&call("TRANSPOSE", &[num(9.0)]), &[&[n(9.0)]]);
}

// ---- UNIQUE ----------------------------------------------------------------

#[test]
fn sheet_fn_array_unique() {
    // First-seen distinct rows (a single column here).
    let g = Grid::new(4, 1, vec![n(1.0), n(2.0), n(1.0), n(3.0)]);
    assert_array(
        &call("UNIQUE", &[g.arg()]),
        &[&[n(1.0)], &[n(2.0)], &[n(3.0)]],
    );
    // exactly_once = TRUE keeps only the singletons (1 appears twice -> dropped).
    let g2 = Grid::new(4, 1, vec![n(1.0), n(2.0), n(1.0), n(3.0)]);
    assert_array(
        &call("UNIQUE", &[g2.arg(), boolean(false), boolean(true)]),
        &[&[n(2.0)], &[n(3.0)]],
    );
    // Case-insensitive text equality (the frozen coerce::compare order).
    let g3 = Grid::new(3, 1, vec![t("a"), t("A"), t("b")]);
    assert_array(&call("UNIQUE", &[g3.arg()]), &[&[t("a")], &[t("b")]]);
}

// ---- SORT ------------------------------------------------------------------

#[test]
fn sheet_fn_array_sort() {
    // Ascending by the (default) first key column.
    let g = Grid::new(3, 1, vec![n(3.0), n(1.0), n(2.0)]);
    assert_array(
        &call("SORT", &[g.arg()]),
        &[&[n(1.0)], &[n(2.0)], &[n(3.0)]],
    );
    // Descending (order = -1).
    let g2 = Grid::new(3, 1, vec![n(3.0), n(1.0), n(2.0)]);
    assert_array(
        &call("SORT", &[g2.arg(), num(1.0), num(-1.0)]),
        &[&[n(3.0)], &[n(2.0)], &[n(1.0)]],
    );
    // A bad order value -> #VALUE!.
    let g3 = Grid::new(2, 1, vec![n(1.0), n(2.0)]);
    assert_err(
        &call("SORT", &[g3.arg(), num(1.0), num(2.0)]),
        CellError::Value,
    );
}

// ---- SORTBY ----------------------------------------------------------------

#[test]
fn sheet_fn_array_sortby() {
    // Sort the data column by a parallel key column (ascending).
    let data = Grid::new(3, 1, vec![t("c"), t("a"), t("b")]);
    let key = Grid::new(3, 1, vec![n(3.0), n(1.0), n(2.0)]);
    assert_array(
        &call("SORTBY", &[data.arg(), key.arg()]),
        &[&[t("a")], &[t("b")], &[t("c")]],
    );
    // A by_array of the wrong length -> #VALUE!.
    let data2 = Grid::new(3, 1, vec![t("c"), t("a"), t("b")]);
    let badkey = Grid::new(2, 1, vec![n(1.0), n(2.0)]);
    assert_err(
        &call("SORTBY", &[data2.arg(), badkey.arg()]),
        CellError::Value,
    );
}

// ---- FILTER ----------------------------------------------------------------

#[test]
fn sheet_fn_array_filter() {
    // Keep rows whose aligned mask entry is truthy (1/0 column).
    let data = Grid::new(3, 1, vec![n(10.0), n(20.0), n(30.0)]);
    let mask = Grid::new(3, 1, vec![n(1.0), n(0.0), n(1.0)]);
    assert_array(
        &call("FILTER", &[data.arg(), mask.arg()]),
        &[&[n(10.0)], &[n(30.0)]],
    );
    // An empty result with no if_empty grounds #CALC! to #N/A.
    let data2 = Grid::new(2, 1, vec![n(1.0), n(2.0)]);
    let mask2 = Grid::new(2, 1, vec![boolean_v(false), boolean_v(false)]);
    assert_err(&call("FILTER", &[data2.arg(), mask2.arg()]), CellError::Na);
    // An empty result WITH if_empty returns the fallback as a 1x1 block.
    let data3 = Grid::new(2, 1, vec![n(1.0), n(2.0)]);
    let mask3 = Grid::new(2, 1, vec![boolean_v(false), boolean_v(false)]);
    assert_array(
        &call("FILTER", &[data3.arg(), mask3.arg(), txt("none")]),
        &[&[t("none")]],
    );
}

fn boolean_v(b: bool) -> CellValue {
    CellValue::Bool(b)
}

// ---- TEXTSPLIT -------------------------------------------------------------

#[test]
fn sheet_fn_array_textsplit() {
    // Column delimiter only -> a 1xN row.
    assert_array(
        &call("TEXTSPLIT", &[txt("a,b,c"), txt(",")]),
        &[&[t("a"), t("b"), t("c")]],
    );
    // Row + column delimiters -> a 2-D block.
    assert_array(
        &call("TEXTSPLIT", &[txt("a,b;c,d"), txt(","), txt(";")]),
        &[&[t("a"), t("b")], &[t("c"), t("d")]],
    );
    // An empty column delimiter is #VALUE!.
    assert_err(&call("TEXTSPLIT", &[txt("a,b"), txt("")]), CellError::Value);
}

// ---- RANDARRAY (volatile) --------------------------------------------------

#[test]
fn sheet_fn_array_randarray() {
    // Default RANDARRAY() -> a 1x1 block with a value in [0,1).
    match call("RANDARRAY", &[]) {
        FnResult::Array(g) => {
            assert_eq!(g.len(), 1);
            assert_eq!(g[0].len(), 1);
            match g[0][0] {
                CellValue::Number(x) => assert!((0.0..1.0).contains(&x), "out of [0,1): {x}"),
                ref other => panic!("expected Number, got {other:?}"),
            }
        }
        other => panic!("expected 1x1 Array, got {other:?}"),
    }
    // Shape: RANDARRAY(2,3) -> 2x3 integers in [1,6] when integer=TRUE.
    match call(
        "RANDARRAY",
        &[num(2.0), num(3.0), num(1.0), num(6.0), boolean(true)],
    ) {
        FnResult::Array(g) => {
            assert_eq!(g.len(), 2);
            assert!(g.iter().all(|r| r.len() == 3));
            for row in &g {
                for v in row {
                    match v {
                        CellValue::Number(x) => {
                            assert!((1.0..=6.0).contains(x), "int out of [1,6]: {x}");
                            assert_eq!(x.fract(), 0.0, "expected an integer, got {x}");
                        }
                        other => panic!("expected Number, got {other:?}"),
                    }
                }
            }
        }
        other => panic!("expected 2x3 Array, got {other:?}"),
    }
    // Deterministic under a fixed seed: the same ctx seed reproduces the draw.
    assert_eq!(call("RANDARRAY", &[]), call("RANDARRAY", &[]));
    // min > max -> #VALUE!.
    assert_err(
        &call("RANDARRAY", &[num(1.0), num(1.0), num(5.0), num(1.0)]),
        CellError::Value,
    );
}
