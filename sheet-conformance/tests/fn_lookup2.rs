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

//! T1 lookup/reference conformance (spec §7, §11 T1 — "full lookup incl.
//! XLOOKUP/INDEX/MATCH"). Self-contained **direct-dispatch** tests: each
//! resolves a function id via [`sheet_core::funcs::lookup_func`] and routes
//! through the frozen [`sheet_fn::dispatch`], so it exercises the SAME path a
//! formula evaluation takes (the registry arity guard is included: an arity
//! violation surfaces as `#VALUE!` from the dispatch table, not the kernel).
//! Test-fn names use the prefix the registry rows point at
//! (`sheet_fn_lookup_<name>`) so the coverage gate (§12.2) finds them.
//!
//! Implemented (fully tested here): `XLOOKUP`, `XMATCH`, `ADDRESS`, `ROWS`,
//! `COLUMNS`. Deferred (status `planned` — they need a model cell/formula
//! reader the FROZEN `EvalCtx` cannot carry): `OFFSET`, `INDIRECT`,
//! `FORMULATEXT`; their `sheet_fn_lookup_<name>_deferred` tests pin the honest
//! contract — a planned row dispatches to `#NAME?` (uncallable by
//! construction), never a silent wrong value.

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

/// A deterministic context (the §7 convention: fixed clock + seed).
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

// ================= XLOOKUP =================

#[test]
fn sheet_fn_lookup_xlookup_exact_default() {
    let c = ctx();
    // lookup_array (1 col): apple, banana, cherry; return_array (1 col): 1,2,3.
    let lk = [txt("apple"), txt("banana"), txt("cherry")];
    let rt = [num(1.0), num(2.0), num(3.0)];
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    // Exact (default match_mode 0): "banana" -> 2.
    assert_eq!(
        call(
            "XLOOKUP",
            &[s(txt("banana")), Arg::Range(l), Arg::Range(r)],
            &c
        ),
        num(2.0)
    );
    // Case-insensitive exact.
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[s(txt("CHERRY")), Arg::Range(l), Arg::Range(r)],
            &c
        ),
        num(3.0)
    );
}

#[test]
fn sheet_fn_lookup_xlookup_not_found_default_and_custom() {
    let c = ctx();
    let lk = [num(1.0), num(2.0), num(3.0)];
    let rt = [txt("a"), txt("b"), txt("c")];
    // Not found, no if_not_found -> #N/A.
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    assert_eq!(
        call("XLOOKUP", &[s(num(9.0)), Arg::Range(l), Arg::Range(r)], &c),
        err(CellError::Na)
    );
    // Not found, custom if_not_found -> "missing".
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[s(num(9.0)), Arg::Range(l), Arg::Range(r), s(txt("missing"))],
            &c
        ),
        txt("missing")
    );
}

#[test]
fn sheet_fn_lookup_xlookup_match_modes() {
    let c = ctx();
    // Sorted ascending numbers for approximate modes.
    let lk = [num(10.0), num(20.0), num(30.0)];
    let rt = [txt("ten"), txt("twenty"), txt("thirty")];
    // match_mode -1 (exact-or-next-smaller): key 25 -> 20 -> "twenty".
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(num(25.0)),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(-1.0))
            ],
            &c
        ),
        txt("twenty")
    );
    // match_mode 1 (exact-or-next-larger): key 25 -> 30 -> "thirty".
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(num(25.0)),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(1.0))
            ],
            &c
        ),
        txt("thirty")
    );
    // match_mode -1 below the smallest -> not found -> if_not_found.
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(num(5.0)),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(-1.0))
            ],
            &c
        ),
        txt("na")
    );
}

#[test]
fn sheet_fn_lookup_xlookup_wildcard_mode() {
    let c = ctx();
    let lk = [txt("apple"), txt("banana"), txt("cherry")];
    let rt = [num(1.0), num(2.0), num(3.0)];
    // match_mode 2 (wildcard): "ban*" matches "banana" -> 2.
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(txt("ban*")),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(2.0))
            ],
            &c
        ),
        num(2.0)
    );
    // "?herry" matches "cherry" -> 3.
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(txt("?herry")),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(2.0))
            ],
            &c
        ),
        num(3.0)
    );
}

#[test]
fn sheet_fn_lookup_xlookup_search_mode_last_to_first() {
    let c = ctx();
    // Duplicate key "x" at positions 1 and 3; first-to-last finds first, last-
    // to-first finds last.
    let lk = [txt("x"), txt("y"), txt("x")];
    let rt = [num(1.0), num(2.0), num(3.0)];
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    // Default (1, first-to-last) -> 1.
    assert_eq!(
        call("XLOOKUP", &[s(txt("x")), Arg::Range(l), Arg::Range(r)], &c),
        num(1.0)
    );
    // search_mode -1 (last-to-first) -> 3.
    let l = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(txt("x")),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(0.0)),
                s(num(-1.0))
            ],
            &c
        ),
        num(3.0)
    );
}

#[test]
fn sheet_fn_lookup_xlookup_binary_search_modes() {
    let c = ctx();
    // Ascending data: binary search (2) exact + approximate.
    let asc = [num(1.0), num(3.0), num(5.0), num(7.0), num(9.0)];
    let rt = [txt("a"), txt("b"), txt("c"), txt("d"), txt("e")];
    // Binary asc, exact hit 5 -> "c".
    let l = RangeView::from_slice(cr(0, 0), 5, 1, &asc);
    let r = RangeView::from_slice(cr(0, 1), 5, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(num(5.0)),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(0.0)),
                s(num(2.0))
            ],
            &c
        ),
        txt("c")
    );
    // Binary asc, exact-or-next-smaller, key 6 -> 5 -> "c".
    let l = RangeView::from_slice(cr(0, 0), 5, 1, &asc);
    let r = RangeView::from_slice(cr(0, 1), 5, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(num(6.0)),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(-1.0)),
                s(num(2.0))
            ],
            &c
        ),
        txt("c")
    );
    // Descending data: binary desc (-2), exact-or-next-larger, key 6 -> 7.
    let desc = [num(9.0), num(7.0), num(5.0), num(3.0), num(1.0)];
    let rtd = [txt("e"), txt("d"), txt("c"), txt("b"), txt("a")];
    let l = RangeView::from_slice(cr(0, 0), 5, 1, &desc);
    let r = RangeView::from_slice(cr(0, 1), 5, 1, &rtd);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(num(6.0)),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(1.0)),
                s(num(-2.0))
            ],
            &c
        ),
        txt("d")
    );
}

#[test]
fn sheet_fn_lookup_xlookup_row_oriented_array() {
    let c = ctx();
    // A single-ROW lookup_array and return_array (1×3).
    let lk = [num(1.0), num(2.0), num(3.0)];
    let rt = [txt("one"), txt("two"), txt("three")];
    let l = RangeView::from_slice(cr(0, 0), 1, 3, &lk);
    let r = RangeView::from_slice(cr(1, 0), 1, 3, &rt);
    assert_eq!(
        call("XLOOKUP", &[s(num(2.0)), Arg::Range(l), Arg::Range(r)], &c),
        txt("two")
    );
}

#[test]
fn sheet_fn_lookup_xlookup_errors_and_arity() {
    let c = ctx();
    let lk = [num(1.0), num(2.0)];
    let rt = [txt("a"), txt("b")];
    // Mismatched lengths (lookup 2, return 3) -> #VALUE!.
    let rt3 = [txt("a"), txt("b"), txt("c")];
    let l = RangeView::from_slice(cr(0, 0), 2, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 3, 1, &rt3);
    assert_eq!(
        call("XLOOKUP", &[s(num(1.0)), Arg::Range(l), Arg::Range(r)], &c),
        err(CellError::Value)
    );
    // Error in the key propagates.
    let l = RangeView::from_slice(cr(0, 0), 2, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 2, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[s(err(CellError::Div0)), Arg::Range(l), Arg::Range(r)],
            &c
        ),
        err(CellError::Div0)
    );
    // Invalid match_mode (3) -> #VALUE!.
    let l = RangeView::from_slice(cr(0, 0), 2, 1, &lk);
    let r = RangeView::from_slice(cr(0, 1), 2, 1, &rt);
    assert_eq!(
        call(
            "XLOOKUP",
            &[
                s(num(1.0)),
                Arg::Range(l),
                Arg::Range(r),
                s(txt("na")),
                s(num(3.0))
            ],
            &c
        ),
        err(CellError::Value)
    );
    // Arity: 2 args (< min 3) -> #VALUE! from the dispatch guard.
    let l = RangeView::from_slice(cr(0, 0), 2, 1, &lk);
    assert_eq!(
        call("XLOOKUP", &[s(num(1.0)), Arg::Range(l)], &c),
        err(CellError::Value)
    );
}

// ================= XMATCH =================

#[test]
fn sheet_fn_lookup_xmatch_exact_and_modes() {
    let c = ctx();
    let lk = [num(10.0), num(20.0), num(30.0)];
    // Exact (default): 20 -> position 2.
    let v = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    assert_eq!(call("XMATCH", &[s(num(20.0)), Arg::Range(v)], &c), num(2.0));
    // match_mode -1 (exact-or-next-smaller): 25 -> 20 -> position 2.
    let v = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    assert_eq!(
        call("XMATCH", &[s(num(25.0)), Arg::Range(v), s(num(-1.0))], &c),
        num(2.0)
    );
    // match_mode 1 (exact-or-next-larger): 25 -> 30 -> position 3.
    let v = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    assert_eq!(
        call("XMATCH", &[s(num(25.0)), Arg::Range(v), s(num(1.0))], &c),
        num(3.0)
    );
}

#[test]
fn sheet_fn_lookup_xmatch_wildcard_and_not_found() {
    let c = ctx();
    let lk = [txt("alpha"), txt("beta"), txt("gamma")];
    // Wildcard mode 2: "g*" -> "gamma" at position 3.
    let v = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    assert_eq!(
        call("XMATCH", &[s(txt("g*")), Arg::Range(v), s(num(2.0))], &c),
        num(3.0)
    );
    // Not found -> #N/A.
    let v = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    assert_eq!(
        call("XMATCH", &[s(txt("zzz")), Arg::Range(v), s(num(0.0))], &c),
        err(CellError::Na)
    );
    // Arity: 1 arg (< min 2) -> #VALUE! from the dispatch guard.
    assert_eq!(call("XMATCH", &[s(num(1.0))], &c), err(CellError::Value));
}

#[test]
fn sheet_fn_lookup_xmatch_search_mode_last_to_first() {
    let c = ctx();
    let lk = [txt("x"), txt("y"), txt("x")];
    // Default first-to-last -> 1; last-to-first -> 3.
    let v = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    assert_eq!(
        call(
            "XMATCH",
            &[s(txt("x")), Arg::Range(v), s(num(0.0)), s(num(1.0))],
            &c
        ),
        num(1.0)
    );
    let v = RangeView::from_slice(cr(0, 0), 3, 1, &lk);
    assert_eq!(
        call(
            "XMATCH",
            &[s(txt("x")), Arg::Range(v), s(num(0.0)), s(num(-1.0))],
            &c
        ),
        num(3.0)
    );
}

// ================= ADDRESS =================

#[test]
fn sheet_fn_lookup_address_abs_modes() {
    let c = ctx();
    // (row 2, col 3) under each abs_num.
    assert_eq!(
        call("ADDRESS", &[s(num(2.0)), s(num(3.0))], &c),
        txt("$C$2")
    ); // default 1
    assert_eq!(
        call("ADDRESS", &[s(num(2.0)), s(num(3.0)), s(num(1.0))], &c),
        txt("$C$2")
    );
    assert_eq!(
        call("ADDRESS", &[s(num(2.0)), s(num(3.0)), s(num(2.0))], &c),
        txt("C$2")
    );
    assert_eq!(
        call("ADDRESS", &[s(num(2.0)), s(num(3.0)), s(num(3.0))], &c),
        txt("$C2")
    );
    assert_eq!(
        call("ADDRESS", &[s(num(2.0)), s(num(3.0)), s(num(4.0))], &c),
        txt("C2")
    );
}

#[test]
fn sheet_fn_lookup_address_r1c1_and_sheet() {
    let c = ctx();
    // a1=FALSE -> R1C1; absolute coords are bare.
    assert_eq!(
        call(
            "ADDRESS",
            &[s(num(2.0)), s(num(3.0)), s(num(1.0)), s(b(false))],
            &c
        ),
        txt("R2C3")
    );
    // a1=FALSE, abs_num 4 (relative) -> bracketed.
    assert_eq!(
        call(
            "ADDRESS",
            &[s(num(2.0)), s(num(3.0)), s(num(4.0)), s(b(false))],
            &c
        ),
        txt("R[2]C[3]")
    );
    // sheet_text prefix (plain name, no quoting).
    assert_eq!(
        call(
            "ADDRESS",
            &[
                s(num(1.0)),
                s(num(1.0)),
                s(num(1.0)),
                s(b(true)),
                s(txt("Sheet1"))
            ],
            &c
        ),
        txt("Sheet1!$A$1")
    );
    // sheet_text with a space -> single-quoted.
    assert_eq!(
        call(
            "ADDRESS",
            &[
                s(num(1.0)),
                s(num(1.0)),
                s(num(1.0)),
                s(b(true)),
                s(txt("My Sheet"))
            ],
            &c
        ),
        txt("'My Sheet'!$A$1")
    );
}

#[test]
fn sheet_fn_lookup_address_errors_and_arity() {
    let c = ctx();
    // row < 1 -> #REF!.
    assert_eq!(
        call("ADDRESS", &[s(num(0.0)), s(num(1.0))], &c),
        err(CellError::Ref)
    );
    // col < 1 -> #REF!.
    assert_eq!(
        call("ADDRESS", &[s(num(1.0)), s(num(0.0))], &c),
        err(CellError::Ref)
    );
    // abs_num out of 1..=4 -> #VALUE!.
    assert_eq!(
        call("ADDRESS", &[s(num(1.0)), s(num(1.0)), s(num(5.0))], &c),
        err(CellError::Value)
    );
    // Error in row propagates.
    assert_eq!(
        call("ADDRESS", &[s(err(CellError::Na)), s(num(1.0))], &c),
        err(CellError::Na)
    );
    // Arity: 1 arg (< min 2) -> #VALUE! from the dispatch guard.
    assert_eq!(call("ADDRESS", &[s(num(1.0))], &c), err(CellError::Value));
    // Arity: 6 args (> max 5) -> #VALUE!.
    assert_eq!(
        call(
            "ADDRESS",
            &[
                s(num(1.0)),
                s(num(1.0)),
                s(num(1.0)),
                s(b(true)),
                s(txt("S")),
                s(num(9.0)),
            ],
            &c
        ),
        err(CellError::Value)
    );
}

// ================= ROWS / COLUMNS =================

#[test]
fn sheet_fn_lookup_rows_basic() {
    let c = ctx();
    // A 3×2 range -> 3 rows.
    let cells = [num(0.0), num(0.0), num(0.0), num(0.0), num(0.0), num(0.0)];
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(call("ROWS", &[Arg::Range(v)], &c), num(3.0));
    // A scalar -> 1.
    assert_eq!(call("ROWS", &[s(num(7.0))], &c), num(1.0));
    // An error scalar propagates.
    assert_eq!(
        call("ROWS", &[s(err(CellError::Ref))], &c),
        err(CellError::Ref)
    );
    // Arity: 0 args (< min 1) -> #VALUE! from the dispatch guard.
    assert_eq!(call("ROWS", &[], &c), err(CellError::Value));
}

#[test]
fn sheet_fn_lookup_columns_basic() {
    let c = ctx();
    // A 3×2 range -> 2 columns.
    let cells = [num(0.0), num(0.0), num(0.0), num(0.0), num(0.0), num(0.0)];
    let v = RangeView::from_slice(cr(0, 0), 3, 2, &cells);
    assert_eq!(call("COLUMNS", &[Arg::Range(v)], &c), num(2.0));
    // A single-row range -> its width.
    let row = [num(0.0), num(0.0), num(0.0), num(0.0)];
    let v = RangeView::from_slice(cr(0, 0), 1, 4, &row);
    assert_eq!(call("COLUMNS", &[Arg::Range(v)], &c), num(4.0));
    // A scalar -> 1.
    assert_eq!(call("COLUMNS", &[s(txt("x"))], &c), num(1.0));
    // Arity: 0 args (< min 1) -> #VALUE! from the dispatch guard.
    assert_eq!(call("COLUMNS", &[], &c), err(CellError::Value));
}

// ================= DEFERRED ROWS (status: planned) =================
//
// OFFSET / INDIRECT / FORMULATEXT need a model cell/formula reader the FROZEN
// `EvalCtx` cannot carry (see fn_lookup2 module header + the lookup2.yaml
// rationale). They stay `status: planned`, so dispatch returns `#NAME?` — the
// honest "registered but uncallable" contract. These tests pin that contract
// (and double as the `tests.rust` prefix target should a future amendment flip
// the rows to `implemented`).

#[test]
fn sheet_fn_lookup_offset_deferred_name_error() {
    let c = ctx();
    // Planned row -> #NAME? regardless of (well-formed) arguments.
    let cells = [num(0.0)];
    let v = RangeView::from_slice(cr(0, 0), 1, 1, &cells);
    assert_eq!(
        call("OFFSET", &[Arg::Range(v), s(num(1.0)), s(num(1.0))], &c),
        err(CellError::Name)
    );
}

#[test]
fn sheet_fn_lookup_indirect_deferred_name_error() {
    let c = ctx();
    assert_eq!(call("INDIRECT", &[s(txt("A1"))], &c), err(CellError::Name));
}

#[test]
fn sheet_fn_lookup_formulatext_deferred_name_error() {
    let c = ctx();
    let cells = [num(0.0)];
    let v = RangeView::from_slice(cr(0, 0), 1, 1, &cells);
    assert_eq!(
        call("FORMULATEXT", &[Arg::Range(v)], &c),
        err(CellError::Name)
    );
}
