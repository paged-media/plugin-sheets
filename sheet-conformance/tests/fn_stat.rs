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

//! Statistics family conformance (spec §7, §11/§13 M1). Self-contained
//! direct-dispatch tests: each call resolves the registry name through
//! `sheet_core::funcs::lookup_func`, builds the `&[Arg]` slice, and routes it
//! through `sheet_fn::dispatch` — exactly the path `sheet-calc` will take, so
//! these tests exercise the generated arity guard + the kernel together.
//!
//! Every test fn is named with the prefix the `registry/functions/stat.yaml`
//! rows point at (`sheet_fn_stat_<name>`), so the coverage gate (§12.2) finds
//! it. The `.golden.tsv` files under `corpus/fn-corpus/stat/` are the
//! formula-level fixtures the differential oracle will replay.
//!
//! The named Excel rulings under test (module doc of `families::stat`):
//! - sample (`n−1`) vs population (`n`) estimators; legacy aliases follow
//!   Excel (STDEV/VAR = sample, STDEVP/VARP = population);
//! - PERCENTILE.INC linear interpolation; QUARTILE.INC `q` in `0..=4`;
//! - LARGE/SMALL `#NUM!` out of bounds; RANK.EQ descending default + ties;
//! - the `*IFS` family AND-across-pairs + AVERAGEIFS empty → `#DIV/0!`;
//! - SUMPRODUCT non-numeric → 0; GEOMEAN/HARMEAN positivity;
//! - CORREL/SLOPE/INTERCEPT/RSQ paired regression + degenerate `#DIV/0!`.

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

/// A fixed, deterministic context (the stat family is non-volatile).
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cr(0, 0), 45000.5, 42)
}

/// Dispatch `name` with the given args, resolving its `FuncId` via the registry
/// (so the generated arity guard runs too).
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

/// Assert a numeric result is within a small tolerance (for f64 statistics).
fn assert_close(got: CellValue, want: f64) {
    match got {
        CellValue::Number(n) => assert!(
            (n - want).abs() < 1e-9,
            "got {n}, want ~{want} (delta {})",
            (n - want).abs()
        ),
        other => panic!("expected a number ~{want}, got {other:?}"),
    }
}

/// Build a `Vec<CellValue>` of numbers (helper for ranges).
fn nums(vs: &[f64]) -> Vec<CellValue> {
    vs.iter().copied().map(num).collect()
}

// ---- MEDIAN -----------------------------------------------------------------

#[test]
fn sheet_fn_stat_median_basic() {
    // Odd count: middle value.
    let cells = nums(&[3.0, 1.0, 2.0]);
    assert_eq!(call("MEDIAN", &[Arg::Range(row_view(&cells))]), num(2.0));
    // Even count: mean of the two middles.
    let cells = nums(&[1.0, 2.0, 3.0, 4.0]);
    assert_eq!(call("MEDIAN", &[Arg::Range(row_view(&cells))]), num(2.5));
}

#[test]
fn sheet_fn_stat_median_skips_nonnumeric_and_coerces_scalars() {
    // Range non-numerics skipped: median of {10, 20, 30} = 20.
    let cells = [num(10.0), txt("x"), num(30.0), CellValue::Empty, num(20.0)];
    assert_eq!(call("MEDIAN", &[Arg::Range(row_view(&cells))]), num(20.0));
    // Scalar bool/numeric-text coerce: MEDIAN(TRUE,"3",2) over {1,3,2} = 2.
    let args = [
        Arg::Scalar(CellValue::Bool(true)),
        Arg::Scalar(txt("3")),
        Arg::Scalar(num(2.0)),
    ];
    assert_eq!(call("MEDIAN", &args), num(2.0));
}

#[test]
fn sheet_fn_stat_median_error_and_empty() {
    // Error cell in a range propagates.
    let cells = [num(1.0), err(CellError::Div0), num(3.0)];
    assert_eq!(
        call("MEDIAN", &[Arg::Range(row_view(&cells))]),
        err(CellError::Div0)
    );
    // No numbers at all -> #NUM!.
    let none = [txt("a"), CellValue::Empty];
    assert_eq!(
        call("MEDIAN", &[Arg::Range(row_view(&none))]),
        err(CellError::Num)
    );
}

#[test]
fn sheet_fn_stat_median_arity_violation() {
    assert_eq!(call("MEDIAN", &[]), err(CellError::Value));
}

// ---- MODE -------------------------------------------------------------------

#[test]
fn sheet_fn_stat_mode_basic() {
    // 4 repeats; it is the mode.
    let cells = nums(&[1.0, 2.0, 4.0, 4.0, 5.0]);
    assert_eq!(call("MODE", &[Arg::Range(row_view(&cells))]), num(4.0));
}

#[test]
fn sheet_fn_stat_mode_tie_takes_first_seen() {
    // 2 and 3 both appear twice; 2 appears first -> mode is 2.
    let cells = nums(&[2.0, 3.0, 2.0, 3.0]);
    assert_eq!(call("MODE", &[Arg::Range(row_view(&cells))]), num(2.0));
}

#[test]
fn sheet_fn_stat_mode_no_repeat_is_na() {
    // No value repeats -> #N/A (Excel ruling).
    let cells = nums(&[1.0, 2.0, 3.0]);
    assert_eq!(
        call("MODE", &[Arg::Range(row_view(&cells))]),
        err(CellError::Na)
    );
}

#[test]
fn sheet_fn_stat_mode_arity_violation() {
    assert_eq!(call("MODE", &[]), err(CellError::Value));
}

// ---- PERCENTILE.INC ---------------------------------------------------------

#[test]
fn sheet_fn_stat_percentile_inc_interpolates() {
    // PERCENTILE.INC({1,2,3,4}, 0.25) interpolates to 1.75.
    let cells = nums(&[1.0, 2.0, 3.0, 4.0]);
    let args = [Arg::Range(row_view(&cells)), Arg::Scalar(num(0.25))];
    assert_eq!(call("PERCENTILE.INC", &args), num(1.75));
    // Endpoints: k=0 -> min, k=1 -> max.
    let args0 = [Arg::Range(row_view(&cells)), Arg::Scalar(num(0.0))];
    assert_eq!(call("PERCENTILE.INC", &args0), num(1.0));
    let args1 = [Arg::Range(row_view(&cells)), Arg::Scalar(num(1.0))];
    assert_eq!(call("PERCENTILE.INC", &args1), num(4.0));
}

#[test]
fn sheet_fn_stat_percentile_inc_k_out_of_range_is_num() {
    let cells = nums(&[1.0, 2.0, 3.0, 4.0]);
    let hi = [Arg::Range(row_view(&cells)), Arg::Scalar(num(1.5))];
    assert_eq!(call("PERCENTILE.INC", &hi), err(CellError::Num));
    let lo = [Arg::Range(row_view(&cells)), Arg::Scalar(num(-0.1))];
    assert_eq!(call("PERCENTILE.INC", &lo), err(CellError::Num));
}

#[test]
fn sheet_fn_stat_percentile_inc_coercion_and_arity() {
    // k as numeric text coerces.
    let cells = nums(&[1.0, 2.0, 3.0, 4.0]);
    let args = [Arg::Range(row_view(&cells)), Arg::Scalar(txt("0.25"))];
    assert_eq!(call("PERCENTILE.INC", &args), num(1.75));
    // arity is exactly 2.
    assert_eq!(
        call("PERCENTILE.INC", &[Arg::Range(row_view(&cells))]),
        err(CellError::Value)
    );
}

// ---- QUARTILE.INC -----------------------------------------------------------

#[test]
fn sheet_fn_stat_quartile_inc_quarters() {
    let cells = nums(&[1.0, 2.0, 3.0, 4.0]);
    // q=0 -> min, q=1 -> 1.75, q=2 -> median 2.5, q=3 -> 3.25, q=4 -> max.
    let q = |q: f64| [Arg::Range(row_view(&cells)), Arg::Scalar(num(q))];
    assert_eq!(call("QUARTILE.INC", &q(0.0)), num(1.0));
    assert_eq!(call("QUARTILE.INC", &q(1.0)), num(1.75));
    assert_eq!(call("QUARTILE.INC", &q(2.0)), num(2.5));
    assert_eq!(call("QUARTILE.INC", &q(3.0)), num(3.25));
    assert_eq!(call("QUARTILE.INC", &q(4.0)), num(4.0));
}

#[test]
fn sheet_fn_stat_quartile_inc_truncates_and_bounds() {
    let cells = nums(&[1.0, 2.0, 3.0, 4.0]);
    // q=1.9 truncates to 1.
    let trunc = [Arg::Range(row_view(&cells)), Arg::Scalar(num(1.9))];
    assert_eq!(call("QUARTILE.INC", &trunc), num(1.75));
    // q=5 (or q<0) -> #NUM!.
    let over = [Arg::Range(row_view(&cells)), Arg::Scalar(num(5.0))];
    assert_eq!(call("QUARTILE.INC", &over), err(CellError::Num));
}

#[test]
fn sheet_fn_stat_quartile_inc_arity_violation() {
    let cells = nums(&[1.0]);
    assert_eq!(
        call("QUARTILE.INC", &[Arg::Range(row_view(&cells))]),
        err(CellError::Value)
    );
}

// ---- STDEV.S / VAR.S vs STDEV.P / VAR.P + legacy aliases --------------------

const D: [f64; 8] = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];

#[test]
fn sheet_fn_stat_stdev_s_sample() {
    let cells = nums(&D);
    assert_close(
        call("STDEV.S", &[Arg::Range(row_view(&cells))]),
        2.138089935299395,
    );
}

#[test]
fn sheet_fn_stat_stdev_p_population() {
    let cells = nums(&D);
    // Population stdev of D is exactly 2.
    assert_eq!(call("STDEV.P", &[Arg::Range(row_view(&cells))]), num(2.0));
}

#[test]
fn sheet_fn_stat_var_s_sample() {
    let cells = nums(&D);
    assert_close(
        call("VAR.S", &[Arg::Range(row_view(&cells))]),
        4.571428571428571,
    );
}

#[test]
fn sheet_fn_stat_var_p_population() {
    let cells = nums(&D);
    assert_eq!(call("VAR.P", &[Arg::Range(row_view(&cells))]), num(4.0));
}

#[test]
fn sheet_fn_stat_stdev_legacy_is_sample() {
    // Legacy STDEV == STDEV.S (sample).
    let cells = nums(&D);
    assert_close(
        call("STDEV", &[Arg::Range(row_view(&cells))]),
        2.138089935299395,
    );
}

#[test]
fn sheet_fn_stat_stdevp_legacy_is_population() {
    // Legacy STDEVP == STDEV.P (population).
    let cells = nums(&D);
    assert_eq!(call("STDEVP", &[Arg::Range(row_view(&cells))]), num(2.0));
}

#[test]
fn sheet_fn_stat_var_legacy_is_sample() {
    let cells = nums(&D);
    assert_close(
        call("VAR", &[Arg::Range(row_view(&cells))]),
        4.571428571428571,
    );
}

#[test]
fn sheet_fn_stat_varp_legacy_is_population() {
    let cells = nums(&D);
    assert_eq!(call("VARP", &[Arg::Range(row_view(&cells))]), num(4.0));
}

#[test]
fn sheet_fn_stat_var_s_needs_two_else_div0() {
    // Sample estimators need >=2 numbers; one number -> #DIV/0!.
    let one = nums(&[5.0]);
    assert_eq!(
        call("VAR.S", &[Arg::Range(row_view(&one))]),
        err(CellError::Div0)
    );
    assert_eq!(
        call("STDEV.S", &[Arg::Range(row_view(&one))]),
        err(CellError::Div0)
    );
    // Population estimator is fine with one number (variance 0).
    assert_eq!(call("VAR.P", &[Arg::Range(row_view(&one))]), num(0.0));
}

#[test]
fn sheet_fn_stat_stdev_error_propagates_and_arity() {
    let cells = [num(1.0), err(CellError::Na), num(3.0)];
    assert_eq!(
        call("STDEV.S", &[Arg::Range(row_view(&cells))]),
        err(CellError::Na)
    );
    assert_eq!(call("VAR.P", &[]), err(CellError::Value));
}

// ---- LARGE / SMALL ----------------------------------------------------------

#[test]
fn sheet_fn_stat_large_basic() {
    let cells = nums(&[3.0, 1.0, 4.0, 1.0, 5.0]);
    // k=1 -> max (5), k=2 -> 4, k=3 -> 3.
    let k = |k: f64| [Arg::Range(row_view(&cells)), Arg::Scalar(num(k))];
    assert_eq!(call("LARGE", &k(1.0)), num(5.0));
    assert_eq!(call("LARGE", &k(2.0)), num(4.0));
    assert_eq!(call("LARGE", &k(3.0)), num(3.0));
}

#[test]
fn sheet_fn_stat_large_out_of_bounds_is_num() {
    let cells = nums(&[3.0, 1.0, 4.0]);
    let over = [Arg::Range(row_view(&cells)), Arg::Scalar(num(4.0))];
    assert_eq!(call("LARGE", &over), err(CellError::Num));
    let zero = [Arg::Range(row_view(&cells)), Arg::Scalar(num(0.0))];
    assert_eq!(call("LARGE", &zero), err(CellError::Num));
}

#[test]
fn sheet_fn_stat_large_arity_violation() {
    let cells = nums(&[1.0]);
    assert_eq!(
        call("LARGE", &[Arg::Range(row_view(&cells))]),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_stat_small_basic() {
    let cells = nums(&[3.0, 1.0, 4.0, 1.0, 5.0]);
    let k = |k: f64| [Arg::Range(row_view(&cells)), Arg::Scalar(num(k))];
    // k=1 -> min (1), k=2 -> 1 (the second 1), k=3 -> 3.
    assert_eq!(call("SMALL", &k(1.0)), num(1.0));
    assert_eq!(call("SMALL", &k(2.0)), num(1.0));
    assert_eq!(call("SMALL", &k(3.0)), num(3.0));
}

#[test]
fn sheet_fn_stat_small_out_of_bounds_and_arity() {
    let cells = nums(&[3.0, 1.0, 4.0]);
    let over = [Arg::Range(row_view(&cells)), Arg::Scalar(num(10.0))];
    assert_eq!(call("SMALL", &over), err(CellError::Num));
    assert_eq!(
        call("SMALL", &[Arg::Range(row_view(&cells))]),
        err(CellError::Value)
    );
}

// ---- RANK.EQ / RANK ---------------------------------------------------------

#[test]
fn sheet_fn_stat_rank_eq_descending_default() {
    let cells = nums(&[7.0, 3.5, 3.5, 1.0, 2.0]);
    // Descending: 7 is rank 1, 3.5 (tie) is rank 2, 2 is rank 4, 1 is rank 5.
    let r = |x: f64| [Arg::Scalar(num(x)), Arg::Range(row_view(&cells))];
    assert_eq!(call("RANK.EQ", &r(7.0)), num(1.0));
    assert_eq!(call("RANK.EQ", &r(3.5)), num(2.0)); // ties share top rank
    assert_eq!(call("RANK.EQ", &r(2.0)), num(4.0)); // tie skips rank 3
    assert_eq!(call("RANK.EQ", &r(1.0)), num(5.0));
}

#[test]
fn sheet_fn_stat_rank_eq_ascending_order_arg() {
    let cells = nums(&[7.0, 3.0, 1.0, 2.0]);
    // order=1 (ascending): 1 is rank 1, 7 is rank 4.
    let asc = |x: f64| {
        [
            Arg::Scalar(num(x)),
            Arg::Range(row_view(&cells)),
            Arg::Scalar(num(1.0)),
        ]
    };
    assert_eq!(call("RANK.EQ", &asc(1.0)), num(1.0));
    assert_eq!(call("RANK.EQ", &asc(7.0)), num(4.0));
}

#[test]
fn sheet_fn_stat_rank_eq_absent_is_na() {
    let cells = nums(&[7.0, 3.0, 1.0]);
    let r = [Arg::Scalar(num(99.0)), Arg::Range(row_view(&cells))];
    assert_eq!(call("RANK.EQ", &r), err(CellError::Na));
}

#[test]
fn sheet_fn_stat_rank_eq_arity_violation() {
    assert_eq!(
        call("RANK.EQ", &[Arg::Scalar(num(1.0))]),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_stat_rank_legacy_matches_rank_eq() {
    // Legacy RANK == RANK.EQ.
    let cells = nums(&[7.0, 3.5, 3.5, 1.0]);
    let r = |x: f64| [Arg::Scalar(num(x)), Arg::Range(row_view(&cells))];
    assert_eq!(call("RANK", &r(3.5)), num(2.0));
    assert_eq!(call("RANK", &r(7.0)), num(1.0));
}

// ---- COUNTIFS ---------------------------------------------------------------

#[test]
fn sheet_fn_stat_countifs_and_across_pairs() {
    // region {N,N,S,N}, sales {>100? } — count rows where region="N" AND sales>100.
    let region = [txt("N"), txt("N"), txt("S"), txt("N")];
    let sales = nums(&[150.0, 90.0, 200.0, 120.0]);
    let args = [
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("N")),
        Arg::Range(row_view(&sales)),
        Arg::Scalar(txt(">100")),
    ];
    // N&>100: row0 (150) and row3 (120) -> 2.
    assert_eq!(call("COUNTIFS", &args), num(2.0));
}

#[test]
fn sheet_fn_stat_countifs_single_pair_like_countif() {
    let cells = nums(&[1.0, 2.0, 3.0, 4.0]);
    let args = [Arg::Range(row_view(&cells)), Arg::Scalar(txt(">2"))];
    assert_eq!(call("COUNTIFS", &args), num(2.0));
}

#[test]
fn sheet_fn_stat_countifs_shape_mismatch_is_value() {
    let a = nums(&[1.0, 2.0, 3.0]);
    let b = nums(&[1.0, 2.0]); // different shape
    let args = [
        Arg::Range(row_view(&a)),
        Arg::Scalar(txt(">0")),
        Arg::Range(row_view(&b)),
        Arg::Scalar(txt(">0")),
    ];
    assert_eq!(call("COUNTIFS", &args), err(CellError::Value));
}

#[test]
fn sheet_fn_stat_countifs_arity_violation() {
    // Odd arg count (a dangling range with no criterion) -> #VALUE!.
    let cells = nums(&[1.0, 2.0]);
    assert_eq!(
        call("COUNTIFS", &[Arg::Range(row_view(&cells))]),
        err(CellError::Value)
    );
}

// ---- SUMIFS -----------------------------------------------------------------

#[test]
fn sheet_fn_stat_sumifs_basic() {
    // sum_range first, then (criteria_range, criteria) pairs.
    let amount = nums(&[10.0, 20.0, 30.0, 40.0]);
    let region = [txt("N"), txt("S"), txt("N"), txt("N")];
    let args = [
        Arg::Range(row_view(&amount)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("N")),
    ];
    // N rows: 10 + 30 + 40 = 80.
    assert_eq!(call("SUMIFS", &args), num(80.0));
}

#[test]
fn sheet_fn_stat_sumifs_two_pairs_and() {
    let amount = nums(&[10.0, 20.0, 30.0, 40.0]);
    let region = [txt("N"), txt("N"), txt("S"), txt("N")];
    let flag = nums(&[1.0, 0.0, 1.0, 1.0]);
    let args = [
        Arg::Range(row_view(&amount)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("N")),
        Arg::Range(row_view(&flag)),
        Arg::Scalar(num(1.0)),
    ];
    // region=N AND flag=1: rows 0 (10) and 3 (40) -> 50.
    assert_eq!(call("SUMIFS", &args), num(50.0));
}

#[test]
fn sheet_fn_stat_sumifs_empty_match_is_zero_and_error_propagates() {
    let amount = nums(&[10.0, 20.0]);
    let region = [txt("N"), txt("N")];
    // No "Z" rows -> sum 0.
    let none = [
        Arg::Range(row_view(&amount)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("Z")),
    ];
    assert_eq!(call("SUMIFS", &none), num(0.0));
    // An error in a CONTRIBUTING value cell propagates.
    let amount_e = [num(10.0), err(CellError::Div0)];
    let args = [
        Arg::Range(row_view(&amount_e)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("N")),
    ];
    assert_eq!(call("SUMIFS", &args), err(CellError::Div0));
}

#[test]
fn sheet_fn_stat_sumifs_arity_violation() {
    // min arity 3; two args is #VALUE!.
    let cells = nums(&[1.0, 2.0]);
    let two = [Arg::Range(row_view(&cells)), Arg::Range(row_view(&cells))];
    assert_eq!(call("SUMIFS", &two), err(CellError::Value));
}

// ---- AVERAGEIFS -------------------------------------------------------------

#[test]
fn sheet_fn_stat_averageifs_basic() {
    let amount = nums(&[10.0, 20.0, 30.0, 40.0]);
    let region = [txt("N"), txt("S"), txt("N"), txt("N")];
    let args = [
        Arg::Range(row_view(&amount)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("N")),
    ];
    // N rows: (10+30+40)/3 = 80/3.
    assert_close(call("AVERAGEIFS", &args), 80.0 / 3.0);
}

#[test]
fn sheet_fn_stat_averageifs_empty_match_is_div0() {
    let amount = nums(&[10.0, 20.0]);
    let region = [txt("N"), txt("N")];
    let none = [
        Arg::Range(row_view(&amount)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("Z")),
    ];
    // No matches -> #DIV/0! (the named Excel ruling).
    assert_eq!(call("AVERAGEIFS", &none), err(CellError::Div0));
}

#[test]
fn sheet_fn_stat_averageifs_arity_violation() {
    let cells = nums(&[1.0, 2.0]);
    let two = [Arg::Range(row_view(&cells)), Arg::Range(row_view(&cells))];
    assert_eq!(call("AVERAGEIFS", &two), err(CellError::Value));
}

// ---- MAXIFS / MINIFS --------------------------------------------------------

#[test]
fn sheet_fn_stat_maxifs_basic() {
    let amount = nums(&[10.0, 20.0, 30.0, 5.0]);
    let region = [txt("N"), txt("S"), txt("N"), txt("N")];
    let args = [
        Arg::Range(row_view(&amount)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("N")),
    ];
    // N rows {10,30,5} -> max 30.
    assert_eq!(call("MAXIFS", &args), num(30.0));
}

#[test]
fn sheet_fn_stat_maxifs_empty_match_is_zero_and_arity() {
    let amount = nums(&[10.0, 20.0]);
    let region = [txt("N"), txt("N")];
    let none = [
        Arg::Range(row_view(&amount)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("Z")),
    ];
    assert_eq!(call("MAXIFS", &none), num(0.0));
    let two = [Arg::Range(row_view(&amount)), Arg::Range(row_view(&region))];
    assert_eq!(call("MAXIFS", &two), err(CellError::Value));
}

#[test]
fn sheet_fn_stat_minifs_basic() {
    let amount = nums(&[10.0, 20.0, 30.0, 5.0]);
    let region = [txt("N"), txt("S"), txt("N"), txt("N")];
    let args = [
        Arg::Range(row_view(&amount)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("N")),
    ];
    // N rows {10,30,5} -> min 5.
    assert_eq!(call("MINIFS", &args), num(5.0));
}

#[test]
fn sheet_fn_stat_minifs_empty_match_is_zero_and_arity() {
    let amount = nums(&[10.0, 20.0]);
    let region = [txt("N"), txt("N")];
    let none = [
        Arg::Range(row_view(&amount)),
        Arg::Range(row_view(&region)),
        Arg::Scalar(txt("Z")),
    ];
    assert_eq!(call("MINIFS", &none), num(0.0));
    let two = [Arg::Range(row_view(&amount)), Arg::Range(row_view(&region))];
    assert_eq!(call("MINIFS", &two), err(CellError::Value));
}

// ---- SUMPRODUCT -------------------------------------------------------------

#[test]
fn sheet_fn_stat_sumproduct_basic() {
    let a = nums(&[1.0, 2.0, 3.0]);
    let b = nums(&[4.0, 5.0, 6.0]);
    let args = [Arg::Range(row_view(&a)), Arg::Range(row_view(&b))];
    // 1*4 + 2*5 + 3*6 = 32.
    assert_eq!(call("SUMPRODUCT", &args), num(32.0));
}

#[test]
fn sheet_fn_stat_sumproduct_nonnumeric_contributes_zero() {
    // A non-numeric cell zeroes its product term (Excel).
    let a = [num(1.0), txt("x"), num(3.0)];
    let b = nums(&[4.0, 5.0, 6.0]);
    let args = [Arg::Range(row_view(&a)), Arg::Range(row_view(&b))];
    // 1*4 + 0 + 3*6 = 22.
    assert_eq!(call("SUMPRODUCT", &args), num(22.0));
}

#[test]
fn sheet_fn_stat_sumproduct_shape_mismatch_is_value() {
    let a = nums(&[1.0, 2.0, 3.0]);
    let b = nums(&[4.0, 5.0]);
    let args = [Arg::Range(row_view(&a)), Arg::Range(row_view(&b))];
    assert_eq!(call("SUMPRODUCT", &args), err(CellError::Value));
}

#[test]
fn sheet_fn_stat_sumproduct_single_array_and_error() {
    // Single array -> sum of numeric cells.
    let a = nums(&[2.0, 4.0, 6.0]);
    assert_eq!(call("SUMPRODUCT", &[Arg::Range(row_view(&a))]), num(12.0));
    // Error cell propagates.
    let e = [num(1.0), err(CellError::Ref)];
    let f = nums(&[1.0, 1.0]);
    let args = [Arg::Range(row_view(&e)), Arg::Range(row_view(&f))];
    assert_eq!(call("SUMPRODUCT", &args), err(CellError::Ref));
}

// ---- GEOMEAN / HARMEAN ------------------------------------------------------

#[test]
fn sheet_fn_stat_geomean_basic() {
    let cells = nums(&[1.0, 2.0, 4.0, 8.0]);
    // Geometric mean of {1,2,4,8} = 2^1.5 = 2.8284271247...
    assert_close(
        call("GEOMEAN", &[Arg::Range(row_view(&cells))]),
        2.8284271247461903,
    );
}

#[test]
fn sheet_fn_stat_geomean_nonpositive_is_num() {
    let cells = nums(&[1.0, 0.0, 4.0]);
    assert_eq!(
        call("GEOMEAN", &[Arg::Range(row_view(&cells))]),
        err(CellError::Num)
    );
    let neg = nums(&[1.0, -2.0]);
    assert_eq!(
        call("GEOMEAN", &[Arg::Range(row_view(&neg))]),
        err(CellError::Num)
    );
}

#[test]
fn sheet_fn_stat_geomean_arity_violation() {
    assert_eq!(call("GEOMEAN", &[]), err(CellError::Value));
}

#[test]
fn sheet_fn_stat_harmean_basic() {
    let cells = nums(&[1.0, 2.0, 4.0]);
    // Harmonic mean = 3 / (1 + 0.5 + 0.25) = 12/7.
    assert_close(call("HARMEAN", &[Arg::Range(row_view(&cells))]), 12.0 / 7.0);
}

#[test]
fn sheet_fn_stat_harmean_nonpositive_and_arity() {
    let cells = nums(&[1.0, -1.0]);
    assert_eq!(
        call("HARMEAN", &[Arg::Range(row_view(&cells))]),
        err(CellError::Num)
    );
    assert_eq!(call("HARMEAN", &[]), err(CellError::Value));
}

// ---- AVEDEV / DEVSQ ---------------------------------------------------------

#[test]
fn sheet_fn_stat_avedev_basic() {
    let cells = nums(&[2.0, 4.0, 8.0]);
    // mean=14/3; mean abs dev = (8/3 + 2/3 + 10/3)/3 = (20/3)/3 = 20/9.
    assert_close(call("AVEDEV", &[Arg::Range(row_view(&cells))]), 20.0 / 9.0);
}

#[test]
fn sheet_fn_stat_avedev_empty_and_arity() {
    let none = [txt("a"), CellValue::Empty];
    assert_eq!(
        call("AVEDEV", &[Arg::Range(row_view(&none))]),
        err(CellError::Num)
    );
    assert_eq!(call("AVEDEV", &[]), err(CellError::Value));
}

#[test]
fn sheet_fn_stat_devsq_basic() {
    let cells = nums(&[2.0, 4.0, 8.0]);
    // mean=14/3; Σ(x-mean)² = 56/3.
    assert_close(call("DEVSQ", &[Arg::Range(row_view(&cells))]), 56.0 / 3.0);
}

#[test]
fn sheet_fn_stat_devsq_error_and_arity() {
    let cells = [num(1.0), err(CellError::Num), num(3.0)];
    assert_eq!(
        call("DEVSQ", &[Arg::Range(row_view(&cells))]),
        err(CellError::Num)
    );
    assert_eq!(call("DEVSQ", &[]), err(CellError::Value));
}

// ---- CORREL / SLOPE / INTERCEPT / RSQ --------------------------------------

const Y: [f64; 5] = [2.0, 3.0, 5.0, 4.0, 6.0];
const X: [f64; 5] = [1.0, 2.0, 3.0, 4.0, 5.0];

#[test]
fn sheet_fn_stat_correl_basic() {
    let y = nums(&Y);
    let x = nums(&X);
    let args = [Arg::Range(row_view(&y)), Arg::Range(row_view(&x))];
    assert_close(call("CORREL", &args), 0.9);
}

#[test]
fn sheet_fn_stat_correl_degenerate_is_div0_and_shape_is_na() {
    // Constant x -> zero variance -> #DIV/0!.
    let y = nums(&[1.0, 2.0, 3.0]);
    let xc = nums(&[5.0, 5.0, 5.0]);
    let deg = [Arg::Range(row_view(&y)), Arg::Range(row_view(&xc))];
    assert_eq!(call("CORREL", &deg), err(CellError::Div0));
    // Shape mismatch -> #N/A.
    let xs = nums(&[1.0, 2.0]);
    let bad = [Arg::Range(row_view(&y)), Arg::Range(row_view(&xs))];
    assert_eq!(call("CORREL", &bad), err(CellError::Na));
}

#[test]
fn sheet_fn_stat_correl_arity_violation() {
    let y = nums(&Y);
    assert_eq!(
        call("CORREL", &[Arg::Range(row_view(&y))]),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_stat_slope_basic() {
    let y = nums(&Y);
    let x = nums(&X);
    let args = [Arg::Range(row_view(&y)), Arg::Range(row_view(&x))];
    assert_close(call("SLOPE", &args), 0.9);
}

#[test]
fn sheet_fn_stat_slope_degenerate_and_arity() {
    let y = nums(&[1.0, 2.0, 3.0]);
    let xc = nums(&[5.0, 5.0, 5.0]);
    let deg = [Arg::Range(row_view(&y)), Arg::Range(row_view(&xc))];
    assert_eq!(call("SLOPE", &deg), err(CellError::Div0));
    assert_eq!(
        call("SLOPE", &[Arg::Range(row_view(&y))]),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_stat_intercept_basic() {
    let y = nums(&Y);
    let x = nums(&X);
    let args = [Arg::Range(row_view(&y)), Arg::Range(row_view(&x))];
    assert_close(call("INTERCEPT", &args), 1.3);
}

#[test]
fn sheet_fn_stat_intercept_degenerate_and_arity() {
    let y = nums(&[1.0, 2.0, 3.0]);
    let xc = nums(&[5.0, 5.0, 5.0]);
    let deg = [Arg::Range(row_view(&y)), Arg::Range(row_view(&xc))];
    assert_eq!(call("INTERCEPT", &deg), err(CellError::Div0));
    assert_eq!(
        call("INTERCEPT", &[Arg::Range(row_view(&y))]),
        err(CellError::Value)
    );
}

#[test]
fn sheet_fn_stat_rsq_basic() {
    let y = nums(&Y);
    let x = nums(&X);
    let args = [Arg::Range(row_view(&y)), Arg::Range(row_view(&x))];
    // RSQ = CORREL² = 0.81.
    assert_close(call("RSQ", &args), 0.81);
}

#[test]
fn sheet_fn_stat_rsq_degenerate_and_arity() {
    let y = nums(&[1.0, 2.0, 3.0]);
    let xc = nums(&[5.0, 5.0, 5.0]);
    let deg = [Arg::Range(row_view(&y)), Arg::Range(row_view(&xc))];
    assert_eq!(call("RSQ", &deg), err(CellError::Div0));
    assert_eq!(
        call("RSQ", &[Arg::Range(row_view(&y))]),
        err(CellError::Value)
    );
}
