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

//! Lookup-family conformance (spec §7, §11 T0): `VLOOKUP`, `HLOOKUP`,
//! `INDEX`, `MATCH`, `CHOOSE`, `ROW`, `COLUMN`. Self-contained
//! **direct-dispatch** tests — each resolves a function id via
//! [`sheet_core::funcs::lookup_func`] and routes through the frozen
//! [`sheet_fn::dispatch`], so they exercise the SAME path a formula
//! evaluation takes (the registry arity guard is included: an arity
//! violation surfaces as `#VALUE!` from the dispatch table, *not* the
//! kernel). Test-fn names use the prefix the registry rows point at
//! (`sheet_fn_lookup_<name>`) so the coverage gate (§12.2) finds them.
//!
//! Rulings exercised (ECMA-376 §18.17.7; mirrors
//! `sheet_fn::families::lookup` module docs):
//! - approximate match (default TRUE / `MATCH` type 1) = largest value `<=`
//!   key, assumed sorted ascending; below the first value → `#N/A`;
//! - exact match (FALSE / type 0) = equality with `*`/`?` wildcards for text
//!   keys; not found → `#N/A`;
//! - `MATCH` type `-1` = smallest value `>=` key, assumed sorted descending;
//! - 1-based indices; out of bounds → `#REF!`; `col/row_index < 1` →
//!   `#VALUE!`; `INDEX` index 0 → `#VALUE!` (T0 degrade, no array form);
//! - `ROW`/`COLUMN` are reference-arg: range origin (`origin.row+1` /
//!   `.col+1`) with an arg, current cell when omitted;
//! - scalar-argument errors propagate (first-error-wins).

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::{dispatch, Arg, EvalCtx, RangeView};

// ---- helpers ----

fn cr(row: u32, col: u32) -> CellRef {
    CellRef {
        sheet: 0,
        row,
        col,
        row_abs: false,
        col_abs: false,
    }
}

/// A deterministic context (the §7 convention: fixed clock + seed). The
/// current cell `D11` (row 10, col 3, 0-based) is the no-arg ROW/COLUMN
/// answer: ROW()→11, COLUMN()→4.
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cr(10, 3), 45000.5, 42)
}

/// Dispatch a function by registry NAME through the frozen table.
fn call(name: &str, args: &[Arg], ctx: &EvalCtx) -> CellValue {
    let id = sheet_core::funcs::lookup_func(name)
        .unwrap_or_else(|| panic!("lookup_func({name}) returned None — registry row missing"));
    dispatch(id, args, ctx)
}

fn num(n: f64) -> CellValue {
    CellValue::Number(n)
}
fn txt(s: &str) -> CellValue {
    CellValue::from(s)
}
fn b(v: bool) -> CellValue {
    CellValue::Bool(v)
}
fn err(e: CellError) -> CellValue {
    CellValue::Error(e)
}
fn s(v: CellValue) -> Arg<'static> {
    Arg::Scalar(v)
}

// ================= VLOOKUP =================

#[test]
fn sheet_fn_lookup_vlookup_basic() {
    let c = ctx();
    // A 3x2 table: keys 1,2,3 in col 0; labels in col 1.
    let cells = [
        num(1.0),
        txt("one"),
        num(2.0),
        txt("two"),
        num(3.0),
        txt("three"),
    ];
    // Exact (range_lookup FALSE): key 2 -> "two" from col 2.
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(
        call(
            "VLOOKUP",
            &[s(num(2.0)), Arg::Range(v), s(num(2.0)), s(b(false))],
            &c
        ),
        txt("two")
    );
    // col_index 1 returns the key column itself.
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(
        call(
            "VLOOKUP",
            &[s(num(3.0)), Arg::Range(v), s(num(1.0)), s(b(false))],
            &c
        ),
        num(3.0)
    );
}

#[test]
fn sheet_fn_lookup_vlookup_approximate_default() {
    let c = ctx();
    // Sorted ascending: 10,20,30. Default range_lookup TRUE = largest <= key.
    let cells = [
        num(10.0),
        txt("a"),
        num(20.0),
        txt("b"),
        num(30.0),
        txt("c"),
    ];
    // key 25 -> floor 20 -> "b" (approximate; arg omitted = TRUE default).
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(
        call("VLOOKUP", &[s(num(25.0)), Arg::Range(v), s(num(2.0))], &c),
        txt("b")
    );
    // key exactly 30 -> "c".
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(
        call("VLOOKUP", &[s(num(30.0)), Arg::Range(v), s(num(2.0))], &c),
        txt("c")
    );
    // key below the first value (5 < 10) -> #N/A.
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(
        call("VLOOKUP", &[s(num(5.0)), Arg::Range(v), s(num(2.0))], &c),
        err(CellError::Na)
    );
}

#[test]
fn sheet_fn_lookup_vlookup_exact_not_found_and_wildcard() {
    let c = ctx();
    let cells = [
        txt("apple"),
        num(1.0),
        txt("banana"),
        num(2.0),
        txt("cherry"),
        num(3.0),
    ];
    // Exact, missing key -> #N/A.
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(
        call(
            "VLOOKUP",
            &[s(txt("grape")), Arg::Range(v), s(num(2.0)), s(b(false))],
            &c
        ),
        err(CellError::Na)
    );
    // Wildcard text key in exact mode: "ban*" matches "banana" -> 2.
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(
        call(
            "VLOOKUP",
            &[s(txt("ban*")), Arg::Range(v), s(num(2.0)), s(b(false))],
            &c
        ),
        num(2.0)
    );
    // Case-insensitive exact: "APPLE" matches "apple" -> 1.
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(
        call(
            "VLOOKUP",
            &[s(txt("APPLE")), Arg::Range(v), s(num(2.0)), s(b(false))],
            &c
        ),
        num(1.0)
    );
}

#[test]
fn sheet_fn_lookup_vlookup_oob_and_error_and_arity() {
    let c = ctx();
    let cells = [num(1.0), txt("one"), num(2.0), txt("two")];
    // col_index past the table width (3 > 2 cols) -> #REF!.
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call(
            "VLOOKUP",
            &[s(num(1.0)), Arg::Range(v), s(num(3.0)), s(b(false))],
            &c
        ),
        err(CellError::Ref)
    );
    // col_index < 1 -> #VALUE!.
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call(
            "VLOOKUP",
            &[s(num(1.0)), Arg::Range(v), s(num(0.0)), s(b(false))],
            &c
        ),
        err(CellError::Value)
    );
    // Error in the key propagates.
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call(
            "VLOOKUP",
            &[s(err(CellError::Div0)), Arg::Range(v), s(num(2.0))],
            &c
        ),
        err(CellError::Div0)
    );
    // Arity: 2 args (< min 3) -> #VALUE! (from the dispatch arity guard).
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call("VLOOKUP", &[s(num(1.0)), Arg::Range(v)], &c),
        err(CellError::Value)
    );
}

// ================= HLOOKUP =================

#[test]
fn sheet_fn_lookup_hlookup_basic() {
    let c = ctx();
    // A 2x3 table: keys 1,2,3 in row 0; labels in row 1.
    let cells = [
        num(1.0),
        num(2.0),
        num(3.0),
        txt("one"),
        txt("two"),
        txt("three"),
    ];
    // Exact: key 3 -> row_index 2 -> "three".
    let v = RangeView::from_slice(cr(0, 0), 2, 3, &cells);
    assert_eq!(
        call(
            "HLOOKUP",
            &[s(num(3.0)), Arg::Range(v), s(num(2.0)), s(b(false))],
            &c
        ),
        txt("three")
    );
}

#[test]
fn sheet_fn_lookup_hlookup_approximate_and_na() {
    let c = ctx();
    let cells = [
        num(10.0),
        num(20.0),
        num(30.0),
        txt("a"),
        txt("b"),
        txt("c"),
    ];
    // Approximate default: key 22 -> floor 20 -> row 2 -> "b".
    let v = RangeView::from_slice(cr(0, 0), 2, 3, &cells);
    assert_eq!(
        call("HLOOKUP", &[s(num(22.0)), Arg::Range(v), s(num(2.0))], &c),
        txt("b")
    );
    // Below first value -> #N/A.
    let v = RangeView::from_slice(cr(0, 0), 2, 3, &cells);
    assert_eq!(
        call("HLOOKUP", &[s(num(1.0)), Arg::Range(v), s(num(2.0))], &c),
        err(CellError::Na)
    );
    // row_index past the table height (3 > 2) -> #REF!.
    let v = RangeView::from_slice(cr(0, 0), 2, 3, &cells);
    assert_eq!(
        call("HLOOKUP", &[s(num(20.0)), Arg::Range(v), s(num(3.0))], &c),
        err(CellError::Ref)
    );
}

#[test]
fn sheet_fn_lookup_hlookup_coercion_and_arity() {
    let c = ctx();
    let cells = [num(1.0), num(2.0), txt("x"), txt("y")];
    // Index given as text "2" coerces to 2 -> "y".
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call(
            "HLOOKUP",
            &[s(num(2.0)), Arg::Range(v), s(txt("2")), s(b(false))],
            &c
        ),
        txt("y")
    );
    // Arity: 5 args (> max 4) -> #VALUE!.
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call(
            "HLOOKUP",
            &[
                s(num(1.0)),
                Arg::Range(v),
                s(num(1.0)),
                s(b(false)),
                s(num(9.0)),
            ],
            &c
        ),
        err(CellError::Value)
    );
}

// ================= INDEX =================

#[test]
fn sheet_fn_lookup_index_basic() {
    let c = ctx();
    // 2x3 grid.
    let cells = [
        num(11.0),
        num(12.0),
        num(13.0),
        num(21.0),
        num(22.0),
        num(23.0),
    ];
    // (row 2, col 3) -> 23.
    let v = RangeView::from_slice(cr(0, 0), 2, 3, &cells);
    assert_eq!(
        call("INDEX", &[Arg::Range(v), s(num(2.0)), s(num(3.0))], &c),
        num(23.0)
    );
    // (row 1, col 1) -> 11.
    let v = RangeView::from_slice(cr(0, 0), 2, 3, &cells);
    assert_eq!(
        call("INDEX", &[Arg::Range(v), s(num(1.0)), s(num(1.0))], &c),
        num(11.0)
    );
}

#[test]
fn sheet_fn_lookup_index_single_axis() {
    let c = ctx();
    // A single-column range: the lone selector addresses the row.
    let col_cells = [num(7.0), num(8.0), num(9.0)];
    let v = RangeView::from_slice(cr(0, 0), 3, 1, &col_cells);
    assert_eq!(call("INDEX", &[Arg::Range(v), s(num(2.0))], &c), num(8.0));
    // A single-row range: the lone selector addresses the column.
    let row_cells = [num(4.0), num(5.0), num(6.0)];
    let v = RangeView::from_slice(cr(0, 0), 1, 3, &row_cells);
    assert_eq!(call("INDEX", &[Arg::Range(v), s(num(3.0))], &c), num(6.0));
}

#[test]
fn sheet_fn_lookup_index_bounds_and_zero_and_error() {
    let c = ctx();
    let cells = [num(1.0), num(2.0), num(3.0), num(4.0)];
    // Out of bounds row (3 > 2) -> #REF!.
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call("INDEX", &[Arg::Range(v), s(num(3.0)), s(num(1.0))], &c),
        err(CellError::Ref)
    );
    // Index 0 (T0 degrade: whole-row/col array form not implemented) -> #VALUE!.
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call("INDEX", &[Arg::Range(v), s(num(0.0)), s(num(1.0))], &c),
        err(CellError::Value)
    );
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call("INDEX", &[Arg::Range(v), s(num(1.0)), s(num(0.0))], &c),
        err(CellError::Value)
    );
    // Error in the row argument propagates.
    let v = RangeView::from_slice(cr(0, 0), 2, 2, &cells);
    assert_eq!(
        call(
            "INDEX",
            &[Arg::Range(v), s(err(CellError::Na)), s(num(1.0))],
            &c
        ),
        err(CellError::Na)
    );
}

// ================= MATCH =================

#[test]
fn sheet_fn_lookup_match_exact() {
    let c = ctx();
    let cells = [txt("a"), txt("b"), txt("c"), txt("d")];
    // type 0 exact: "c" is at position 3.
    let v = RangeView::from_slice(cr(0, 0), 1, 4, &cells);
    assert_eq!(
        call("MATCH", &[s(txt("c")), Arg::Range(v), s(num(0.0))], &c),
        num(3.0)
    );
    // type 0 wildcard: "?" matches a single char -> first one-char cell.
    let v = RangeView::from_slice(cr(0, 0), 1, 4, &cells);
    assert_eq!(
        call("MATCH", &[s(txt("?")), Arg::Range(v), s(num(0.0))], &c),
        num(1.0)
    );
    // Not found -> #N/A.
    let v = RangeView::from_slice(cr(0, 0), 1, 4, &cells);
    assert_eq!(
        call("MATCH", &[s(txt("z")), Arg::Range(v), s(num(0.0))], &c),
        err(CellError::Na)
    );
}

#[test]
fn sheet_fn_lookup_match_approximate_asc_and_desc() {
    let c = ctx();
    // type 1 (default): ascending, largest <= key.
    let asc = [num(1.0), num(3.0), num(5.0), num(7.0)];
    let v = RangeView::from_slice(cr(0, 0), 1, 4, &asc);
    // key 6 -> floor 5 at position 3.
    assert_eq!(
        call("MATCH", &[s(num(6.0)), Arg::Range(v), s(num(1.0))], &c),
        num(3.0)
    );
    // Default match_type omitted = 1.
    let v = RangeView::from_slice(cr(0, 0), 1, 4, &asc);
    assert_eq!(call("MATCH", &[s(num(7.0)), Arg::Range(v)], &c), num(4.0));
    // key below first -> #N/A.
    let v = RangeView::from_slice(cr(0, 0), 1, 4, &asc);
    assert_eq!(
        call("MATCH", &[s(num(0.0)), Arg::Range(v), s(num(1.0))], &c),
        err(CellError::Na)
    );
    // type -1: descending, smallest >= key.
    let desc = [num(7.0), num(5.0), num(3.0), num(1.0)];
    let v = RangeView::from_slice(cr(0, 0), 1, 4, &desc);
    // key 4 -> smallest >= 4 is 5 at position 2.
    assert_eq!(
        call("MATCH", &[s(num(4.0)), Arg::Range(v), s(num(-1.0))], &c),
        num(2.0)
    );
}

#[test]
fn sheet_fn_lookup_match_error_propagation() {
    let c = ctx();
    let cells = [num(1.0), num(2.0)];
    // Error in the key propagates.
    let v = RangeView::from_slice(cr(0, 0), 1, 2, &cells);
    assert_eq!(
        call(
            "MATCH",
            &[s(err(CellError::Value)), Arg::Range(v), s(num(0.0))],
            &c
        ),
        err(CellError::Value)
    );
    // Arity: 1 arg (< min 2) -> #VALUE! from the dispatch guard.
    assert_eq!(call("MATCH", &[s(num(1.0))], &c), err(CellError::Value));
}

// ================= CHOOSE =================

#[test]
fn sheet_fn_lookup_choose_basic() {
    let c = ctx();
    // 1-based index into the value args.
    assert_eq!(
        call(
            "CHOOSE",
            &[s(num(2.0)), s(txt("a")), s(txt("b")), s(txt("c"))],
            &c
        ),
        txt("b")
    );
    assert_eq!(
        call("CHOOSE", &[s(num(1.0)), s(num(10.0)), s(num(20.0))], &c),
        num(10.0)
    );
}

#[test]
fn sheet_fn_lookup_choose_coercion_and_oob() {
    let c = ctx();
    // Index as text "3" coerces; truncation of 2.9 -> 2.
    assert_eq!(
        call(
            "CHOOSE",
            &[s(txt("3")), s(txt("x")), s(txt("y")), s(txt("z"))],
            &c
        ),
        txt("z")
    );
    assert_eq!(
        call("CHOOSE", &[s(num(2.9)), s(txt("x")), s(txt("y"))], &c),
        txt("y")
    );
    // index < 1 -> #VALUE!.
    assert_eq!(
        call("CHOOSE", &[s(num(0.0)), s(txt("x")), s(txt("y"))], &c),
        err(CellError::Value)
    );
    // index past the value count -> #VALUE!.
    assert_eq!(
        call("CHOOSE", &[s(num(3.0)), s(txt("x")), s(txt("y"))], &c),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_lookup_choose_error_and_arity() {
    let c = ctx();
    // Error in the index propagates.
    assert_eq!(
        call("CHOOSE", &[s(err(CellError::Div0)), s(num(1.0))], &c),
        err(CellError::Div0)
    );
    // An error reaching the chosen value passes through.
    assert_eq!(
        call(
            "CHOOSE",
            &[s(num(1.0)), s(err(CellError::Na)), s(num(2.0))],
            &c
        ),
        err(CellError::Na)
    );
    // Arity: 1 arg (< min 2) -> #VALUE! from the dispatch guard.
    assert_eq!(call("CHOOSE", &[s(num(1.0))], &c), err(CellError::Value));
}

// ================= ROW =================

#[test]
fn sheet_fn_lookup_row_basic() {
    let c = ctx();
    // With a reference: origin.row + 1. Range origin at B5 (row 4) -> 5.
    let cells = [num(0.0), num(0.0)];
    let v = RangeView::from_slice(cr(4, 1), 1, 2, &cells);
    assert_eq!(call("ROW", &[Arg::Range(v)], &c), num(5.0));
}

#[test]
fn sheet_fn_lookup_row_no_arg_uses_current() {
    let c = ctx(); // current = row 10 -> ROW() = 11.
    assert_eq!(call("ROW", &[], &c), num(11.0));
}

// ================= COLUMN =================

#[test]
fn sheet_fn_lookup_column_basic() {
    let c = ctx();
    // Range origin at D2 (col 3) -> COLUMN = 4.
    let cells = [num(0.0)];
    let v = RangeView::from_slice(cr(1, 3), 1, 1, &cells);
    assert_eq!(call("COLUMN", &[Arg::Range(v)], &c), num(4.0));
}

#[test]
fn sheet_fn_lookup_column_no_arg_uses_current() {
    let c = ctx(); // current = col 3 -> COLUMN() = 4.
    assert_eq!(call("COLUMN", &[], &c), num(4.0));
}
