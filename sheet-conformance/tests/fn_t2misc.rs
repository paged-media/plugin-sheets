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

//! Misc T2 family (`t2misc`) conformance (spec §7/§11 T2). SELF-CONTAINED
//! direct-dispatch tests: each case resolves the function name through
//! `sheet_core::funcs::lookup_func` and routes the call through
//! [`sheet_fn::dispatch`] — the same choke point a real evaluation crosses, so
//! the generated arity guard (`#VALUE!` on violation) is exercised end-to-end.
//!
//! Every function gets at least one `fn sheet_fn_t2misc_<name>…` test (the
//! prefix the `registry/functions/t2misc.yaml` rows point at, which the
//! coverage gate greps for). Cases cover happy path, coercion, error
//! propagation, range behavior, arity violation, and each named T2 ruling:
//!
//! - **AGGREGATE**: function_num 1–11 selectors; options 0/4 (ignore nothing,
//!   errors propagate) vs 6 (ignore errors); deferred function_num 12–19 and
//!   the hidden-row options 1/2/3/5/7 → `#VALUE!`.
//! - **SUBTOTAL**: 1–11 == 101–111 (no row-visibility metadata); error
//!   propagation; bad function_num → `#VALUE!`.
//! - **ROMAN**: classic numerals; `0`→`""`; out-of-domain → `#VALUE!`; `form`
//!   0–4 accepted (renders classic).
//! - **ARABIC**: inverse of classic ROMAN; `""`→0; leading `-` negates;
//!   non-roman → `#VALUE!`.
//! - **CONVERT**: the basic unit set; cross-dimension / unknown unit → `#N/A`;
//!   case-sensitive units; temperature affine pivot.
//! - **HYPERLINK**: display-only — friendly text, else the link text.

use sheet_core::{CellError, CellRef, CellValue, DateSystem};
use sheet_fn::arg::{Arg, RangeView};
use sheet_fn::{dispatch, EvalCtx};

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

/// A deterministic context (the family is non-volatile; the clock/RNG never
/// matter, but the convention pins them).
fn ctx() -> EvalCtx {
    EvalCtx::new(DateSystem::Date1900, cell(), 45000.5, 42)
}

/// Dispatch a function by registry name with the given args (so the generated
/// arity guard runs too).
fn call(name: &str, args: &[Arg]) -> CellValue {
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
fn err(e: CellError) -> Arg<'static> {
    Arg::Scalar(CellValue::Error(e))
}

fn n(x: f64) -> CellValue {
    CellValue::Number(x)
}
fn t(s: &str) -> CellValue {
    CellValue::from(s)
}
fn e(e: CellError) -> CellValue {
    CellValue::Error(e)
}

/// A 1-row range view over `cells` (origin A1), borrowing the slice.
fn row_view(cells: &[CellValue]) -> RangeView<'_> {
    RangeView::from_slice(cell(), 1, cells.len() as u32, cells)
}

// ============================================================================
// AGGREGATE
// ============================================================================

#[test]
fn sheet_fn_t2misc_aggregate_inner_selectors() {
    let cells = [n(2.0), n(4.0), n(6.0), n(8.0)];
    let view = || Arg::Range(row_view(&cells));
    // 1=AVERAGE, 4=MAX, 5=MIN, 6=PRODUCT, 9=SUM (options 0 = ignore nothing).
    assert_eq!(call("AGGREGATE", &[num(1.0), num(0.0), view()]), n(5.0));
    assert_eq!(call("AGGREGATE", &[num(4.0), num(0.0), view()]), n(8.0));
    assert_eq!(call("AGGREGATE", &[num(5.0), num(0.0), view()]), n(2.0));
    assert_eq!(call("AGGREGATE", &[num(6.0), num(0.0), view()]), n(384.0));
    assert_eq!(call("AGGREGATE", &[num(9.0), num(0.0), view()]), n(20.0));
}

#[test]
fn sheet_fn_t2misc_aggregate_count_variants() {
    // 2=COUNT (numbers only), 3=COUNTA (non-blank) over a mixed range.
    let cells = [
        n(1.0),
        t("x"),
        CellValue::Bool(true),
        CellValue::Empty,
        n(9.0),
    ];
    let view = || Arg::Range(row_view(&cells));
    // COUNT counts numbers only (1, 9) = 2; COUNTA counts non-blank cells
    // (1, "x", TRUE, 9 — Empty is blank) = 4.
    assert_eq!(call("AGGREGATE", &[num(2.0), num(0.0), view()]), n(2.0));
    assert_eq!(call("AGGREGATE", &[num(3.0), num(0.0), view()]), n(4.0));
}

#[test]
fn sheet_fn_t2misc_aggregate_stat_selectors() {
    // 8=STDEV.P, 11=VAR.P over {2,4,4,4,5,5,7,9}: VARP=4, STDEVP=2.
    let cells = [
        n(2.0),
        n(4.0),
        n(4.0),
        n(4.0),
        n(5.0),
        n(5.0),
        n(7.0),
        n(9.0),
    ];
    let view = || Arg::Range(row_view(&cells));
    assert_eq!(call("AGGREGATE", &[num(11.0), num(0.0), view()]), n(4.0));
    assert_eq!(call("AGGREGATE", &[num(8.0), num(0.0), view()]), n(2.0));
    // 7=STDEV.S, 10=VAR.S need n-1; a single point -> #DIV/0!.
    assert_eq!(
        call("AGGREGATE", &[num(10.0), num(0.0), num(5.0)]),
        e(CellError::Div0)
    );
}

#[test]
fn sheet_fn_t2misc_aggregate_option0_propagates_error() {
    // Option 0/4 = ignore nothing: an error cell in a ref IS the result.
    let cells = [n(1.0), e(CellError::Div0), n(3.0)];
    let view = || Arg::Range(row_view(&cells));
    assert_eq!(
        call("AGGREGATE", &[num(9.0), num(0.0), view()]),
        e(CellError::Div0)
    );
    assert_eq!(
        call("AGGREGATE", &[num(9.0), num(4.0), view()]),
        e(CellError::Div0)
    );
}

#[test]
fn sheet_fn_t2misc_aggregate_option6_ignores_errors() {
    // Option 6 = ignore error values: SUM skips the error cell -> 1+3 = 4.
    let cells = [n(1.0), e(CellError::Na), n(3.0)];
    assert_eq!(
        call(
            "AGGREGATE",
            &[num(9.0), num(6.0), Arg::Range(row_view(&cells))]
        ),
        n(4.0)
    );
}

#[test]
fn sheet_fn_t2misc_aggregate_unsupported_function_num() {
    // 12-19 are deferred in T2; 0 / 20 are invalid. All -> #VALUE!.
    for fnum in [0.0, 12.0, 14.0, 19.0, 20.0] {
        assert_eq!(
            call("AGGREGATE", &[num(fnum), num(0.0), num(5.0)]),
            e(CellError::Value),
            "function_num {fnum} should be #VALUE!"
        );
    }
}

#[test]
fn sheet_fn_t2misc_aggregate_unsupported_options() {
    // Hidden-row options need metadata a pure kernel cannot see -> #VALUE!.
    for opt in [1.0, 2.0, 3.0, 5.0, 7.0, 8.0] {
        assert_eq!(
            call("AGGREGATE", &[num(9.0), num(opt), num(5.0)]),
            e(CellError::Value),
            "option {opt} should be #VALUE!"
        );
    }
}

#[test]
fn sheet_fn_t2misc_aggregate_coercion_and_arity() {
    // function_num/options coerce from numeric text; scalar refs coerce too.
    // AGGREGATE("9","0",TRUE,"3") = SUM(1,3) = 4.
    assert_eq!(
        call("AGGREGATE", &[txt("9"), txt("0"), boolean(true), txt("3")]),
        n(4.0)
    );
    // min arity is 3 -> two args is #VALUE! (the generated guard).
    assert_eq!(
        call("AGGREGATE", &[num(9.0), num(0.0)]),
        e(CellError::Value)
    );
}

#[test]
fn sheet_fn_t2misc_aggregate_selector_error_propagates() {
    // An error function_num propagates before selection.
    assert_eq!(
        call("AGGREGATE", &[err(CellError::Ref), num(0.0), num(5.0)]),
        e(CellError::Ref)
    );
}

// ============================================================================
// SUBTOTAL
// ============================================================================

#[test]
fn sheet_fn_t2misc_subtotal_basic_and_hidden_parity() {
    let cells = [n(10.0), n(20.0), n(30.0)];
    let view = || Arg::Range(row_view(&cells));
    // 9=SUM, 1=AVERAGE, 4=MAX, 2=COUNT.
    assert_eq!(call("SUBTOTAL", &[num(9.0), view()]), n(60.0));
    assert_eq!(call("SUBTOTAL", &[num(1.0), view()]), n(20.0));
    assert_eq!(call("SUBTOTAL", &[num(4.0), view()]), n(30.0));
    assert_eq!(call("SUBTOTAL", &[num(2.0), view()]), n(3.0));
    // 101-111 == 1-11 in T2 (no row-visibility metadata).
    assert_eq!(call("SUBTOTAL", &[num(109.0), view()]), n(60.0));
    assert_eq!(call("SUBTOTAL", &[num(101.0), view()]), n(20.0));
}

#[test]
fn sheet_fn_t2misc_subtotal_error_propagates() {
    // SUBTOTAL has no ignore-error option -> an error cell propagates.
    let cells = [n(1.0), e(CellError::Value), n(3.0)];
    assert_eq!(
        call("SUBTOTAL", &[num(9.0), Arg::Range(row_view(&cells))]),
        e(CellError::Value)
    );
}

#[test]
fn sheet_fn_t2misc_subtotal_bad_function_num_and_arity() {
    // 0 / 12 / 112 are not in 1-11 or 101-111 -> #VALUE!.
    assert_eq!(call("SUBTOTAL", &[num(0.0), num(1.0)]), e(CellError::Value));
    assert_eq!(
        call("SUBTOTAL", &[num(12.0), num(1.0)]),
        e(CellError::Value)
    );
    assert_eq!(
        call("SUBTOTAL", &[num(112.0), num(1.0)]),
        e(CellError::Value)
    );
    // min arity 2 -> one arg is #VALUE! (the generated guard).
    assert_eq!(call("SUBTOTAL", &[num(9.0)]), e(CellError::Value));
}

// ============================================================================
// ROMAN
// ============================================================================

#[test]
fn sheet_fn_t2misc_roman_classic() {
    assert_eq!(call("ROMAN", &[num(1.0)]), t("I"));
    assert_eq!(call("ROMAN", &[num(4.0)]), t("IV"));
    assert_eq!(call("ROMAN", &[num(9.0)]), t("IX"));
    assert_eq!(call("ROMAN", &[num(49.0)]), t("XLIX"));
    assert_eq!(call("ROMAN", &[num(1994.0)]), t("MCMXCIV"));
    assert_eq!(call("ROMAN", &[num(2024.0)]), t("MMXXIV"));
    assert_eq!(call("ROMAN", &[num(3999.0)]), t("MMMCMXCIX"));
}

#[test]
fn sheet_fn_t2misc_roman_zero_and_truncation_and_form() {
    // 0 -> "" (Excel). Non-integer truncates toward zero.
    assert_eq!(call("ROMAN", &[num(0.0)]), t(""));
    assert_eq!(call("ROMAN", &[num(9.9)]), t("IX"));
    // [form] 0-4 accepted (T2 renders classic for all).
    assert_eq!(call("ROMAN", &[num(4.0), num(0.0)]), t("IV"));
    assert_eq!(call("ROMAN", &[num(4.0), num(4.0)]), t("IV"));
    // Numeric-text scalar coerces.
    assert_eq!(call("ROMAN", &[txt("12")]), t("XII"));
}

#[test]
fn sheet_fn_t2misc_roman_out_of_domain_and_errors() {
    // Negative or >3999 -> #VALUE! (Excel).
    assert_eq!(call("ROMAN", &[num(-1.0)]), e(CellError::Value));
    assert_eq!(call("ROMAN", &[num(4000.0)]), e(CellError::Value));
    // form out of 0-4 -> #VALUE!.
    assert_eq!(call("ROMAN", &[num(4.0), num(5.0)]), e(CellError::Value));
    // Error / non-numeric arg propagates / #VALUE!.
    assert_eq!(call("ROMAN", &[err(CellError::Na)]), e(CellError::Na));
    assert_eq!(call("ROMAN", &[txt("abc")]), e(CellError::Value));
    // arity: zero args -> #VALUE! (the generated guard, min 1).
    assert_eq!(call("ROMAN", &[]), e(CellError::Value));
}

// ============================================================================
// ARABIC
// ============================================================================

#[test]
fn sheet_fn_t2misc_arabic_basic() {
    assert_eq!(call("ARABIC", &[txt("LVII")]), n(57.0));
    assert_eq!(call("ARABIC", &[txt("MCMXCIV")]), n(1994.0));
    assert_eq!(call("ARABIC", &[txt("MMXXIV")]), n(2024.0));
    // Case-folded (Excel accepts lower-case).
    assert_eq!(call("ARABIC", &[txt("mcmxciv")]), n(1994.0));
    // Round-trips classic ROMAN.
    let r = call("ROMAN", &[num(888.0)]);
    if let CellValue::Text(s) = r {
        assert_eq!(call("ARABIC", &[txt(s.as_str())]), n(888.0));
    } else {
        panic!("ROMAN(888) should be text, got {r:?}");
    }
}

#[test]
fn sheet_fn_t2misc_arabic_empty_negative_and_errors() {
    // "" / whitespace -> 0.
    assert_eq!(call("ARABIC", &[txt("")]), n(0.0));
    assert_eq!(call("ARABIC", &[txt("   ")]), n(0.0));
    // Leading '-' negates.
    assert_eq!(call("ARABIC", &[txt("-IV")]), n(-4.0));
    // Non-roman text -> #VALUE!.
    assert_eq!(call("ARABIC", &[txt("hello")]), e(CellError::Value));
    // Error arg propagates.
    assert_eq!(call("ARABIC", &[err(CellError::Div0)]), e(CellError::Div0));
    // arity: zero args -> #VALUE! (min 1).
    assert_eq!(call("ARABIC", &[]), e(CellError::Value));
}

// ============================================================================
// CONVERT
// ============================================================================

/// Assert a CONVERT result is numerically within 1e-9 of `want`. The
/// multiplicative path (value × from / to) carries inherent f64 rounding for
/// factors that are not power-of-ten ratios (`ft↔in`), so the exact bit
/// pattern is fuzzy even though the General projection rounds clean ("12").
/// The goldens pin the projected display; this pins the numeric magnitude.
fn assert_convert_approx(got: CellValue, want: f64) {
    match got {
        CellValue::Number(g) => assert!(
            (g - want).abs() < 1e-9,
            "CONVERT result {g} not within 1e-9 of {want}"
        ),
        other => panic!("expected a number, got {other:?}"),
    }
}

#[test]
fn sheet_fn_t2misc_convert_length_mass_time() {
    // Length. (ft↔in carries f64 fuzz: 12.000000000000002 — magnitude check.)
    assert_eq!(call("CONVERT", &[num(1.0), txt("km"), txt("m")]), n(1000.0));
    assert_convert_approx(call("CONVERT", &[num(1.0), txt("ft"), txt("in")]), 12.0);
    assert_convert_approx(call("CONVERT", &[num(12.0), txt("in"), txt("ft")]), 1.0);
    // Mass.
    assert_eq!(call("CONVERT", &[num(2.0), txt("kg"), txt("g")]), n(2000.0));
    // Time.
    assert_eq!(
        call("CONVERT", &[num(2.0), txt("hr"), txt("min")]),
        n(120.0)
    );
    assert_eq!(call("CONVERT", &[num(1.0), txt("day"), txt("hr")]), n(24.0));
    // Same unit -> identity.
    assert_eq!(call("CONVERT", &[num(7.0), txt("m"), txt("m")]), n(7.0));
}

#[test]
fn sheet_fn_t2misc_convert_temperature_affine() {
    assert_eq!(call("CONVERT", &[num(100.0), txt("C"), txt("F")]), n(212.0));
    assert_eq!(call("CONVERT", &[num(212.0), txt("F"), txt("C")]), n(100.0));
    assert_eq!(call("CONVERT", &[num(0.0), txt("C"), txt("K")]), n(273.15));
    assert_eq!(call("CONVERT", &[num(273.15), txt("K"), txt("C")]), n(0.0));
}

#[test]
fn sheet_fn_t2misc_convert_unknown_and_cross_dimension() {
    // Unknown unit -> #N/A.
    assert_eq!(
        call("CONVERT", &[num(1.0), txt("furlong"), txt("m")]),
        e(CellError::Na)
    );
    // Cross-dimension -> #N/A.
    assert_eq!(
        call("CONVERT", &[num(1.0), txt("m"), txt("kg")]),
        e(CellError::Na)
    );
    // Units are case-sensitive: "M" is not a known unit -> #N/A.
    assert_eq!(
        call("CONVERT", &[num(1.0), txt("M"), txt("m")]),
        e(CellError::Na)
    );
    // Error in the number arg propagates.
    assert_eq!(
        call("CONVERT", &[err(CellError::Ref), txt("m"), txt("ft")]),
        e(CellError::Ref)
    );
    // arity is exactly 3: two args -> #VALUE!.
    assert_eq!(call("CONVERT", &[num(1.0), txt("m")]), e(CellError::Value));
}

// ============================================================================
// HYPERLINK
// ============================================================================

#[test]
fn sheet_fn_t2misc_hyperlink_display_only() {
    // With a friendly name -> the friendly text.
    assert_eq!(
        call("HYPERLINK", &[txt("https://paged.media"), txt("Paged")]),
        t("Paged")
    );
    // Without friendly -> the link text.
    assert_eq!(
        call("HYPERLINK", &[txt("https://paged.media")]),
        t("https://paged.media")
    );
    // Blank friendly -> falls back to the link text.
    assert_eq!(
        call("HYPERLINK", &[txt("https://paged.media"), txt("")]),
        t("https://paged.media")
    );
    // A numeric friendly coerces to its General text.
    assert_eq!(call("HYPERLINK", &[txt("x"), num(42.0)]), t("42"));
    // Error arg propagates.
    assert_eq!(
        call("HYPERLINK", &[err(CellError::Value), txt("y")]),
        e(CellError::Value)
    );
    // arity: zero args -> #VALUE! (min 1).
    assert_eq!(call("HYPERLINK", &[]), e(CellError::Value));
}
