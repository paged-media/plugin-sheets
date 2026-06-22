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

//! Aggregation family conformance (spec §7, §11 T0). Self-contained
//! direct-dispatch tests: each call resolves the registry name through
//! `sheet_core::funcs::lookup_func`, builds the `&[Arg]` slice, and routes it
//! through `sheet_fn::dispatch` — exactly the path `sheet-calc` will take, so
//! these tests exercise the generated arity guard + the kernel together.
//!
//! Every test fn is named with the prefix the `registry/functions/agg.yaml`
//! rows point at (`sheet_fn_agg_<name>`), so the coverage gate (§12.2) finds
//! it. The `.golden.tsv` files under `corpus/fn-corpus/agg/` are the
//! formula-level fixtures the Phase-2 differential oracle will replay.
//!
//! The named Excel rulings under test (module doc of `families::agg`):
//! - `COUNT`: scalar bool/numeric-text counts; range bool/numeric-text does
//!   NOT (the scalar/range asymmetry).
//! - `AVERAGE`/`MIN`/`MAX`: skip non-numeric range cells; `MIN`/`MAX` of
//!   nothing is `0`; `AVERAGE` of nothing is `#DIV/0!`.
//! - an error cell inside an aggregated range IS the result.
//! - `SUMIF` `sum_range` defaults to `range` and aligns by offset.

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::{dispatch, Arg, EvalCtx, RangeView};

// ---- harness ----------------------------------------------------------------

fn cr(row: u32, col: u32) -> CellRef {
    CellRef {
        sheet: 0,
        row,
        col,
        row_abs: false,
        col_abs: false,
    }
}

/// A fixed, deterministic context (the agg family is non-volatile, so the
/// clock/RNG never matter — but the convention pins them anyway).
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cr(0, 0), 45000.5, 42)
}

/// Dispatch `name` with the given args, resolving its `FuncId` via the
/// registry (so the generated arity guard runs too).
fn call(name: &str, args: &[Arg]) -> CellValue {
    let id =
        sheet_core::funcs::lookup_func(name).unwrap_or_else(|| panic!("unknown function {name}"));
    dispatch(id, args, &ctx())
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

/// A 1-row range view over `cells` (origin A1), borrowing the slice.
fn row_view(cells: &[CellValue]) -> RangeView<'_> {
    RangeView::from_slice(cr(0, 0), 1, cells.len() as u32, cells)
}

// ---- AVERAGE ----------------------------------------------------------------

#[test]
fn sheet_fn_agg_average_basic() {
    // Scalars: mean of 1,2,3.
    let args = [
        Arg::Scalar(num(1.0)),
        Arg::Scalar(num(2.0)),
        Arg::Scalar(num(3.0)),
    ];
    assert_eq!(call("AVERAGE", &args), num(2.0));

    // Range: mean over a numeric range.
    let cells = [num(2.0), num(4.0), num(6.0)];
    assert_eq!(call("AVERAGE", &[Arg::Range(row_view(&cells))]), num(4.0));
}

#[test]
fn sheet_fn_agg_average_skips_nonnumeric_in_range() {
    // Text/bool/blank in a range are skipped: mean of {10, 20} = 15.
    let cells = [
        num(10.0),
        txt("x"),
        CellValue::Bool(true),
        CellValue::Empty,
        num(20.0),
    ];
    assert_eq!(call("AVERAGE", &[Arg::Range(row_view(&cells))]), num(15.0));
}

#[test]
fn sheet_fn_agg_average_coercion_edge() {
    // A scalar bool/numeric-text IS coerced (unlike inside a range):
    // AVERAGE(TRUE, "3") -> (1 + 3) / 2 = 2.
    let args = [Arg::Scalar(CellValue::Bool(true)), Arg::Scalar(txt("3"))];
    assert_eq!(call("AVERAGE", &args), num(2.0));
}

#[test]
fn sheet_fn_agg_average_empty_is_div0() {
    // No numbers at all -> #DIV/0! (Excel).
    let cells = [txt("a"), CellValue::Empty];
    assert_eq!(
        call("AVERAGE", &[Arg::Range(row_view(&cells))]),
        err(CellError::Div0)
    );
}

#[test]
fn sheet_fn_agg_average_error_propagates_from_range() {
    // An error cell inside the range IS the result.
    let cells = [num(1.0), err(CellError::Div0), num(3.0)];
    assert_eq!(
        call("AVERAGE", &[Arg::Range(row_view(&cells))]),
        err(CellError::Div0)
    );
}

#[test]
fn sheet_fn_agg_average_arity_violation() {
    // min arity is 1 -> zero args is #VALUE! (the generated guard).
    assert_eq!(call("AVERAGE", &[]), err(CellError::Value));
}

// ---- MIN --------------------------------------------------------------------

#[test]
fn sheet_fn_agg_min_basic() {
    let cells = [num(5.0), num(-2.0), num(9.0)];
    assert_eq!(call("MIN", &[Arg::Range(row_view(&cells))]), num(-2.0));
}

#[test]
fn sheet_fn_agg_min_skips_nonnumeric_and_empty_is_zero() {
    // Non-numeric range cells are skipped...
    let cells = [txt("x"), num(7.0), CellValue::Bool(false)];
    assert_eq!(call("MIN", &[Arg::Range(row_view(&cells))]), num(7.0));
    // ...and MIN of nothing is 0 (Excel ruling), not an error.
    let none = [txt("a"), CellValue::Empty];
    assert_eq!(call("MIN", &[Arg::Range(row_view(&none))]), num(0.0));
}

#[test]
fn sheet_fn_agg_min_error_propagates() {
    let cells = [num(1.0), err(CellError::Na)];
    assert_eq!(
        call("MIN", &[Arg::Range(row_view(&cells))]),
        err(CellError::Na)
    );
}

#[test]
fn sheet_fn_agg_min_arity_violation() {
    assert_eq!(call("MIN", &[]), err(CellError::Value));
}

// ---- MAX --------------------------------------------------------------------

#[test]
fn sheet_fn_agg_max_basic() {
    let cells = [num(5.0), num(-2.0), num(9.0)];
    assert_eq!(call("MAX", &[Arg::Range(row_view(&cells))]), num(9.0));
}

#[test]
fn sheet_fn_agg_max_empty_is_zero() {
    // MAX of nothing is 0 (Excel), even when negatives would be smaller.
    let none: [CellValue; 0] = [];
    assert_eq!(call("MAX", &[Arg::Range(row_view(&none))]), num(0.0));
}

#[test]
fn sheet_fn_agg_max_coercion_edge() {
    // Scalar bool/numeric-text participate: MAX(FALSE, "5", 2) = 5.
    let args = [
        Arg::Scalar(CellValue::Bool(false)),
        Arg::Scalar(txt("5")),
        Arg::Scalar(num(2.0)),
    ];
    assert_eq!(call("MAX", &args), num(5.0));
}

#[test]
fn sheet_fn_agg_max_error_propagates() {
    let cells = [num(1.0), err(CellError::Value), num(3.0)];
    assert_eq!(
        call("MAX", &[Arg::Range(row_view(&cells))]),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_agg_max_arity_violation() {
    assert_eq!(call("MAX", &[]), err(CellError::Value));
}

// ---- COUNT (the scalar/range asymmetry) ------------------------------------

#[test]
fn sheet_fn_agg_count_basic() {
    let cells = [num(1.0), num(2.0), num(3.0)];
    assert_eq!(call("COUNT", &[Arg::Range(row_view(&cells))]), num(3.0));
}

#[test]
fn sheet_fn_agg_count_range_excludes_bool_and_text() {
    // In a RANGE, only stored numbers count: bool and numeric-text do NOT.
    let cells = [
        num(1.0),
        CellValue::Bool(true),
        txt("5"),
        txt("x"),
        CellValue::Empty,
        num(2.0),
    ];
    assert_eq!(call("COUNT", &[Arg::Range(row_view(&cells))]), num(2.0));
}

#[test]
fn sheet_fn_agg_count_scalar_counts_bool_and_numeric_text() {
    // As SCALARS, bool and numeric-text DO count (the asymmetry).
    let args = [
        Arg::Scalar(CellValue::Bool(true)), // counts
        Arg::Scalar(txt("5")),              // counts (numeric text)
        Arg::Scalar(txt("x")),              // does not count
        Arg::Scalar(num(9.0)),              // counts
    ];
    assert_eq!(call("COUNT", &args), num(3.0));
}

#[test]
fn sheet_fn_agg_count_never_errors_on_error_cell() {
    // An error cell is not a number and is not counted; COUNT never errors.
    let cells = [num(1.0), err(CellError::Div0), num(2.0)];
    assert_eq!(call("COUNT", &[Arg::Range(row_view(&cells))]), num(2.0));
    let scalar_err = [Arg::Scalar(err(CellError::Na)), Arg::Scalar(num(4.0))];
    assert_eq!(call("COUNT", &scalar_err), num(1.0));
}

#[test]
fn sheet_fn_agg_count_arity_violation() {
    assert_eq!(call("COUNT", &[]), err(CellError::Value));
}

// ---- COUNTA -----------------------------------------------------------------

#[test]
fn sheet_fn_agg_counta_basic() {
    // Counts every non-empty range cell: numbers, text, bools, AND errors.
    let cells = [
        num(1.0),
        txt("x"),
        CellValue::Bool(false),
        err(CellError::Na),
        CellValue::Empty,
    ];
    assert_eq!(call("COUNTA", &[Arg::Range(row_view(&cells))]), num(4.0));
}

#[test]
fn sheet_fn_agg_counta_scalars_always_count() {
    // Every scalar argument is a present value -> counts (even text/error).
    let args = [
        Arg::Scalar(txt("")),
        Arg::Scalar(num(0.0)),
        Arg::Scalar(err(CellError::Value)),
    ];
    assert_eq!(call("COUNTA", &args), num(3.0));
}

#[test]
fn sheet_fn_agg_counta_edge_empty_range() {
    let none: [CellValue; 0] = [];
    assert_eq!(call("COUNTA", &[Arg::Range(row_view(&none))]), num(0.0));
}

#[test]
fn sheet_fn_agg_counta_arity_violation() {
    assert_eq!(call("COUNTA", &[]), err(CellError::Value));
}

// ---- COUNTBLANK (range-only) -----------------------------------------------

#[test]
fn sheet_fn_agg_countblank_basic() {
    let cells = [
        num(1.0),
        CellValue::Empty,
        txt("x"),
        CellValue::Empty,
        CellValue::Empty,
    ];
    assert_eq!(
        call("COUNTBLANK", &[Arg::Range(row_view(&cells))]),
        num(3.0)
    );
}

#[test]
fn sheet_fn_agg_countblank_text_empty_is_not_blank() {
    // Text("") is NOT blank; only CellValue::Empty counts.
    let cells = [txt(""), CellValue::Empty, num(0.0)];
    assert_eq!(
        call("COUNTBLANK", &[Arg::Range(row_view(&cells))]),
        num(1.0)
    );
}

#[test]
fn sheet_fn_agg_countblank_edge_no_blanks() {
    let cells = [num(1.0), num(2.0)];
    assert_eq!(
        call("COUNTBLANK", &[Arg::Range(row_view(&cells))]),
        num(0.0)
    );
}

#[test]
fn sheet_fn_agg_countblank_arity_violation() {
    // arity is exactly 1: zero args and two args are both #VALUE!.
    assert_eq!(call("COUNTBLANK", &[]), err(CellError::Value));
    let two = [Arg::Scalar(num(1.0)), Arg::Scalar(num(2.0))];
    assert_eq!(call("COUNTBLANK", &two), err(CellError::Value));
}

// ---- COUNTIF ----------------------------------------------------------------

#[test]
fn sheet_fn_agg_countif_basic() {
    // ">2" over {1,2,3,4} matches 3 and 4.
    let cells = [num(1.0), num(2.0), num(3.0), num(4.0)];
    let args = [Arg::Range(row_view(&cells)), Arg::Scalar(txt(">2"))];
    assert_eq!(call("COUNTIF", &args), num(2.0));
}

#[test]
fn sheet_fn_agg_countif_text_wildcard_edge() {
    // "a*" matches text starting with a (case-insensitive).
    let cells = [txt("apple"), txt("Avocado"), txt("banana"), txt("cherry")];
    let args = [Arg::Range(row_view(&cells)), Arg::Scalar(txt("a*"))];
    assert_eq!(call("COUNTIF", &args), num(2.0));
}

#[test]
fn sheet_fn_agg_countif_number_text_equality() {
    // Bare numeric criterion matches the number 5 and the text "5".
    let cells = [num(5.0), txt("5"), num(6.0)];
    let args = [Arg::Range(row_view(&cells)), Arg::Scalar(num(5.0))];
    assert_eq!(call("COUNTIF", &args), num(2.0));
}

#[test]
fn sheet_fn_agg_countif_wildcard_matches_text_only() {
    // Audit finding 3: wildcard criteria match TEXT cells only — numbers and
    // blanks NEVER match. The exact audit range: {100, "hello", 200, "5"(text),
    // empty}. COUNTIF(range,"*") == 2 (the two text cells), NOT 5.
    let cells = [
        num(100.0),
        txt("hello"),
        num(200.0),
        txt("5"), // numeric-LOOKING text — still a text cell, so it matches
        CellValue::Empty,
    ];
    let star = [Arg::Range(row_view(&cells)), Arg::Scalar(txt("*"))];
    assert_eq!(
        call("COUNTIF", &star),
        num(2.0),
        "COUNTIF(*) = text cells only"
    );

    // "?*" (at least one character) likewise matches only the two text cells.
    let qstar = [Arg::Range(row_view(&cells)), Arg::Scalar(txt("?*"))];
    assert_eq!(
        call("COUNTIF", &qstar),
        num(2.0),
        "COUNTIF(?*) = text cells"
    );
}

#[test]
fn sheet_fn_agg_sumif_wildcard_ignores_numeric_criteria_cells() {
    // Audit finding 3, SUMIF form: a wildcard criterion over a criteria range
    // that mixes numbers and text must IGNORE the numeric criteria cells
    // (they never match `*`), summing only the offset-aligned amounts of the
    // matched TEXT rows.
    let labels = [num(100.0), txt("hello"), num(200.0), txt("world")];
    let amounts = [num(1.0), num(10.0), num(100.0), num(1000.0)];
    let args = [
        Arg::Range(row_view(&labels)),
        Arg::Scalar(txt("*")),
        Arg::Range(row_view(&amounts)),
    ];
    // Only the text labels ("hello", "world") match -> 10 + 1000 = 1010.
    assert_eq!(call("SUMIF", &args), num(1010.0));
}

#[test]
fn sheet_fn_agg_countif_arity_violation() {
    // arity is exactly 2.
    assert_eq!(
        call("COUNTIF", &[Arg::Scalar(num(1.0))]),
        err(CellError::Value)
    );
}

// ---- SUMIF ------------------------------------------------------------------

#[test]
fn sheet_fn_agg_sumif_basic() {
    // Two-arg form (no sum_range): sum the matching cells of `range` itself.
    let cells = [num(1.0), num(5.0), num(3.0), num(8.0)];
    let args = [Arg::Range(row_view(&cells)), Arg::Scalar(txt(">2"))];
    // 5 + 3 + 8 = 16.
    assert_eq!(call("SUMIF", &args), num(16.0));
}

#[test]
fn sheet_fn_agg_sumif_sum_range_aligns_by_offset() {
    // range = labels, sum_range = amounts; align by offset.
    let labels = [txt("apple"), txt("pear"), txt("apple")];
    let amounts = [num(10.0), num(20.0), num(30.0)];
    let args = [
        Arg::Range(row_view(&labels)),
        Arg::Scalar(txt("apple")),
        Arg::Range(row_view(&amounts)),
    ];
    // apple at offsets 0 and 2 -> 10 + 30 = 40.
    assert_eq!(call("SUMIF", &args), num(40.0));
}

#[test]
fn sheet_fn_agg_sumif_default_sum_range_is_range() {
    // Omitting sum_range defaults it to `range` (Excel).
    let cells = [num(4.0), num(9.0), num(2.0)];
    let args = [Arg::Range(row_view(&cells)), Arg::Scalar(txt(">=4"))];
    // 4 + 9 = 13.
    assert_eq!(call("SUMIF", &args), num(13.0));
}

#[test]
fn sheet_fn_agg_sumif_error_in_summed_cell_propagates() {
    // An error in a CONTRIBUTING sum_range cell propagates.
    let labels = [txt("a"), txt("a"), txt("b")];
    let amounts = [num(10.0), err(CellError::Div0), num(99.0)];
    let args = [
        Arg::Range(row_view(&labels)),
        Arg::Scalar(txt("a")),
        Arg::Range(row_view(&amounts)),
    ];
    assert_eq!(call("SUMIF", &args), err(CellError::Div0));
}

#[test]
fn sheet_fn_agg_sumif_nonnumeric_match_contributes_zero() {
    // A matched but non-numeric sum cell adds 0 (does not error).
    let labels = [txt("a"), txt("a")];
    let amounts = [txt("not a number"), num(7.0)];
    let args = [
        Arg::Range(row_view(&labels)),
        Arg::Scalar(txt("a")),
        Arg::Range(row_view(&amounts)),
    ];
    assert_eq!(call("SUMIF", &args), num(7.0));
}

#[test]
fn sheet_fn_agg_sumif_arity_violation() {
    // min arity is 2; one arg is #VALUE!.
    assert_eq!(
        call("SUMIF", &[Arg::Range(row_view(&[num(1.0)]))]),
        err(CellError::Value)
    );
}
