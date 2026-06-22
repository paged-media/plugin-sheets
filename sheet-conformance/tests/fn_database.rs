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

//! Database (D-) family conformance (spec §7, §11 T2; ECMA-376 §18.17.7).
//! Self-contained direct-dispatch tests: each call resolves the registry name
//! through `sheet_core::funcs::lookup_func`, builds the `&[Arg]` slice, and
//! routes it through `sheet_fn::dispatch` — exactly the path `sheet-calc`
//! takes, so the generated arity guard (exactly 3 args) runs alongside the
//! kernel.
//!
//! Every test fn is named with the prefix the `registry/functions/database.yaml`
//! rows point at (`sheet_fn_database_<name>`), so the coverage gate (§12.2)
//! finds it. The `.golden.tsv` files under `corpus/fn-corpus/database/` replay
//! the same families end-to-end through `corpus_runner.rs`.
//!
//! ## The shared shape under test
//!
//! All twelve take `(database, field, criteria)` as RANGE arguments:
//! - `database` first row = headers, remaining rows = records;
//! - `field` = a header label, a 1-based column number, or a 1×1 header ref;
//! - `criteria` first row = headers, condition rows AND within a row / OR
//!   across rows; a blank criteria cell imposes no constraint.
//!
//! ## The named Excel rulings exercised here
//!
//! - field-by-header (case-insensitive) AND field-by-1-based-number agree;
//! - field out of range / blank / bool → `#VALUE!`;
//! - AND-within-row, OR-across-rows criteria; blank cell = no constraint;
//! - a header-only criteria table matches every record;
//! - `DCOUNT` numeric-only; `DCOUNTA` non-blank; `DMAX`/`DMIN` of no numbers
//!   → `0`; `DAVERAGE` of no numbers → `#DIV/0!`;
//! - `DSTDEV`/`DVAR` sample (`n−1`, `#DIV/0!` at `n≤1`); `DSTDEVP`/`DVARP`
//!   population (`n`);
//! - `DGET`: no match → `#VALUE!`, multiple matches → `#NUM!`;
//! - an error in a contributing field cell propagates (DSUM); `DCOUNT` does
//!   not propagate.

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

/// A fixed, deterministic context (the database family is non-volatile, so the
/// clock/RNG never matter — but the convention pins them anyway).
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

/// A range view over a row-major buffer, origin A1.
fn view(rows: u32, cols: u32, cells: &[CellValue]) -> RangeView<'_> {
    RangeView::from_slice(cr(0, 0), rows, cols, cells)
}

/// Assert a value is `Number` within an absolute tolerance (for stdev/var).
fn assert_close(got: CellValue, want: f64) {
    match got {
        CellValue::Number(n) => assert!(
            (n - want).abs() < 1e-9,
            "expected ≈{want}, got {n} (Δ={})",
            (n - want).abs()
        ),
        other => panic!("expected a number ≈{want}, got {other:?}"),
    }
}

// ---- shared fixtures --------------------------------------------------------

/// The canonical sales table (4 columns × header + 5 records):
///   Tree   | Height | Age | Yield
///   Apple  | 18     | 20  | 14
///   Pear   | 12     | 12  | 10
///   Cherry | 13     | 14  | 9
///   Apple  | 14     | 15  | 10
///   Pear   | 9      | 8   | 8
/// (The ECMA-376 §18.17.7 worked example shape.)
fn orchard() -> Vec<CellValue> {
    vec![
        txt("Tree"),
        txt("Height"),
        txt("Age"),
        txt("Yield"),
        txt("Apple"),
        num(18.0),
        num(20.0),
        num(14.0),
        txt("Pear"),
        num(12.0),
        num(12.0),
        num(10.0),
        txt("Cherry"),
        num(13.0),
        num(14.0),
        num(9.0),
        txt("Apple"),
        num(14.0),
        num(15.0),
        num(10.0),
        txt("Pear"),
        num(9.0),
        num(8.0),
        num(8.0),
    ]
}
const ORCHARD_ROWS: u32 = 6;
const ORCHARD_COLS: u32 = 4;

/// Criteria: a single header `Tree` over one condition row `Apple`.
fn crit_tree_apple() -> Vec<CellValue> {
    vec![txt("Tree"), txt("Apple")]
}

/// A header-only criteria table (one column, no condition rows) → matches all.
fn crit_all() -> Vec<CellValue> {
    vec![txt("Tree")]
}

// ---- DSUM -------------------------------------------------------------------

#[test]
fn sheet_fn_database_dsum_by_header() {
    // Sum Yield for Apple rows: 14 + 10 = 24. field selected by header label.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(24.0));
}

#[test]
fn sheet_fn_database_dsum_field_by_number_agrees() {
    // field = 4 (1-based "Yield") gives the same answer as the header label.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(num(4.0)),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(24.0));
}

#[test]
fn sheet_fn_database_dsum_header_match_is_case_insensitive() {
    // "yIeLd" still selects the Yield column.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("yIeLd")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(24.0));
}

#[test]
fn sheet_fn_database_dsum_error_in_field_cell_propagates() {
    // Replace an Apple Yield with #DIV/0! — DSUM propagates it.
    let mut db = orchard();
    // Apple row 1 (record 0), Yield col 3 → flat index 1*4 + 3 = 7.
    db[7] = err(CellError::Div0);
    let crit = crit_tree_apple();
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, err(CellError::Div0));
}

#[test]
fn sheet_fn_database_dsum_arity_guard_fires() {
    // Two args (not three): the generated arity guard returns #VALUE! before
    // the kernel runs.
    let db = orchard();
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
        ],
    );
    assert_eq!(out, err(CellError::Value));
}

// ---- DCOUNT -----------------------------------------------------------------

#[test]
fn sheet_fn_database_dcount_numeric_only() {
    // DCOUNT(Age, Tree=Apple): both Apple rows have a numeric Age → 2.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DCOUNT",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Age")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(2.0));
}

#[test]
fn sheet_fn_database_dcount_text_field_counts_zero() {
    // Counting the TEXT "Tree" column gives 0 (DCOUNT counts numbers only),
    // even though every matching record has a (text) Tree value.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DCOUNT",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Tree")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(0.0));
}

#[test]
fn sheet_fn_database_dcount_does_not_propagate_error() {
    // An error in a matching field cell is NOT counted and NOT propagated
    // (DCOUNT mirrors COUNT). Apple row 0 Yield → index 7.
    let mut db = orchard();
    db[7] = err(CellError::Div0);
    let crit = crit_tree_apple();
    let out = call(
        "DCOUNT",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    // One Apple Yield is an error (skipped), one is numeric → count 1.
    assert_eq!(out, num(1.0));
}

// ---- DCOUNTA ----------------------------------------------------------------

#[test]
fn sheet_fn_database_dcounta_non_blank() {
    // DCOUNTA over the Tree (text) column for Apple → 2 (text counts).
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DCOUNTA",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Tree")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(2.0));
}

#[test]
fn sheet_fn_database_dcounta_skips_blank_field_cell() {
    // Blank one Apple's Height; DCOUNTA over Height for Apple → 1.
    let mut db = orchard();
    // Apple record 0, Height col 1 → index 1*4 + 1 = 5.
    db[5] = CellValue::Empty;
    let crit = crit_tree_apple();
    let out = call(
        "DCOUNTA",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Height")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(1.0));
}

// ---- DGET -------------------------------------------------------------------

#[test]
fn sheet_fn_database_dget_single_match() {
    // Tree=Cherry has exactly one record; DGET its Yield → 9.
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Cherry")];
    let out = call(
        "DGET",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(9.0));
}

#[test]
fn sheet_fn_database_dget_no_match_is_value() {
    // Tree=Plum matches nothing → #VALUE!.
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Plum")];
    let out = call(
        "DGET",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, err(CellError::Value));
}

#[test]
fn sheet_fn_database_dget_multiple_match_is_num() {
    // Tree=Apple matches two records → #NUM!.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DGET",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, err(CellError::Num));
}

#[test]
fn sheet_fn_database_dget_returns_text_field_verbatim() {
    // DGET of a text field returns the text value, not a number.
    let db = orchard();
    let crit = vec![txt("Yield"), txt("9")];
    let out = call(
        "DGET",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Tree")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, txt("Cherry"));
}

// ---- DMAX / DMIN ------------------------------------------------------------

#[test]
fn sheet_fn_database_dmax_basic() {
    // Max Height where Tree=Apple: max(18, 14) = 18.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DMAX",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Height")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(18.0));
}

#[test]
fn sheet_fn_database_dmax_no_match_is_zero() {
    // No matching record → DMAX is 0 (Excel, like MAX of nothing).
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Plum")];
    let out = call(
        "DMAX",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Height")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(0.0));
}

#[test]
fn sheet_fn_database_dmin_basic() {
    // Min Height where Tree=Apple: min(18, 14) = 14.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DMIN",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Height")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(14.0));
}

#[test]
fn sheet_fn_database_dmin_no_match_is_zero() {
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Plum")];
    let out = call(
        "DMIN",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Height")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(0.0));
}

// ---- DAVERAGE ---------------------------------------------------------------

#[test]
fn sheet_fn_database_daverage_basic() {
    // Average Age where Tree=Apple: (20 + 15) / 2 = 17.5.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DAVERAGE",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Age")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(17.5));
}

#[test]
fn sheet_fn_database_daverage_no_match_is_div0() {
    // No matching record → #DIV/0! (Excel).
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Plum")];
    let out = call(
        "DAVERAGE",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Age")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, err(CellError::Div0));
}

// ---- DPRODUCT ---------------------------------------------------------------

#[test]
fn sheet_fn_database_dproduct_basic() {
    // Product of Yield where Tree=Apple: 14 * 10 = 140.
    let db = orchard();
    let crit = crit_tree_apple();
    let out = call(
        "DPRODUCT",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(140.0));
}

#[test]
fn sheet_fn_database_dproduct_no_match_is_zero() {
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Plum")];
    let out = call(
        "DPRODUCT",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(0.0));
}

// ---- DSTDEV / DSTDEVP / DVAR / DVARP ----------------------------------------

/// All-records criteria → the field over the full table. Yield = {14,10,9,10,8}.
/// mean = 10.2; Σ(x−m)² = (3.8² + .2² + 1.2² + .2² + 2.2²)
///   = 14.44 + 0.04 + 1.44 + 0.04 + 4.84 = 20.8.
/// sample var = 20.8/4 = 5.2 ; population var = 20.8/5 = 4.16.
#[test]
fn sheet_fn_database_dvar_sample() {
    let db = orchard();
    let crit = crit_all();
    let out = call(
        "DVAR",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(1, 1, &crit)),
        ],
    );
    assert_close(out, 5.2);
}

#[test]
fn sheet_fn_database_dvarp_population() {
    let db = orchard();
    let crit = crit_all();
    let out = call(
        "DVARP",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(1, 1, &crit)),
        ],
    );
    assert_close(out, 4.16);
}

#[test]
fn sheet_fn_database_dstdev_sample() {
    let db = orchard();
    let crit = crit_all();
    let out = call(
        "DSTDEV",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(1, 1, &crit)),
        ],
    );
    assert_close(out, 5.2_f64.sqrt());
}

#[test]
fn sheet_fn_database_dstdevp_population() {
    let db = orchard();
    let crit = crit_all();
    let out = call(
        "DSTDEVP",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(1, 1, &crit)),
        ],
    );
    assert_close(out, 4.16_f64.sqrt());
}

#[test]
fn sheet_fn_database_dstdev_single_value_is_div0() {
    // Sample stdev needs n≥2: Tree=Cherry has one record → #DIV/0!.
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Cherry")];
    let out = call(
        "DSTDEV",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, err(CellError::Div0));
}

#[test]
fn sheet_fn_database_dvar_single_value_is_div0() {
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Cherry")];
    let out = call(
        "DVAR",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, err(CellError::Div0));
}

// ---- criteria grammar -------------------------------------------------------

#[test]
fn sheet_fn_database_criteria_and_within_row() {
    // AND within a row: Tree=Apple AND Height>15 → only the (18,20,14) record.
    // DSUM Yield → 14.
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Height"), txt("Apple"), txt(">15")];
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 2, &crit)),
        ],
    );
    assert_eq!(out, num(14.0));
}

#[test]
fn sheet_fn_database_criteria_or_across_rows() {
    // OR across rows: Tree=Apple OR Tree=Cherry.
    // DSUM Yield → 14 + 10 (Apples) + 9 (Cherry) = 33.
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Apple"), txt("Cherry")];
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(3, 1, &crit)),
        ],
    );
    assert_eq!(out, num(33.0));
}

#[test]
fn sheet_fn_database_criteria_blank_cell_no_constraint() {
    // A blank criteria cell imposes no constraint: header {Tree, Height},
    // row {Apple, blank} → just Tree=Apple. DSUM Yield → 24.
    let db = orchard();
    let crit = vec![txt("Tree"), txt("Height"), txt("Apple"), CellValue::Empty];
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 2, &crit)),
        ],
    );
    assert_eq!(out, num(24.0));
}

#[test]
fn sheet_fn_database_criteria_comparison_operator() {
    // Numeric comparison criterion: Age>=14. Records with Age≥14: 20,14,15.
    // DSUM Yield over those rows: Apple(14)+Cherry(9)+Apple(10) = 33.
    let db = orchard();
    let crit = vec![txt("Age"), txt(">=14")];
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(33.0));
}

#[test]
fn sheet_fn_database_criteria_header_only_matches_all() {
    // A header-only criteria table matches every record:
    // DSUM Yield over all = 14+10+9+10+8 = 51.
    let db = orchard();
    let crit = crit_all();
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Yield")),
            Arg::Range(view(1, 1, &crit)),
        ],
    );
    assert_eq!(out, num(51.0));
}

// ---- field resolution errors ------------------------------------------------

#[test]
fn sheet_fn_database_field_out_of_range_is_value() {
    let db = orchard();
    let crit = crit_all();
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(num(99.0)),
            Arg::Range(view(1, 1, &crit)),
        ],
    );
    assert_eq!(out, err(CellError::Value));
}

#[test]
fn sheet_fn_database_field_unknown_header_is_value() {
    let db = orchard();
    let crit = crit_all();
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Scalar(txt("Nope")),
            Arg::Range(view(1, 1, &crit)),
        ],
    );
    assert_eq!(out, err(CellError::Value));
}

#[test]
fn sheet_fn_database_field_as_one_by_one_range() {
    // A 1×1 RANGE field (a cell reference to a header) resolves like the
    // scalar header label.
    let db = orchard();
    let crit = crit_tree_apple();
    let field_cell = [txt("Yield")];
    let out = call(
        "DSUM",
        &[
            Arg::Range(view(ORCHARD_ROWS, ORCHARD_COLS, &db)),
            Arg::Range(view(1, 1, &field_cell)),
            Arg::Range(view(2, 1, &crit)),
        ],
    );
    assert_eq!(out, num(24.0));
}
